//! Process-global interrupt handling
//!
//! This module contains facilities to globally request an interrupt, which will cause supporting computations to
//! abort once it is observed.
//! Such checks for interrupts are provided in custom implementations of various traits to transparently add interrupt
//! support to methods who wouldn't otherwise by injecting it. see [`Read`].

mod init {
    use std::{
        io,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    static IS_INITIALIZED: AtomicBool = AtomicBool::new(false);

    #[derive(Default)]
    pub struct Deregister(Vec<(i32, signal_hook::SigId)>);
    pub struct AutoDeregister(Deregister);

    impl Deregister {
        /// Remove all previously registered handlers, and assure the default behaviour is reinstated.
        ///
        /// Note that only the instantiation of the default behaviour can fail.
        pub fn deregister(self) -> std::io::Result<()> {
            if self.0.is_empty() {
                return Ok(());
            }
            static REINSTATE_DEFAULT_BEHAVIOUR: AtomicBool = AtomicBool::new(true);
            for (_, hook_id) in &self.0 {
                signal_hook::low_level::unregister(*hook_id);
            }
            IS_INITIALIZED.store(false, Ordering::SeqCst);
            if REINSTATE_DEFAULT_BEHAVIOUR
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |_| Some(false))
                .expect("always returns value")
            {
                for (sig, _) in self.0 {
                    // # SAFETY
                    // * we only call a handler that is specifically designed to run in this environment.
                    #[allow(unsafe_code)]
                    unsafe {
                        signal_hook::low_level::register(sig, move || {
                            signal_hook::low_level::emulate_default_handler(sig).ok();
                        })?;
                    }
                }
            }
            Ok(())
        }

        /// Return a type that deregisters all installed signal handlers on drop.
        pub fn auto_deregister(self) -> AutoDeregister {
            AutoDeregister(self)
        }
    }

    impl Drop for AutoDeregister {
        fn drop(&mut self) {
            std::mem::take(&mut self.0).deregister().ok();
        }
    }

    /// Initialize a signal handler to listen to SIGINT and SIGTERM and trigger our [`trigger()`][super::trigger()] that way.
    /// Also trigger `interrupt()` which promises to never use a Mutex, allocate or deallocate.
    ///
    /// # Note
    ///
    /// It will abort the process on second press and won't inform the user about this behaviour either as we are unable to do so without
    /// deadlocking even when trying to write to stderr directly.
    pub fn init_handler(interrupt: impl Fn() + Send + Sync + Clone + 'static) -> io::Result<Deregister> {
        if IS_INITIALIZED
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |_| Some(true))
            .expect("always returns value")
        {
            return Err(io::Error::new(io::ErrorKind::Other, "Already initialized"));
        }
        let mut hooks = Vec::with_capacity(signal_hook::consts::TERM_SIGNALS.len());
        for sig in signal_hook::consts::TERM_SIGNALS {
            // # SAFETY
            // * we only set atomics or call functions that do
            // * there is no use of the heap
            let interrupt = interrupt.clone();
            #[allow(unsafe_code)]
            unsafe {
                let hook_id = signal_hook::low_level::register(*sig, move || {
                    static INTERRUPT_COUNT: AtomicUsize = AtomicUsize::new(0);
                    if !super::is_triggered() {
                        INTERRUPT_COUNT.store(0, Ordering::SeqCst);
                    }
                    let msg_idx = INTERRUPT_COUNT.fetch_add(1, Ordering::SeqCst);
                    if msg_idx == 1 {
                        gix_tempfile::handler::cleanup_tempfiles();
                        signal_hook::low_level::emulate_default_handler(*sig).ok();
                    }
                    interrupt();
                    super::trigger();
                })?;
                hooks.push((*sig, hook_id));
            }
        }

        // This means that they won't setup a handler allowing us to call them right before we actually abort.
        gix_tempfile::setup(gix_tempfile::SignalHandlerMode::None);

        Ok(Deregister(hooks))
    }
}
use std::{
    io,
    sync::atomic::{AtomicBool, Ordering},
};

pub use init::init_handler;

/// A wrapper for an inner iterator which will check for interruptions on each iteration.
pub struct Iter<I, EFN> {
    /// The actual iterator to yield elements from.
    inner: gix_features::interrupt::IterWithErr<'static, I, EFN>,
}

impl<I, EFN, E> Iter<I, EFN>
where
    I: Iterator,
    EFN: FnOnce() -> E,
{
    /// Create a new iterator over `inner` which checks for interruptions on each iteration and calls `make_err()` to
    /// signal an interruption happened, causing no further items to be iterated from that point on.
    pub fn new(inner: I, make_err: EFN) -> Self {
        Iter {
            inner: gix_features::interrupt::IterWithErr::new(inner, make_err, &IS_INTERRUPTED),
        }
    }

    /// Return the inner iterator
    pub fn into_inner(self) -> I {
        self.inner.inner
    }

    /// Return the inner iterator as reference
    pub fn inner(&self) -> &I {
        &self.inner.inner
    }
}

impl<I, EFN, E> Iterator for Iter<I, EFN>
where
    I: Iterator,
    EFN: FnOnce() -> E,
{
    type Item = Result<I::Item, E>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

/// A wrapper for implementors of [`std::io::Read`] or [`std::io::BufRead`] with interrupt support.
///
/// It fails a [read][`std::io::Read::read`] while an interrupt was requested.
pub struct Read<R> {
    /// The actual implementor of [`std::io::Read`] to which interrupt support will be added.
    inner: gix_features::interrupt::Read<'static, R>,
}

impl<R> Read<R>
where
    R: io::Read,
{
    /// Create a new interruptible reader from `read`.
    pub fn new(read: R) -> Self {
        Read {
            inner: gix_features::interrupt::Read {
                inner: read,
                should_interrupt: &IS_INTERRUPTED,
            },
        }
    }

    /// Return the inner reader
    pub fn into_inner(self) -> R {
        self.inner.inner
    }
}

impl<R> io::Read for Read<R>
where
    R: io::Read,
{
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl<R> io::BufRead for Read<R>
where
    R: io::BufRead,
{
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt)
    }
}

/// The flag behind all utility functions in this module.
pub static IS_INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Returns true if an interrupt is requested.
pub fn is_triggered() -> bool {
    IS_INTERRUPTED.load(Ordering::Relaxed)
}

/// Trigger an interrupt, signalling to those checking for [`is_triggered()`] to stop what they are doing.
pub fn trigger() {
    IS_INTERRUPTED.store(true, Ordering::SeqCst);
}

/// Sets the interrupt request to false, thus allowing those checking for [`is_triggered()`] to proceed.
pub fn reset() {
    IS_INTERRUPTED.store(false, Ordering::SeqCst);
}