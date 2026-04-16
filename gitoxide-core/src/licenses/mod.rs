//! Data model, renderers, SPDX fallback table, and build-time helpers for the
//! third-party dependency license manifest shipped with the `gix` and `ein`
//! binaries.
//!
//! These modules are intentionally data- and platform-pure: they know how to
//! describe, sort, scan, and render attribution material, but they do not
//! know how the manifest gets into a binary. That binary-specific plumbing
//! (the `build.rs` that emits the manifest for the `gitoxide` package, and
//! the `include_bytes!`-backed runtime loader that decompresses it) lives
//! in the `gitoxide` crate's own `src/licenses/` tree alongside the CLI
//! dispatcher for the `gix licenses` / `ein licenses` subcommands.
//!
//! Gated on the `serde` feature because the manifest crosses the build ↔
//! runtime boundary as JSON. The `gitoxide` binary always enables
//! `gitoxide-core/serde` via its `pretty-cli` feature (which is in turn
//! pulled in by every shipping feature profile), so from the binary's
//! perspective this gate is invisible.
//!
//! # Why a custom implementation rather than an existing tool?
//!
//! Several established tools solve adjacent problems. None of them fit the
//! constraint set imposed by gitoxide's release surface, which is roughly:
//! *every build path that produces a `gix` or `ein` binary must produce
//! complete attribution for that binary with zero additional tooling on the
//! user's machine*. Concretely that includes `cargo build` and
//! `cargo install` by end users, source builds by intermediaries such as
//! QuickInstall, and the release workflow's `cross` / native matrix.
//!
//! * **[cargo-about]**. Mature SPDX-text database, widely used, well
//!   maintained. But it is a CLI binary with no stable library API, so it
//!   can only be invoked from `build.rs` as a subprocess. That forces
//!   anyone running `cargo install gitoxide` (or anyone rebuilding from
//!   crates.io) to have `cargo-about` already installed on their machine.
//!   Since there is no way to express "run if available, skip if not"
//!   without silently shipping binaries with no attribution for most
//!   users, this is a non-starter for the primary pipeline. (We do keep
//!   cargo-about on the list as a CI parity / cross-check candidate.)
//!
//! * **[cargo-bundle-licenses]**. Similar shape to cargo-about — primarily
//!   a CLI with a thin helper library. Same "must be installed on every
//!   user's machine" problem.
//!
//! * **[license-retriever]**. A Rust crate designed to be called from
//!   `build.rs`, which would sidestep the CLI-tool problem. The usable
//!   surface is small and the crate is relatively young; pulling it in
//!   would mean trusting an under-reviewed dependency with the legally
//!   sensitive job of describing every other dependency's license. The
//!   volume of code we'd have to write ourselves to achieve roughly the
//!   same thing is about 200 lines of filesystem scanning and SPDX
//!   expression splitting, which is what this module contains.
//!
//! * **[cargo-license] / [cargo-3pl] / [cargo-attribution]**. Report SPDX
//!   identifiers and/or authors but not license text, or are insufficiently
//!   maintained.
//!
//! In short, none of the established tools can produce complete
//! attribution across all of gitoxide's build paths today. The CLI-based
//! ones cannot be used from `build.rs` without imposing an installation
//! prerequisite on every person who runs `cargo install gitoxide`, the
//! one library-based option is not yet mature enough for the role, and
//! the rest do not emit license text at all. Writing the pipeline
//! ourselves was a last resort, not a preference — it is roughly 200
//! lines of filesystem scanning and SPDX expression splitting, which is
//! small but still code we have to maintain. If any of the above tools
//! gains a stable, widely-adopted build-time API in the future, migrating
//! to it would be preferable.
//!
//! [cargo-about]: https://github.com/EmbarkStudios/cargo-about
//! [cargo-bundle-licenses]: https://github.com/sstadick/cargo-bundle-licenses
//! [license-retriever]: https://github.com/Cryptex-github/license-retriever
//! [cargo-license]: https://github.com/onur/cargo-license
//! [cargo-3pl]: https://github.com/davidB/cargo-3pl
//! [cargo-attribution]: https://github.com/ameknite/cargo-attribution

pub mod build_support;
pub mod render;
pub mod spdx_texts;
pub mod types;

pub use render::{render_all, render_crate, render_summary};
pub use types::{CrateLicense, LicenseFile, Manifest};
