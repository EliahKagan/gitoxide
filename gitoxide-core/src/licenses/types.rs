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
    /// `true` if this entry describes a member of gitoxide's own workspace
    /// rather than a third-party dependency. Workspace members appear in this
    /// list (rather than in [`Manifest::workspace_members_same_attribution`])
    /// when their license expression or authorship differs from the root
    /// `gitoxide` package and they therefore need their own attribution
    /// entry. Synthetic entries the build script may inject (such as the
    /// Rust standard library) are neither workspace members nor third-party
    /// crates from cargo's perspective and have this set to `false`.
    #[serde(default)]
    pub is_workspace_member: bool,
}

/// The full manifest: one entry per third-party dependency linked into the
/// build that produced this binary, plus the workspace members the root
/// package's `LICENSE-*` files already cover, plus metadata about that build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Full attribution entries: every third-party dep, every workspace
    /// member whose license or authorship differs from the root (so it
    /// needs its own entry), and any synthetic entry the build script
    /// injects (e.g. the Rust standard library on builds that attribute
    /// it). Sorted by name then version.
    pub crates: Vec<CrateLicense>,
    /// Workspace members that *are* linked into the binary but whose
    /// attribution is identical to the root `gitoxide` package's — same
    /// license, same authors. Listed by name only because repeating
    /// `gitoxide`'s own LICENSE-MIT / LICENSE-APACHE for each one would
    /// bloat the manifest without adding information. Sorted
    /// alphabetically.
    ///
    /// The listing exists so readers can tell at a glance which workspace
    /// crates are part of the shipped binary; without it, the absence of
    /// e.g. `gix-ref` would be indistinguishable from the presence of
    /// e.g. `gix-imara-diff`, obscuring the recognizability of the "this
    /// workspace crate has separate attribution" case.
    #[serde(default)]
    pub workspace_members_same_attribution: Vec<String>,
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
    /// Return every entry in [`Manifest::crates`] whose `name` matches.
    ///
    /// Most names resolve to a single entry. The Vec form makes the
    /// multi-version case visible at the call site: if cargo resolved
    /// two different versions of the same crate (each potentially under
    /// a different license expression), both must be surfaced — silently
    /// dropping all but the first would under-attribute the build.
    /// Entries are returned in the order they appear in `crates`.
    pub fn find_all(&self, name: &str) -> Vec<&CrateLicense> {
        self.crates.iter().filter(|c| c.name == name).collect()
    }

    /// Return `true` if `name` is a workspace member whose attribution is
    /// identical to the root `gitoxide` package's — i.e. it appears in
    /// [`Manifest::workspace_members_same_attribution`] rather than in
    /// [`Manifest::crates`].
    pub fn is_same_attribution_workspace_member(&self, name: &str) -> bool {
        self.workspace_members_same_attribution.iter().any(|n| n == name)
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
            is_workspace_member: false,
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
    fn manifest_find_all_returns_match() {
        let manifest = Manifest {
            crates: vec![sample_crate()],
            workspace_members_same_attribution: Vec::new(),
            generated_at: "2026-04-15T00:00:00Z".into(),
            feature_profile: Some("max".into()),
            target_triple: "aarch64-apple-darwin".into(),
        };
        let hits = manifest.find_all("sample");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].version, "1.2.3");
        assert!(manifest.find_all("missing").is_empty());
    }

    /// When two entries in the manifest share a name (different versions,
    /// possibly different licenses), `find_all` must surface all of them
    /// in the order they appear in `crates`. The CLI relies on this so
    /// that `gix licenses <name>` against an ambiguous name shows every
    /// matching crate rather than silently dropping all but the first.
    #[test]
    fn manifest_find_all_returns_every_matching_version() {
        let mut v1 = sample_crate();
        v1.version = "1.0.0".into();
        v1.spdx = Some("MIT".into());
        let mut v2 = sample_crate();
        v2.version = "2.0.0".into();
        v2.spdx = Some("Apache-2.0".into());

        let manifest = Manifest {
            crates: vec![v1, v2],
            workspace_members_same_attribution: Vec::new(),
            generated_at: "t".into(),
            feature_profile: None,
            target_triple: "x86_64-unknown-linux-gnu".into(),
        };
        let hits = manifest.find_all("sample");
        assert_eq!(hits.len(), 2, "expected both versions of `sample`");
        assert_eq!(hits[0].version, "1.0.0");
        assert_eq!(hits[1].version, "2.0.0");
        assert_eq!(hits[0].spdx.as_deref(), Some("MIT"));
        assert_eq!(hits[1].spdx.as_deref(), Some("Apache-2.0"));
    }

    #[test]
    fn workspace_members_same_attribution_defaults_to_empty_on_deserialize() {
        // Older builds without the field must still deserialize so
        // `load()` continues to work for embedded blobs captured before
        // the field was added.
        let without_field = r#"{
          "crates": [],
          "generated_at": "t",
          "feature_profile": null,
          "target_triple": "x86_64-unknown-linux-gnu"
        }"#;
        let m: Manifest = serde_json::from_str(without_field).expect("deserialize without the field");
        assert!(m.workspace_members_same_attribution.is_empty());
    }

    #[test]
    fn is_workspace_member_defaults_to_false_on_deserialize() {
        // Older blobs without the per-crate `is_workspace_member` field must
        // still deserialize. The field defaults to `false` (i.e. the entry is
        // assumed to be a third-party crate, which is the only kind we used
        // to record before the workspace-member-with-separate-attribution
        // case got its own typed indicator).
        let json = r#"{
          "name": "anyhow",
          "version": "1.0.98",
          "spdx": "MIT OR Apache-2.0",
          "authors": [],
          "repository": null,
          "homepage": null,
          "files": []
        }"#;
        let c: CrateLicense = serde_json::from_str(json).expect("deserialize without the field");
        assert!(!c.is_workspace_member);
    }

    #[test]
    fn is_workspace_member_round_trips_through_serde() {
        let mut c = sample_crate();
        c.is_workspace_member = true;
        let json = serde_json::to_string(&c).expect("serialize");
        let back: CrateLicense = serde_json::from_str(&json).expect("deserialize");
        assert!(back.is_workspace_member);
    }
}
