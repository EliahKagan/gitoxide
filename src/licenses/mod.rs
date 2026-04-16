//! Binary-specific plumbing for the third-party dependency license manifest.
//!
//! The data model, renderers, SPDX fallback table, and build-time helpers
//! live in `gitoxide_core::licenses` — this module only owns the pieces
//! that are tied to *this* binary: the compressed JSON blob that
//! `build.rs` embeds via `include_bytes!`, its deserialisation into a
//! [`Manifest`], and the CLI dispatcher that backs `gix licenses` and
//! `ein licenses`.
//!
//! Callers who need the shared types or a `render_*` function should pull
//! them from `gitoxide_core::licenses::{render, types}` (or via the
//! convenience re-exports below); this module does not re-publish every
//! detail of `gitoxide_core::licenses`.

pub mod cli;
pub mod embedded;

pub use embedded::{json, load, JSON_GZ};
// Re-exported so callers inside the `gitoxide` crate can stay on
// `crate::licenses::{Manifest, CrateLicense, LicenseFile}` without
// knowing whether the types live here or in `gitoxide-core`.
pub use gitoxide_core::licenses::{render_all, render_crate, render_summary, CrateLicense, LicenseFile, Manifest};
