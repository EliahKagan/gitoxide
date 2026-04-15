//! Data types describing the third-party dependency manifest.
//!
//! These types are shared by `build.rs` (which serializes a manifest into
//! `OUT_DIR/third_party_licenses.json` and a pre-rendered companion `.txt`)
//! and the runtime library (which embeds the JSON via `include_str!` and
//! deserializes it on demand to power the `gix licenses` / `ein licenses`
//! subcommands). Keeping both producers on the same schema means the archive
//! file and the subcommand output cannot drift.

use serde::{Deserialize, Serialize};

/// A single license-related file from a crate's source tree, such as
/// `LICENSE-MIT`, `COPYING`, `NOTICE`, or `AUTHORS`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseFile {
    /// The filename, relative to the crate's root (e.g. `"LICENSE-APACHE"`).
    pub name: String,
    /// The exact contents of the file.
    pub text: String,
}

/// A third-party dependency and everything needed to satisfy its license.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrateLicense {
    /// Crate name, e.g. `"anyhow"`.
    pub name: String,
    /// Crate version as declared in `Cargo.toml` / `Cargo.lock`.
    pub version: String,
    /// The `license` field from the crate's `Cargo.toml`, if any.
    ///
    /// This is typically an SPDX expression like `"MIT OR Apache-2.0"`,
    /// `"Apache-2.0 WITH LLVM-exception"`, or a plain identifier like `"MIT"`.
    pub spdx: Option<String>,
    /// Authors as listed in the crate's `Cargo.toml`.
    #[serde(default)]
    pub authors: Vec<String>,
    /// `repository` URL from `Cargo.toml`, if any.
    pub repository: Option<String>,
    /// `homepage` URL from `Cargo.toml`, if any.
    pub homepage: Option<String>,
    /// LICENSE / NOTICE / AUTHORS / COPYRIGHT / COPYING files found in the
    /// crate's source tree, in the order discovered.
    #[serde(default)]
    pub files: Vec<LicenseFile>,
    /// `true` if no license file was found in the crate source and we fell
    /// back to bundled canonical SPDX text. Surfaced in the summary so users
    /// know which attributions may be less complete than the upstream crate.
    #[serde(default)]
    pub used_spdx_fallback: bool,
}

/// The full manifest: one entry per third-party dependency linked into the
/// build that produced this binary, plus metadata about that build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Third-party dependencies, sorted by name then version.
    pub crates: Vec<CrateLicense>,
    /// Stamp identifying when the manifest was generated.
    ///
    /// Currently a `"unix=SECONDS"` string; may become ISO-8601 in a later
    /// iteration. `build.rs` honours `SOURCE_DATE_EPOCH` when present so
    /// reproducible builds are unaffected by wall-clock time.
    pub generated_at: String,
    /// Feature profile, e.g. `Some("max")`, or `None` if built with a
    /// non-profile feature combination.
    pub feature_profile: Option<String>,
    /// Target triple the binary was built for.
    pub target_triple: String,
}

impl CrateLicense {
    /// True if this crate has no attribution text at all — neither a
    /// discovered file nor a bundled SPDX fallback populated it.
    pub fn is_missing_text(&self) -> bool {
        self.files.is_empty()
    }
}

impl Manifest {
    /// Find a single crate by name. First match wins; when multiple versions
    /// are present, pass the full crate list through [`Manifest::crates`] and
    /// filter manually.
    pub fn find(&self, name: &str) -> Option<&CrateLicense> {
        self.crates.iter().find(|c| c.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_crate() -> CrateLicense {
        CrateLicense {
            name: "sample".into(),
            version: "1.2.3".into(),
            spdx: Some("MIT OR Apache-2.0".into()),
            authors: vec!["Alice <alice@example.com>".into()],
            repository: Some("https://example.com/sample".into()),
            homepage: None,
            files: vec![LicenseFile {
                name: "LICENSE-MIT".into(),
                text: "MIT text body".into(),
            }],
            used_spdx_fallback: false,
        }
    }

    #[test]
    fn crate_with_at_least_one_file_is_not_missing_text() {
        assert!(!sample_crate().is_missing_text());
    }

    #[test]
    fn crate_with_no_files_is_missing_text() {
        let mut c = sample_crate();
        c.files.clear();
        assert!(c.is_missing_text());
    }

    #[test]
    fn manifest_find_returns_match() {
        let manifest = Manifest {
            crates: vec![sample_crate()],
            generated_at: "2026-04-15T00:00:00Z".into(),
            feature_profile: Some("max".into()),
            target_triple: "aarch64-apple-darwin".into(),
        };
        assert_eq!(manifest.find("sample").map(|c| c.version.as_str()), Some("1.2.3"));
        assert!(manifest.find("missing").is_none());
    }
}
