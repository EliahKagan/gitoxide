//! Third-party dependency license manifest for the `gix` and `ein` binaries.
//!
//! The goal is to ship full license, attribution, copyright, and notice
//! information for every third-party dependency linked into `gix` and `ein`,
//! so users and downstream packagers can satisfy the attribution requirements
//! of every license in the tree without inspecting the source separately.
//!
//! At build time, `build.rs` will emit a machine-readable manifest (and a
//! pre-rendered plain-text version) into `OUT_DIR`. Both are embedded in the
//! binaries via `include_str!` and also copied into release archives. This
//! module owns the types, the SPDX canonical-text fallback library, and the
//! rendering functions that turn a manifest into human-readable output.
//!
//! At this stage the build-time wiring is not yet in place; only the types,
//! renderers, and SPDX fallback table exist, exercised by unit tests with
//! hand-built manifests.

pub mod build_support;
pub mod cli;
pub mod embedded;
pub mod render;
pub mod spdx_texts;
pub mod types;

pub use embedded::{json, load, JSON_GZ};
pub use render::{render_all, render_crate, render_summary};
pub use types::{CrateLicense, LicenseFile, Manifest};
