//! Helpers used by `build.rs` to populate the license manifest with actual
//! license text — either from files shipped in each dependency's source tree,
//! or (when nothing is there) from bundled canonical SPDX text.
//!
//! These helpers live on the library side so they can be exercised by the
//! regular unit-test harness rather than only via `cargo build`. Both
//! `build.rs` and the library compile this module; sharing the code means
//! the producer and the runtime agree on what counts as a license file.

use std::path::Path;

use super::types::LicenseFile;

/// Filename prefixes (case-insensitive) treated as license / attribution /
/// copyright / notice material.
///
/// The set covers both British (`LICENCE`) and American (`LICENSE`) spellings
/// plus the `COPYING` and `COPYRIGHT` conventions common in older projects,
/// and the `AUTHORS` / `NOTICE` files required by a handful of the licenses
/// on the allowlist (Apache-2.0 and MPL-2.0 most prominently).
const LICENSE_FILE_PREFIXES: &[&str] = &["LICENSE", "LICENCE", "COPYING", "NOTICE", "AUTHORS", "COPYRIGHT"];

/// Return `true` if `name` looks like a license/notice filename.
///
/// A filename matches when, after uppercasing, it starts with one of the
/// recognised prefixes (`LICENSE`, `LICENCE`, `COPYING`, `NOTICE`, `AUTHORS`,
/// `COPYRIGHT`) followed by end-of-name, `'.'`, `'-'`, or `'_'`. So
/// `LICENSE`, `LICENSE-MIT`, `LICENSE.md`, `NOTICE_FOR_X` all match, while
/// unrelated names like `LICENSES.md` or `AUTHORSHIP` do not.
pub fn is_license_filename(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    LICENSE_FILE_PREFIXES
        .iter()
        .any(|&prefix| match upper.strip_prefix(prefix) {
            Some(rest) => rest.is_empty() || rest.starts_with('.') || rest.starts_with('-') || rest.starts_with('_'),
            None => false,
        })
}

/// Collect every license/notice file from the top level of `dir`.
///
/// Returns an empty vector on any I/O error, silently: a missing or
/// unreadable directory is indistinguishable from "no license file shipped"
/// for our purposes. The returned entries are sorted by filename so the
/// generated manifest is reproducible regardless of filesystem ordering.
pub fn collect_crate_license_files(dir: &Path) -> Vec<LicenseFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<LicenseFile> = entries
        .flatten()
        .filter_map(|entry| {
            let file_name = entry.file_name().into_string().ok()?;
            if !is_license_filename(&file_name) {
                return None;
            }
            let path = entry.path();
            if !path.is_file() {
                return None;
            }
            let text = std::fs::read_to_string(&path).ok()?;
            Some(LicenseFile { name: file_name, text })
        })
        .collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    files
}

/// Split an SPDX license expression into its component identifiers.
///
/// Accepts common constructors — `A OR B`, `A AND B`, `A WITH X`, parentheses
/// for grouping — and returns a deduplicated, sorted list of the base license
/// identifiers that must be represented in the attribution output.
///
/// The exception after `WITH` is dropped. Its semantics modify the preceding
/// license but do not themselves carry a separate license text; users who
/// need the exception's wording should consult the crate source directly.
pub fn parse_spdx_ids(expression: &str) -> Vec<String> {
    // Normalize so tokenization handles parens, the pre-SPDX `/` "OR"
    // separator (still seen on crates.io in `"MIT/Apache-2.0"` style), and
    // slashes nestled against an identifier.
    let normalized = expression.replace('(', " ( ").replace(')', " ) ").replace('/', " OR ");
    let mut ids: Vec<String> = Vec::new();
    let mut skip_next = false;
    for tok in normalized.split_whitespace() {
        if skip_next {
            skip_next = false;
            continue;
        }
        match tok {
            "OR" | "AND" | "(" | ")" => {}
            "WITH" => {
                skip_next = true;
            }
            id => ids.push(id.to_string()),
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

/// Build a list of fallback [`LicenseFile`] entries for a crate that ships
/// no license file of its own, using bundled canonical SPDX text wherever
/// `lookup` can provide it.
///
/// Callers pass `lookup` as a callback rather than importing the SPDX text
/// table directly, which keeps this function testable in isolation and
/// lets `build.rs` and the library share the same logic even though their
/// `spdx_texts` modules are compiled separately.
pub fn spdx_fallback_files<F>(expression: &str, lookup: F) -> Vec<LicenseFile>
where
    F: Fn(&str) -> Option<&'static str>,
{
    parse_spdx_ids(expression)
        .into_iter()
        .filter_map(|id| {
            lookup(&id).map(|text| LicenseFile {
                name: format!("canonical-{id}.txt"),
                text: text.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_common_license_filenames() {
        for name in [
            "LICENSE",
            "LICENSE-MIT",
            "LICENSE-APACHE",
            "LICENSE.md",
            "LICENSE.txt",
            "LICENCE",
            "LICENCE-MIT",
            "license",
            "license-mit",
            "COPYING",
            "COPYING-GPL",
            "NOTICE",
            "AUTHORS",
            "AUTHORS.md",
            "COPYRIGHT",
            "COPYRIGHT.txt",
        ] {
            assert!(is_license_filename(name), "expected `{name}` to be recognised");
        }
    }

    #[test]
    fn rejects_non_license_filenames() {
        for name in [
            "README",
            "README.md",
            "Cargo.toml",
            "LICENSES.md",
            "AUTHORSHIP.md",
            "src",
            "licensed.rs",
            "mylicense.md",
            "",
        ] {
            assert!(!is_license_filename(name), "expected `{name}` to be rejected");
        }
    }

    #[test]
    fn collects_license_files_from_repo_root() {
        // The gitoxide repo ships `LICENSE-MIT` and `LICENSE-APACHE` at its
        // root — this doubles as both a smoke test and a guard against
        // someone accidentally removing or renaming them.
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let files = collect_crate_license_files(dir);
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"LICENSE-MIT"), "found {names:?}");
        assert!(names.contains(&"LICENSE-APACHE"), "found {names:?}");
        // Each must be non-empty.
        for f in &files {
            assert!(!f.text.trim().is_empty(), "license file `{}` was empty", f.name);
        }
    }

    #[test]
    fn collects_no_files_from_missing_directory() {
        let files = collect_crate_license_files(Path::new("/definitely/not/a/real/directory"));
        assert!(files.is_empty());
    }

    #[test]
    fn parses_single_spdx_id() {
        assert_eq!(parse_spdx_ids("MIT"), vec!["MIT"]);
    }

    #[test]
    fn parses_or_expression() {
        assert_eq!(parse_spdx_ids("MIT OR Apache-2.0"), vec!["Apache-2.0", "MIT"]);
    }

    #[test]
    fn parses_legacy_slash_separator() {
        // Pre-SPDX syntax still appears on crates.io; treat it as `OR`.
        assert_eq!(parse_spdx_ids("MIT/Apache-2.0"), vec!["Apache-2.0", "MIT"]);
        assert_eq!(parse_spdx_ids("Apache-2.0/MIT"), vec!["Apache-2.0", "MIT"]);
        assert_eq!(parse_spdx_ids("Unlicense/MIT"), vec!["MIT", "Unlicense"]);
    }

    #[test]
    fn parses_and_expression() {
        assert_eq!(parse_spdx_ids("MIT AND Apache-2.0"), vec!["Apache-2.0", "MIT"]);
    }

    #[test]
    fn drops_with_exception_tail() {
        assert_eq!(parse_spdx_ids("Apache-2.0 WITH LLVM-exception"), vec!["Apache-2.0"]);
    }

    #[test]
    fn handles_parentheses_and_mixed_operators() {
        assert_eq!(
            parse_spdx_ids("(MIT OR Apache-2.0) AND Unicode-3.0"),
            vec!["Apache-2.0", "MIT", "Unicode-3.0"]
        );
    }

    #[test]
    fn deduplicates_repeated_ids() {
        assert_eq!(parse_spdx_ids("MIT OR MIT"), vec!["MIT"]);
    }

    #[test]
    fn spdx_fallback_populates_files_from_lookup() {
        let files = spdx_fallback_files("MIT OR Apache-2.0", |id| match id {
            "MIT" => Some("MIT CANONICAL TEXT"),
            "Apache-2.0" => Some("APACHE CANONICAL TEXT"),
            _ => None,
        });
        assert_eq!(files.len(), 2);
        let by_name: std::collections::HashMap<_, _> =
            files.iter().map(|f| (f.name.as_str(), f.text.as_str())).collect();
        assert_eq!(by_name.get("canonical-MIT.txt"), Some(&"MIT CANONICAL TEXT"));
        assert_eq!(by_name.get("canonical-Apache-2.0.txt"), Some(&"APACHE CANONICAL TEXT"));
    }

    #[test]
    fn spdx_fallback_skips_unknown_ids() {
        let files = spdx_fallback_files("MIT OR WTFPL", |id| match id {
            "MIT" => Some("MIT TEXT"),
            _ => None,
        });
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "canonical-MIT.txt");
    }

    #[test]
    fn spdx_fallback_returns_empty_for_unparsable_expression() {
        let files = spdx_fallback_files("", |_| Some("never"));
        assert!(files.is_empty());
    }
}
