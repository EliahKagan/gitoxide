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

pub mod build_support;
pub mod render;
pub mod spdx_texts;
pub mod types;

pub use render::{render_all, render_crate, render_summary};
pub use types::{CrateLicense, LicenseFile, Manifest};
