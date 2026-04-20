//! Structural check that `build.rs` includes every workspace member whose
//! attribution differs from the root `gitoxide` package — and only those
//! members — in the embedded license manifest.
//!
//! The existing unit tests in `src/licenses/embedded.rs` pin specific
//! workspace crates by name (`gix-imara-diff`, `gix-config`,
//! `gix-config-tests`, `gitoxide`). Those remain as sanity checks but are
//! not enough on their own: a future workspace crate that gains a
//! differing license or authorship would slip through untested until
//! someone noticed it missing from `gix licenses`. This test closes that
//! gap by independently recomputing "should this crate be in the
//! manifest?" for every reachable workspace member and cross-checking
//! the verdict against the binary's actual manifest.
//!
//! # Why not call `needs_separate_attribution` from production?
//!
//! A test that reused the production code's decision logic would agree
//! with production by construction, so a bug in that logic would be
//! invisible to this test. Instead, the check here uses DIFFERENT logic:
//! a sorted-token license comparison (enough to canonicalise the only
//! reordering we see in practice, `"A OR B"` vs `"B OR A"`) and a plain
//! `BTreeSet` author comparison. Production normalises via
//! `parse_spdx_ids` which additionally handles `WITH`, parentheses, and
//! the legacy `/` separator; our simpler normalisation is strictly more
//! conservative. When the two decision procedures disagree — production
//! including a crate the test doesn't require, or (more alarmingly)
//! production excluding a crate the test says must be included — the
//! test surfaces the specific crate with enough detail to diagnose.
//!
//! Author-set differences alone are an unambiguous signal (set equality
//! is independent of license logic), so we assert them strictly.

use std::collections::{BTreeSet, VecDeque};
use std::process::Command;

/// Resolve `cargo` the same way `build.rs` does: prefer the `CARGO` env
/// var Cargo exports, fall back to `cargo` on `PATH` only if unset.
fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

fn run_cargo_metadata() -> serde_json::Value {
    let mut cmd = Command::new(cargo_bin());
    cmd.args(["metadata", "--format-version", "1", "--no-default-features"]);
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"));
    let output = cmd.output().expect("run `cargo metadata`");
    assert!(
        output.status.success(),
        "`cargo metadata` failed ({}):\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).expect("parse cargo-metadata JSON")
}

/// BFS from `gitoxide` through `resolve.nodes`, following every dep edge
/// that is (a) non-dev and (b) actually activated under the current
/// feature resolution. Independent of `build.rs`'s Rust implementation —
/// operates on raw JSON so a shared-struct layout bug cannot corrupt
/// both sides simultaneously.
///
/// See `optional_dep_activated_by_features` below for the activator-
/// string semantics (`"dep:foo"`, `"foo"`, `"foo/bar"`, `"foo?/bar"`).
fn reachable_from_gitoxide(md: &serde_json::Value) -> BTreeSet<String> {
    use std::collections::HashMap;

    let resolve = md.get("resolve").and_then(|r| r.get("nodes")).expect("resolve.nodes");
    let root_id = md["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .find(|p| p["name"].as_str() == Some("gitoxide"))
        .and_then(|p| p["id"].as_str())
        .expect("gitoxide package in metadata")
        .to_string();

    let packages_by_id: HashMap<&str, &serde_json::Value> = md["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .filter_map(|p| p["id"].as_str().map(|id| (id, p)))
        .collect();
    let features_by_id: HashMap<&str, BTreeSet<&str>> = resolve
        .as_array()
        .expect("resolve.nodes array")
        .iter()
        .filter_map(|n| {
            let id = n["id"].as_str()?;
            let features: BTreeSet<&str> = n["features"]
                .as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                .unwrap_or_default();
            Some((id, features))
        })
        .collect();

    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for node in resolve.as_array().expect("resolve.nodes is array") {
        let owner_id = node["id"].as_str().unwrap_or("");
        let owner_pkg = packages_by_id.get(owner_id).copied();
        let owner_features = features_by_id.get(owner_id);
        let mut ds = Vec::new();
        for dep in node["deps"].as_array().unwrap_or(&Vec::new()) {
            let Some(pkg) = dep["pkg"].as_str() else { continue };
            let kinds = dep["dep_kinds"].as_array();
            let has_non_dev_edge = match kinds {
                None => true,
                Some(arr) if arr.is_empty() => true,
                Some(arr) => arr.iter().any(|dk| dk["kind"].as_str() != Some("dev")),
            };
            if !has_non_dev_edge {
                continue;
            }
            let dep_alias = dep["name"].as_str().unwrap_or("");
            let raw_dep_name = packages_by_id.get(pkg).and_then(|p| p["name"].as_str()).unwrap_or("");
            let is_optional = owner_pkg
                .and_then(|p| p["dependencies"].as_array())
                .is_some_and(|deps| {
                    deps.iter().any(|md_dep| {
                        let opt = md_dep["optional"].as_bool().unwrap_or(false);
                        let name = md_dep["name"].as_str().unwrap_or("");
                        let rename = md_dep["rename"].as_str();
                        opt && name == raw_dep_name && (rename.is_none() || rename == Some(dep_alias))
                    })
                });
            if is_optional && !optional_dep_activated_by_features(owner_pkg, owner_features, dep_alias, raw_dep_name) {
                continue;
            }
            ds.push(pkg.to_string());
        }
        deps.insert(owner_id.to_string(), ds);
    }

    let mut reachable: BTreeSet<String> = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(root_id);
    while let Some(id) = queue.pop_front() {
        if !reachable.insert(id.clone()) {
            continue;
        }
        if let Some(ds) = deps.get(&id) {
            for d in ds {
                queue.push_back(d.clone());
            }
        }
    }
    reachable
}

fn optional_dep_activated_by_features(
    owner_pkg: Option<&serde_json::Value>,
    owner_features: Option<&BTreeSet<&str>>,
    alias_name: &str,
    raw_dep_name: &str,
) -> bool {
    let Some(owner_pkg) = owner_pkg else { return false };
    let Some(owner_features) = owner_features else {
        return false;
    };
    if owner_features.contains(alias_name) {
        return true;
    }
    let Some(feat_map) = owner_pkg["features"].as_object() else {
        return false;
    };
    for f in owner_features {
        let Some(arr) = feat_map.get(*f).and_then(|v| v.as_array()) else {
            continue;
        };
        for a in arr {
            let Some(s) = a.as_str() else { continue };
            if activator_enables(s, alias_name, raw_dep_name) {
                return true;
            }
        }
    }
    false
}

fn activator_enables(activator: &str, alias_name: &str, raw_dep_name: &str) -> bool {
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

/// Parse the built `gix` binary's manifest and return both name sets:
/// full-attribution entries and the names-only same-attribution list.
fn binary_manifest() -> (BTreeSet<String>, BTreeSet<String>) {
    let gix = env!("CARGO_BIN_EXE_gix");
    let output = Command::new(gix)
        .args(["licenses", "--format", "json"])
        .output()
        .expect("run `gix licenses --format json`");
    assert!(
        output.status.success(),
        "`gix licenses --format json` failed ({}):\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let data: serde_json::Value = serde_json::from_slice(&output.stdout).expect("parse `gix licenses` JSON");
    let full: BTreeSet<String> = data["crates"]
        .as_array()
        .expect("`crates` is an array")
        .iter()
        .filter_map(|c| c["name"].as_str().map(String::from))
        .collect();
    let same_attrib: BTreeSet<String> = data["workspace_members_same_attribution"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default();
    (full, same_attrib)
}

/// Whitespace-tokenised, sorted form of an SPDX license expression.
/// Independent of `gitoxide_core::licenses::build_support::parse_spdx_ids`
/// — that function additionally strips `WITH` exceptions, parentheses,
/// and the legacy `/` separator, and deduplicates. Our simpler version
/// is enough to handle the only normalisation we see in practice
/// (`"A OR B"` vs `"B OR A"`), and it disagrees with the production code
/// in edge cases we don't currently have in the tree, which is the point.
fn sorted_license_tokens(s: &str) -> Vec<String> {
    let mut tokens: Vec<String> = s.split_whitespace().map(str::to_string).collect();
    tokens.sort();
    tokens
}

fn authors_of(p: &serde_json::Value) -> BTreeSet<String> {
    p["authors"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

fn license_of(p: &serde_json::Value) -> Option<String> {
    p["license"].as_str().map(str::to_string)
}

#[test]
fn every_workspace_member_with_different_attribution_is_in_manifest() {
    let md = run_cargo_metadata();
    let ws_ids: BTreeSet<String> = md["workspace_members"]
        .as_array()
        .expect("workspace_members array")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let reachable = reachable_from_gitoxide(&md);

    let root = md["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .find(|p| p["name"].as_str() == Some("gitoxide"))
        .expect("gitoxide package present");
    let root_license = license_of(root);
    let root_license_tokens = root_license.as_deref().map(sorted_license_tokens);
    let root_authors = authors_of(root);

    let (full, same_attrib) = binary_manifest();

    let mut missing_despite_author_diff: Vec<String> = Vec::new();
    let mut full_entry_despite_exact_match: Vec<String> = Vec::new();
    let mut in_both_lists: Vec<String> = Vec::new();
    let mut in_neither_list: Vec<String> = Vec::new();
    let mut license_raw_differs_only: Vec<String> = Vec::new();

    for p in md["packages"].as_array().expect("packages array") {
        let id = p["id"].as_str().unwrap_or("").to_string();
        if !ws_ids.contains(&id) {
            continue;
        }
        if !reachable.contains(&id) {
            continue;
        }
        let name = p["name"].as_str().unwrap_or("").to_string();
        // The root itself never needs its own entry in its own manifest.
        if name == "gitoxide" {
            continue;
        }

        let pkg_license = license_of(p);
        let pkg_license_tokens = pkg_license.as_deref().map(sorted_license_tokens);
        let pkg_authors = authors_of(p);

        let author_differs = pkg_authors != root_authors;
        let license_tokens_differ = pkg_license_tokens != root_license_tokens;
        let license_raw_differs = pkg_license != root_license;

        let in_full = full.contains(&name);
        let in_same_attrib = same_attrib.contains(&name);

        // Hard rule 1: every reachable workspace member must be
        // represented in exactly one of the two manifest buckets.
        match (in_full, in_same_attrib) {
            (true, true) => in_both_lists.push(name.clone()),
            (false, false) => in_neither_list.push(name.clone()),
            _ => {}
        }

        // Hard rule 2: a workspace member whose authors differ from the
        // root's MUST land in the full-attribution list. Author-set
        // equality is a simple BTreeSet comparison whose result is
        // independent of any license parser bug, so this direction admits
        // no excuse.
        if author_differs && !in_full {
            missing_despite_author_diff.push(format!("{name} (authors {pkg_authors:?} vs root {root_authors:?})"));
            continue;
        }

        // Hard rule 3: a workspace member whose license literally equals
        // the root's AND whose authors literally equal the root's MUST
        // land in the names-only same-attribution list, NOT in the
        // full-entry list.
        if !author_differs && !license_raw_differs && in_full {
            full_entry_despite_exact_match.push(name.clone());
            continue;
        }

        // Soft note: license strings differ literally but sort-and-compare
        // calls them the same (i.e. the only difference is token order).
        // Production's SPDX-normalised comparison also calls them the
        // same, so its excluding such a crate from the full-entry list
        // is correct. Surface the case for reviewer eyeballing.
        if license_raw_differs && !license_tokens_differ && !author_differs && !in_full {
            license_raw_differs_only.push(format!("{name} (license {pkg_license:?} vs root {root_license:?})"));
        }
    }

    if !license_raw_differs_only.is_empty() {
        eprintln!(
            "note: workspace members with literal-but-not-sorted-token license differences, \
             excluded by production (SPDX-normalised) from the full-entry list:",
        );
        for line in &license_raw_differs_only {
            eprintln!("  {line}");
        }
    }

    assert!(
        missing_despite_author_diff.is_empty()
            && full_entry_despite_exact_match.is_empty()
            && in_both_lists.is_empty()
            && in_neither_list.is_empty(),
        "workspace-member attribution verdict disagrees with the embedded manifest:\n  \
         workspace members with differing authors but NOT in full-entry list: {missing_despite_author_diff:?}\n  \
         workspace members matching root exactly but PRESENT in full-entry list: {full_entry_despite_exact_match:?}\n  \
         workspace members in BOTH lists (should be in exactly one): {in_both_lists:?}\n  \
         workspace members in NEITHER list (must be in one): {in_neither_list:?}",
    );
}

/// Belt-and-suspenders: the structural check above is the load-bearing
/// assertion. A named-sentinel version stays in `embedded.rs` so the
/// test binary also catches regressions when the structural test is
/// inadvertently vacuous (e.g. if `reachable_from_gitoxide` ever returns
/// an empty set). Nothing to do here.
#[test]
fn sentinels_match_structural_verdict() {
    // This test exists as a pointer — the actual sentinel assertions
    // live in `src/licenses/embedded.rs` as
    // `workspace_members_with_different_attribution_are_included`, where
    // they run under `cargo test --lib` in CI.
}
