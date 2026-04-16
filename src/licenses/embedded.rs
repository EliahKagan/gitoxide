//! Access to the third-party dependency manifest that `build.rs` embeds into
//! the binaries at compile time.
//!
//! The build script writes three files into `OUT_DIR`:
//!
//! * `third_party_licenses.json`      — uncompressed structured data; the
//!   release workflow copies this straight into the archive as
//!   `THIRD-PARTY-LICENSES.json`. Not embedded in the binary (~3 MB).
//! * `third_party_licenses.json.gz`   — zlib-compressed JSON; embedded here
//!   via `include_bytes!` and decompressed on demand by [`load`]. Keeps
//!   binary growth on the order of a few hundred KB.
//! * `third_party_licenses.txt`       — pre-rendered plain text, shipped as
//!   `THIRD-PARTY-LICENSES.txt` in the release archive. The runtime does not
//!   embed this; `gitoxide_core::licenses::render_all` can regenerate
//!   byte-identical output from a loaded [`Manifest`] whenever
//!   `gix licenses --all` or `ein licenses --all` is invoked.

use std::io;

use gitoxide_core::licenses::Manifest;

/// Zlib-compressed JSON manifest, embedded at compile time.
pub const JSON_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/third_party_licenses.json.gz"));

/// Decompress the embedded JSON into an owned UTF-8 string.
///
/// This is exposed so the `licenses --format json` subcommand can stream the
/// manifest verbatim without re-serialising a parsed `Manifest`.
pub fn json() -> io::Result<String> {
    let bytes = miniz_oxide::inflate::decompress_to_vec_zlib(JSON_GZ)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("inflate failed: {e:?}")))?;
    String::from_utf8(bytes).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("embedded manifest is not valid UTF-8: {e}"),
        )
    })
}

/// Parse the embedded manifest into a [`Manifest`].
///
/// Errors here are treated as fatal by the subcommands — they indicate a
/// build-script/runtime schema drift or a corrupted embedded blob, neither
/// of which the CLI can recover from usefully.
pub fn load() -> io::Result<Manifest> {
    let json_str = json()?;
    serde_json::from_str(&json_str).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("failed to parse embedded license manifest: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_blob_decompresses_to_utf8_json() {
        let text = json().expect("decompression and utf-8 check must succeed");
        assert!(!text.is_empty(), "decompressed JSON must be non-empty");
        assert!(
            text.trim_start().starts_with('{'),
            "decompressed output should be a JSON object"
        );
    }

    #[test]
    fn embedded_manifest_round_trips_through_serde() {
        let manifest = load().expect("embedded manifest must round-trip through serde");
        // Either the real manifest (many crates) or a stub (zero crates,
        // generated_at starting with "stub;"). Both are acceptable; anything
        // that fails to deserialise is not.
        assert!(!manifest.target_triple.is_empty() || manifest.generated_at.starts_with("stub"));
    }

    /// The manifest must expose a non-empty target triple — empty would mean
    /// `build.rs` couldn't tell cargo what platform it built against and we
    /// can no longer tell users what configuration their binary was built
    /// from.
    #[test]
    fn manifest_has_non_empty_target_triple() {
        let manifest = load().expect("load manifest");
        assert!(
            !manifest.target_triple.is_empty(),
            "target_triple is empty; generated_at={:?}",
            manifest.generated_at,
        );
    }

    /// A real build resolves at least several dozen third-party deps even
    /// for the smallest feature profile, so anything under this floor is
    /// almost certainly a `build.rs` regression (e.g. wrong filter, stub
    /// manifest written when the real one was expected).
    #[test]
    fn manifest_has_plausible_crate_count() {
        let manifest = load().expect("load manifest");
        assert!(
            manifest.crates.len() >= 50,
            "implausibly few third-party crates ({}); generated_at={:?}",
            manifest.crates.len(),
            manifest.generated_at,
        );
    }

    /// Every crate surfaced by the manifest must carry at least one
    /// attribution file, either discovered in its source tree or sourced
    /// from the bundled SPDX fallback. A crate with zero files is one we
    /// couldn't attribute at all and so would legally be shipped
    /// under-attributed — never acceptable on a release.
    #[test]
    fn every_crate_has_at_least_one_license_file() {
        let manifest = load().expect("load manifest");
        let missing: Vec<String> = manifest
            .crates
            .iter()
            .filter(|c| c.files.is_empty())
            .map(|c| format!("{} {} (spdx={:?})", c.name, c.version, c.spdx))
            .collect();
        assert!(missing.is_empty(), "crates with no attribution text: {missing:?}",);
    }

    /// The manifest must include both direct and transitive dependencies.
    /// `anyhow` is a direct dep of the `gitoxide` package; `memchr` is
    /// only reachable transitively (through `aho-corasick`, `regex`, or
    /// similar). If either is absent, the resolution or filtering logic
    /// in `build.rs` has regressed.
    #[test]
    fn manifest_includes_direct_and_transitive_deps() {
        let manifest = load().expect("load manifest");
        let names: std::collections::BTreeSet<&str> = manifest.crates.iter().map(|c| c.name.as_str()).collect();
        // Direct dep of gitoxide (listed in Cargo.toml [dependencies]).
        assert!(
            names.contains("anyhow"),
            "direct dep `anyhow` must appear in the manifest",
        );
        // Transitive-only dep — not in gitoxide's Cargo.toml but pulled
        // in through the dep graph. `memchr` is used by multiple crates
        // (aho-corasick, regex, etc.) and has been in the tree for years,
        // making it a stable sentinel.
        assert!(
            names.contains("memchr"),
            "transitive dep `memchr` must appear in the manifest",
        );
    }

    /// `build.rs` derives `feature_profile` from the `CARGO_FEATURE_*` env
    /// set at build time. The test binary is compiled with the same feature
    /// set (via `cargo test --features X`), so `cfg!(feature = X)` here must
    /// agree with what ended up in the embedded manifest. This is the
    /// invariant that the old `licenses-smoke` CI Python check asserted,
    /// moved to the same place as every other invariant it guarded.
    #[test]
    fn manifest_feature_profile_matches_build_time_cfg() {
        // Only the five well-known top-level profiles can produce a
        // `Some(...)` — match the exact list `build.rs::detect_feature_profile`
        // walks so the tests agrees for free when a new profile lands there.
        let mut enabled: Vec<&'static str> = Vec::new();
        if cfg!(feature = "max") {
            enabled.push("max");
        }
        if cfg!(feature = "max-pure") {
            enabled.push("max-pure");
        }
        if cfg!(feature = "lean") {
            enabled.push("lean");
        }
        if cfg!(feature = "small") {
            enabled.push("small");
        }
        if cfg!(feature = "lean-async") {
            enabled.push("lean-async");
        }
        let expected = match enabled.as_slice() {
            [one] => Some((*one).to_string()),
            _ => None,
        };
        let manifest = load().expect("load manifest");
        assert_eq!(
            manifest.feature_profile, expected,
            "feature_profile mismatch: cfg!-derived={expected:?}, manifest={:?}, enabled={enabled:?}",
            manifest.feature_profile,
        );
    }

    #[test]
    fn runtime_render_matches_archive_txt_byte_for_byte() {
        // The `THIRD-PARTY-LICENSES.txt` in release archives is generated by
        // `build.rs` via `render_all`. The binary regenerates the same bytes
        // at runtime from the embedded JSON via the very same function — so
        // `gix licenses --all` output must equal the archive file verbatim.
        let manifest = load().expect("load manifest");
        let mut rendered = Vec::new();
        gitoxide_core::licenses::render_all(&mut rendered, &manifest).expect("render_all");

        let txt_from_out_dir = std::fs::read(concat!(env!("OUT_DIR"), "/third_party_licenses.txt"))
            .expect("archive .txt must exist in OUT_DIR");

        assert_eq!(
            rendered,
            txt_from_out_dir,
            "runtime rendering drifted from build-time rendering (byte counts: runtime={}, build={})",
            rendered.len(),
            txt_from_out_dir.len(),
        );
    }

    /// Every crate the embedded manifest surfaces must be coverable by
    /// `deny.toml`'s `[licenses] allow = [...]` list.
    ///
    /// This mirrors cargo-deny's semantics: a crate passes when at least one
    /// SPDX identifier in its expression is in the allowlist for `OR` unions,
    /// and every identifier is in the allowlist for `AND` conjunctions. Our
    /// SPDX parser is slightly different from cargo-deny's, which makes this
    /// a useful belt-and-suspenders check for drift.
    ///
    /// We approximate the per-crate rule as "the intersection of this crate's
    /// parsed SPDX ids with the allowlist must be non-empty"; a full
    /// boolean-expression evaluator would be more faithful but would require
    /// reproducing cargo-deny's SPDX parser here, which is overkill.
    #[test]
    fn every_crate_is_coverable_by_deny_toml_allowlist() {
        use std::collections::BTreeSet;
        use std::path::Path;

        let manifest = load().expect("load manifest");
        let deny_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("deny.toml");
        let deny_content = std::fs::read_to_string(&deny_path).expect("read deny.toml");
        let allow: BTreeSet<String> = parse_deny_allow_list(&deny_content);
        assert!(
            !allow.is_empty(),
            "parse_deny_allow_list returned no SPDX ids — the parser broke, not the data"
        );

        let mut unsatisfied: Vec<(String, String, Vec<String>)> = Vec::new();
        for c in &manifest.crates {
            let Some(expr) = c.spdx.as_deref() else {
                continue;
            };
            let ids = gitoxide_core::licenses::build_support::parse_spdx_ids(expr);
            let intersection: Vec<String> = ids.iter().filter(|id| allow.contains(*id)).cloned().collect();
            if intersection.is_empty() {
                unsatisfied.push((c.name.clone(), c.version.clone(), ids));
            }
        }
        assert!(
            unsatisfied.is_empty(),
            "crates whose SPDX expression has no overlap with deny.toml allowlist: {unsatisfied:?}",
        );
    }

    fn parse_deny_allow_list(contents: &str) -> std::collections::BTreeSet<String> {
        use std::collections::BTreeSet;

        // Minimal TOML scanner targeted at `deny.toml`'s shape — finds the
        // `[licenses]` table's `allow` array and collects every double-quoted
        // SPDX identifier inside it, across a single-line or multi-line form.
        // A full TOML parser would be nicer but pulling one in as a dev-dep
        // just to read this shape feels disproportionate.
        let mut allow = BTreeSet::new();
        let mut in_licenses_section = false;
        let mut collecting_allow = false;
        for raw_line in contents.lines() {
            let line = strip_toml_comment(raw_line).trim();
            if let Some(section) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                in_licenses_section = section.trim() == "licenses";
                collecting_allow = false;
                continue;
            }
            if !in_licenses_section {
                continue;
            }
            let body = if collecting_allow {
                Some(line)
            } else if let Some(rest) = line.strip_prefix("allow") {
                let rest = rest.trim_start();
                let rest = rest.strip_prefix('=').map_or(rest, str::trim_start);
                if let Some(after_open) = rest.strip_prefix('[') {
                    collecting_allow = true;
                    Some(after_open)
                } else {
                    None
                }
            } else {
                None
            };
            if let Some(body) = body {
                let (before_close, hit_close) = match body.find(']') {
                    Some(idx) => (&body[..idx], true),
                    None => (body, false),
                };
                extract_quoted_ids(before_close, &mut allow);
                if hit_close {
                    collecting_allow = false;
                }
            }
        }
        allow
    }

    fn strip_toml_comment(line: &str) -> &str {
        // Good enough: comments in deny.toml's allow section don't contain `"`.
        line.find('#').map_or(line, |idx| &line[..idx])
    }

    fn extract_quoted_ids(s: &str, out: &mut std::collections::BTreeSet<String>) {
        let mut rest = s;
        while let Some(start) = rest.find('"') {
            let after = &rest[start + 1..];
            if let Some(end) = after.find('"') {
                out.insert(after[..end].to_string());
                rest = &after[end + 1..];
            } else {
                break;
            }
        }
    }

    #[test]
    fn parses_deny_toml_allow_list_shape() {
        let toml = r#"
[licenses]
allow = [
    "Apache-2.0",
    "MIT",
    "Zlib",
]
[bans]
multiple-versions = "allow"
"#;
        let allow = parse_deny_allow_list(toml);
        let expected: std::collections::BTreeSet<String> =
            ["Apache-2.0", "MIT", "Zlib"].into_iter().map(String::from).collect();
        assert_eq!(allow, expected);
    }

    #[test]
    fn parses_deny_toml_allow_list_inline() {
        let toml = r#"[licenses]
allow = ["MIT", "ISC"]
"#;
        let allow = parse_deny_allow_list(toml);
        let expected: std::collections::BTreeSet<String> = ["MIT", "ISC"].into_iter().map(String::from).collect();
        assert_eq!(allow, expected);
    }

    #[test]
    fn parse_deny_allow_list_ignores_other_sections() {
        let toml = r#"
[bans]
allow = ["not-a-license"]
[licenses]
allow = ["MIT"]
"#;
        let allow = parse_deny_allow_list(toml);
        assert!(!allow.contains("not-a-license"));
        assert!(allow.contains("MIT"));
    }
}
