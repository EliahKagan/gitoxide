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

/// Return `true` if a package's license or authorship differs from the
/// `root` package's and so requires its own attribution entry in the manifest.
///
/// The license comparison normalises SPDX expressions (so `MIT OR Apache-2.0`
/// and `Apache-2.0 OR MIT` are treated as equivalent) via [`parse_spdx_ids`],
/// and the author comparison is order-independent.
///
/// Takes plain types (`Option<&str>` for the SPDX expression, `&[String]` for
/// authors) so it can live on the shared library side and be reused by
/// `build.rs` without `build.rs` forcing `cargo_metadata` into the library's
/// dependency surface.
pub fn needs_separate_attribution(
    pkg_license: Option<&str>,
    pkg_authors: &[String],
    root_license: Option<&str>,
    root_authors: &[String],
) -> bool {
    match (pkg_license, root_license) {
        (Some(pkg_lic), Some(root_lic)) => {
            if parse_spdx_ids(pkg_lic) != parse_spdx_ids(root_lic) {
                return true;
            }
        }
        (None, None) => {}
        // One declares a license, the other doesn't — different enough to
        // require its own entry.
        _ => return true,
    }
    let pkg_set: std::collections::HashSet<&str> = pkg_authors.iter().map(String::as_str).collect();
    let root_set: std::collections::HashSet<&str> = root_authors.iter().map(String::as_str).collect();
    pkg_set != root_set
}

/// Return `true` if the cargo feature-activator string `activator` would
/// activate the optional dep whose owner-side alias is `alias_name` and
/// whose underlying package name is `raw_dep_name`.
///
/// Activator syntaxes, from cargo's features reference:
///
/// * `"dep:foo"` — explicit activation of the optional dep `foo` (modern).
/// * `"foo"` — in pre-2021-edition crates that don't use the `dep:`
///   syntax anywhere, this also activates optional dep `foo` (legacy
///   implicit-feature behaviour).
/// * `"foo/bar"` — activates the optional dep `foo` *and* sets feature
///   `bar` on it. Strong activation.
/// * `"foo?/bar"` — *weak* activation: sets feature `bar` on `foo` only
///   if `foo` is otherwise enabled. Does not by itself activate `foo`.
///
/// The function matches against either the alias name or the raw package
/// name so a renamed dep (e.g.
/// `foo-alias = { package = "foo", optional = true }`) is handled whether
/// the activator refers to it as `foo-alias` or (less conventionally) as
/// `foo`. This is conservatively inclusive — license attribution is
/// safer when slightly over-inclusive than when a linked crate is
/// omitted.
pub fn activator_enables(activator: &str, alias_name: &str, raw_dep_name: &str) -> bool {
    let matches = |n: &str| n == alias_name || n == raw_dep_name;
    if let Some(rest) = activator.strip_prefix("dep:") {
        return matches(rest);
    }
    if activator.contains("?/") {
        return false;
    }
    if let Some((name, _)) = activator.split_once('/') {
        return matches(name);
    }
    matches(activator)
}

/// Return `true` if any activator reachable from `owner_enabled_features`
/// through `owner_features_table` enables the optional dep whose alias
/// is `alias_name` and whose package name is `raw_dep_name`.
///
/// Also honours cargo's "implicit feature named after the optional dep":
/// if `owner_enabled_features` itself contains the alias, the dep is
/// considered active.
///
/// `owner_features_table` is the `features` map from the owner package
/// as reported by cargo metadata — i.e. feature name → list of activator
/// strings. `owner_enabled_features` is the expanded set of feature
/// names cargo has resolved as active on the owner under the current
/// feature selection. Taking these two inputs verbatim keeps this
/// function a pure transformation: the cargo resolver remains the
/// source of truth for which features are active.
pub fn any_activator_enables_dep(
    owner_features_table: &std::collections::BTreeMap<String, Vec<String>>,
    owner_enabled_features: &[String],
    alias_name: &str,
    raw_dep_name: &str,
) -> bool {
    if owner_enabled_features.iter().any(|f| f == alias_name) {
        return true;
    }
    for f in owner_enabled_features {
        let Some(activators) = owner_features_table.get(f) else {
            continue;
        };
        for a in activators {
            if activator_enables(a, alias_name, raw_dep_name) {
                return true;
            }
        }
    }
    false
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

    // ----- needs_separate_attribution -----

    fn author(name: &str) -> Vec<String> {
        vec![name.to_string()]
    }

    #[test]
    fn same_license_and_same_authors_does_not_need_separate_attribution() {
        assert!(!needs_separate_attribution(
            Some("MIT OR Apache-2.0"),
            &author("Alice"),
            Some("MIT OR Apache-2.0"),
            &author("Alice"),
        ));
    }

    #[test]
    fn spdx_order_is_ignored_for_license_equality() {
        // `A OR B` and `B OR A` must be treated as equivalent.
        assert!(!needs_separate_attribution(
            Some("Apache-2.0 OR MIT"),
            &author("Alice"),
            Some("MIT OR Apache-2.0"),
            &author("Alice"),
        ));
    }

    #[test]
    fn different_license_requires_separate_attribution() {
        assert!(needs_separate_attribution(
            Some("Apache-2.0"),
            &author("Alice"),
            Some("MIT OR Apache-2.0"),
            &author("Alice"),
        ));
    }

    #[test]
    fn different_authors_require_separate_attribution() {
        assert!(needs_separate_attribution(
            Some("MIT OR Apache-2.0"),
            &author("Bob"),
            Some("MIT OR Apache-2.0"),
            &author("Alice"),
        ));
    }

    #[test]
    fn author_set_order_is_ignored() {
        let pkg = vec!["Alice".to_string(), "Bob".to_string()];
        let root = vec!["Bob".to_string(), "Alice".to_string()];
        assert!(!needs_separate_attribution(Some("MIT"), &pkg, Some("MIT"), &root));
    }

    #[test]
    fn extra_author_requires_separate_attribution() {
        let pkg = vec!["Alice".to_string(), "Bob".to_string()];
        let root = vec!["Alice".to_string()];
        assert!(needs_separate_attribution(Some("MIT"), &pkg, Some("MIT"), &root));
    }

    #[test]
    fn one_side_missing_license_requires_separate_attribution() {
        assert!(needs_separate_attribution(
            None,
            &author("Alice"),
            Some("MIT"),
            &author("Alice"),
        ));
        assert!(needs_separate_attribution(
            Some("MIT"),
            &author("Alice"),
            None,
            &author("Alice"),
        ));
    }

    #[test]
    fn both_missing_license_falls_back_to_author_comparison() {
        assert!(!needs_separate_attribution(
            None,
            &author("Alice"),
            None,
            &author("Alice"),
        ));
        assert!(needs_separate_attribution(None, &author("Alice"), None, &author("Bob"),));
    }

    #[test]
    fn both_empty_author_lists_are_equal() {
        assert!(!needs_separate_attribution(Some("MIT"), &[], Some("MIT"), &[]));
    }

    // ----- activator_enables -----

    #[test]
    fn dep_prefix_activates_exact_alias() {
        assert!(activator_enables("dep:foo", "foo", "foo"));
    }

    #[test]
    fn dep_prefix_activates_renamed_alias() {
        // `foo-alias = { package = "foo", optional = true }`, activator
        // referencing the alias.
        assert!(activator_enables("dep:foo-alias", "foo-alias", "foo"));
    }

    #[test]
    fn dep_prefix_also_activates_by_raw_name_conservatively() {
        // Cargo semantics say activators reference aliases, but matching
        // on the raw name too keeps us on the safe side of over-inclusion
        // rather than silently dropping a crate that is in fact linked.
        assert!(activator_enables("dep:foo", "foo-alias", "foo"));
    }

    #[test]
    fn dep_prefix_does_not_activate_other_deps() {
        assert!(!activator_enables("dep:bar", "foo", "foo"));
    }

    #[test]
    fn bare_name_activates_optional_dep_legacy_style() {
        // Legacy syntax still widely in use — e.g. gix's
        // `worktree-archive = ["gix-archive", ...]`.
        assert!(activator_enables("foo", "foo", "foo"));
    }

    #[test]
    fn bare_name_rejects_non_matching() {
        assert!(!activator_enables("bar", "foo", "foo"));
    }

    #[test]
    fn strong_slash_activates_the_named_dep() {
        // `"foo/bar"` enables the dep `foo` *and* its feature `bar`.
        assert!(activator_enables("foo/bar", "foo", "foo"));
    }

    #[test]
    fn strong_slash_requires_name_match() {
        assert!(!activator_enables("other/bar", "foo", "foo"));
    }

    #[test]
    fn weak_activator_does_not_enable_by_itself() {
        // `"foo?/bar"` is weak: it only sets `bar` on `foo` if `foo` is
        // already enabled by some other means. On its own it does NOT
        // activate `foo`.
        assert!(!activator_enables("foo?/bar", "foo", "foo"));
    }

    #[test]
    fn weak_activator_does_not_enable_even_with_dep_prefix() {
        // `"dep:foo?/bar"` is not a real activator form, but make sure
        // the weak-marker substring takes precedence over the prefix.
        assert!(!activator_enables("dep:foo?/bar", "foo", "foo"));
    }

    // ----- any_activator_enables_dep -----

    fn feature_table(pairs: &[(&str, &[&str])]) -> std::collections::BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, vs)| ((*k).to_string(), vs.iter().map(|s| (*s).to_string()).collect()))
            .collect()
    }

    #[test]
    fn implicit_dep_named_feature_activates_dep() {
        // Cargo 2021+: optional dep `foo` gets an implicit feature
        // `foo`. If that feature is in the owner's enabled set, the
        // dep is active.
        let enabled = vec!["foo".to_string()];
        let table = feature_table(&[]);
        assert!(any_activator_enables_dep(&table, &enabled, "foo", "foo"));
    }

    #[test]
    fn activator_via_enabled_feature_activates_dep() {
        // Owner feature `extras` activates a dep via `dep:foo`;
        // `extras` is enabled, so the dep is active.
        let enabled = vec!["extras".to_string()];
        let table = feature_table(&[("extras", &["dep:foo"])]);
        assert!(any_activator_enables_dep(&table, &enabled, "foo", "foo"));
    }

    #[test]
    fn inactive_feature_does_not_activate_dep() {
        // `extras` is NOT enabled; the dep stays inactive.
        let enabled: Vec<String> = Vec::new();
        let table = feature_table(&[("extras", &["dep:foo"])]);
        assert!(!any_activator_enables_dep(&table, &enabled, "foo", "foo"));
    }

    #[test]
    fn weak_only_activation_does_not_enable_dep() {
        // The only path from an enabled feature to the dep is via a
        // weak activator; the dep stays inactive.
        let enabled = vec!["sha1".to_string()];
        let table = feature_table(&[("sha1", &["foo?/sha1"])]);
        assert!(!any_activator_enables_dep(&table, &enabled, "foo", "foo"));
    }

    #[test]
    fn strong_slash_activates_even_through_an_active_feature() {
        // `sha1 = ["foo/feat-bar"]` activates the dep `foo` AND sets
        // its `feat-bar` feature. `sha1` is enabled → dep is active.
        let enabled = vec!["sha1".to_string()];
        let table = feature_table(&[("sha1", &["foo/feat-bar"])]);
        assert!(any_activator_enables_dep(&table, &enabled, "foo", "foo"));
    }

    #[test]
    fn renamed_dep_activator_respects_alias() {
        // `archive = ["dep:gix-archive-for-configuration-only"]`
        // activating the alias for the renamed `gix-archive` dep.
        let enabled = vec!["archive".to_string()];
        let table = feature_table(&[("archive", &["dep:gix-archive-for-configuration-only"])]);
        assert!(any_activator_enables_dep(
            &table,
            &enabled,
            "gix-archive-for-configuration-only",
            "gix-archive",
        ));
    }

    #[test]
    fn activator_for_other_dep_does_not_false_positive() {
        let enabled = vec!["extras".to_string()];
        let table = feature_table(&[("extras", &["dep:unrelated"])]);
        assert!(!any_activator_enables_dep(&table, &enabled, "foo", "foo"));
    }

    #[test]
    fn feature_table_missing_entry_is_not_a_panic() {
        // An enabled-feature name that isn't in the table at all (can
        // happen for workspace inheritance / implicit features) must
        // not panic — just contribute no activation.
        let enabled = vec!["phantom".to_string()];
        let table = feature_table(&[]);
        assert!(!any_activator_enables_dep(&table, &enabled, "foo", "foo"));
    }
}
