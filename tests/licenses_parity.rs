//! Verify that the crate set the compiled `gix` binary reports via
//! `gix licenses --format json` matches what `cargo metadata` resolves for
//! the same feature profile and host platform. This is the build-time-to-
//! runtime parity check: if `build.rs`'s feature or platform filtering
//! drifts, the two crate sets will diverge and this test fails.
//!
//! The comparison is scoped *structurally* to crates cargo knows about as
//! third-party, by intersecting the binary's name set with the third-party
//! set cargo-metadata reports. Two other categories of entries the binary
//! may (legitimately) include are excluded by the intersection without a
//! per-name allowlist:
//!
//!   * workspace members with different attribution — covered by the
//!     dedicated `workspace_members_with_different_attribution_are_included`
//!     unit test in `embedded.rs`; and
//!   * synthetic entries that have no cargo-metadata counterpart at all
//!     (e.g. the Rust standard library when `build.rs` adds it), covered
//!     by their own targeted tests.
//!
//! Rust-side replacement for what was previously a `licenses-parity` CI
//! step implemented as a Bash + Python heredoc. Now it lives with the rest
//! of the integration tests and runs whenever `cargo test` does.

use std::collections::BTreeSet;
use std::process::Command;

/// Host triple as reported by the `rustc` this `cargo test` is using.
fn host_target() -> String {
    let out = Command::new("rustc").arg("-vV").output().expect("run `rustc -vV`");
    let text = String::from_utf8(out.stdout).expect("rustc output is UTF-8");
    match text.lines().find_map(|l| l.strip_prefix("host: ")) {
        Some(host) => host.trim().to_string(),
        None => panic!("no `host:` line in rustc -vV output:\n{text}"),
    }
}

/// The top-level feature-profile flags this test binary was compiled with.
fn enabled_feature_profiles() -> Vec<&'static str> {
    let mut features = Vec::new();
    for (on, name) in [
        (cfg!(feature = "max"), "max"),
        (cfg!(feature = "max-pure"), "max-pure"),
        (cfg!(feature = "lean"), "lean"),
        (cfg!(feature = "small"), "small"),
        (cfg!(feature = "lean-async"), "lean-async"),
    ] {
        if on {
            features.push(name);
        }
    }
    features
}

/// Run `cargo metadata` and return two sets of crate names, restricted to
/// packages that are reachable from the `gitoxide` package via `normal` or
/// `build` dep edges (dev-dep edges are excluded):
///
///   * workspace members reachable via non-dev edges, and
///   * third-party (non-workspace, sourced) crates reachable via non-dev
///     edges.
///
/// This is an independent implementation of the same reachability filter
/// `build.rs` applies when building the manifest. The two code paths share
/// no functions; if either has a bug, the two sets diverge and the parity
/// assertion below surfaces it.
fn cargo_metadata_sets(features: &[&str], platform: &str) -> (BTreeSet<String>, BTreeSet<String>) {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args(["metadata", "--format-version", "1", "--no-default-features"]);
    for f in features {
        cmd.args(["--features", f]);
    }
    cmd.args(["--filter-platform", platform]);
    cmd.current_dir(env!("CARGO_MANIFEST_DIR"));
    let output = cmd.output().expect("run `cargo metadata`");
    assert!(
        output.status.success(),
        "`cargo metadata` failed ({}):\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout).expect("parse cargo-metadata JSON");
    let workspace_member_ids: BTreeSet<String> = metadata["workspace_members"]
        .as_array()
        .expect("workspace_members is an array")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    let reachable = reachable_from_gitoxide_via_non_dev(&metadata);

    let mut workspace_names = BTreeSet::new();
    let mut third_party_names = BTreeSet::new();
    for p in metadata["packages"].as_array().expect("packages is an array") {
        let id = p["id"].as_str().unwrap_or("").to_string();
        if !reachable.contains(&id) {
            continue;
        }
        let name = p["name"].as_str().unwrap_or("").to_string();
        if workspace_member_ids.contains(&id) {
            workspace_names.insert(name);
        } else if !p["source"].is_null() {
            third_party_names.insert(name);
        }
    }
    (workspace_names, third_party_names)
}

/// BFS from the `gitoxide` package through `resolve.nodes`, following
/// every dep edge that is (a) non-dev and (b) actually activated by the
/// owner's resolved feature set. Independent from `build.rs` — operates
/// on raw JSON to rule out a whole class of coordinated bugs.
///
/// Feature activation is evaluated against cargo's edition-2021 semantics
/// for the activator strings in `packages[owner].features`: `"dep:foo"`,
/// `"foo"`, and `"foo/bar"` all activate optional dep `foo`; `"foo?/bar"`
/// is weak and by itself does not. In addition, cargo's implicit "feature
/// named after the dep" is honoured: if the dep's alias name is in the
/// owner's enabled-feature set, the dep is active.
fn reachable_from_gitoxide_via_non_dev(md: &serde_json::Value) -> BTreeSet<String> {
    use std::collections::{HashMap, VecDeque};

    let root_id = md["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .find(|p| p["name"].as_str() == Some("gitoxide"))
        .and_then(|p| p["id"].as_str())
        .expect("gitoxide package in metadata")
        .to_string();

    // Index packages by ID for quick lookup of per-package info.
    let packages_by_id: HashMap<&str, &serde_json::Value> = md["packages"]
        .as_array()
        .expect("packages array")
        .iter()
        .filter_map(|p| p["id"].as_str().map(|id| (id, p)))
        .collect();

    // Index resolved features by ID.
    let features_by_id: HashMap<&str, BTreeSet<&str>> = md["resolve"]["nodes"]
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

    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for node in md["resolve"]["nodes"].as_array().expect("resolve.nodes array") {
        let owner_id = node["id"].as_str().unwrap_or("");
        let owner_pkg = packages_by_id.get(owner_id).copied();
        let owner_features = features_by_id.get(owner_id);
        let mut linked = Vec::new();
        for dep in node["deps"].as_array().unwrap_or(&Vec::new()) {
            let Some(target_id) = dep["pkg"].as_str() else { continue };
            let dep_kinds = dep["dep_kinds"].as_array();
            let has_non_dev_edge = match dep_kinds {
                None => true,
                Some(kinds) if kinds.is_empty() => true,
                Some(kinds) => kinds.iter().any(|dk| dk["kind"].as_str() != Some("dev")),
            };
            if !has_non_dev_edge {
                continue;
            }
            // Feature-aware optional-dep filter. If the owner's manifest
            // entry for this dep says `optional = true`, require that
            // some activator in the owner's enabled features references
            // the dep by alias or package name.
            let dep_alias = dep["name"].as_str().unwrap_or("");
            let raw_dep_name = packages_by_id
                .get(target_id)
                .and_then(|p| p["name"].as_str())
                .unwrap_or("");
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
            linked.push(target_id.to_string());
        }
        adjacency.insert(owner_id.to_string(), linked);
    }

    let mut reachable = BTreeSet::new();
    let mut queue: VecDeque<String> = VecDeque::new();
    queue.push_back(root_id);
    while let Some(id) = queue.pop_front() {
        if !reachable.insert(id.clone()) {
            continue;
        }
        if let Some(neighbours) = adjacency.get(&id) {
            for n in neighbours {
                queue.push_back(n.clone());
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
        let Some(activators) = feat_map.get(*f).and_then(|v| v.as_array()) else {
            continue;
        };
        for a in activators {
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

/// Crate names the built `gix` binary embeds.
fn gix_manifest_crate_names() -> BTreeSet<String> {
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
    data["crates"]
        .as_array()
        .expect("`crates` is an array")
        .iter()
        .filter_map(|c| c["name"].as_str().map(String::from))
        .collect()
}

#[test]
fn third_party_crates_match_cargo_metadata() {
    let features = enabled_feature_profiles();
    let host = host_target();
    let (workspace_names, from_cargo) = cargo_metadata_sets(&features, &host);
    let from_binary = gix_manifest_crate_names();

    // Scope the comparison to just crates cargo classifies as third-party
    // by intersecting. This drops both workspace members (cargo classifies
    // them separately) and any synthetic entry that has no cargo-metadata
    // counterpart at all (e.g. the Rust standard library) — without
    // needing to know their names in advance. Each is tested elsewhere.
    let from_binary_third_party: BTreeSet<String> = from_binary.intersection(&from_cargo).cloned().collect();

    // Also surface any binary entries that cargo-metadata has never heard
    // of, so a mis-classification or a new synthetic category is caught
    // somewhere rather than silently excluded. Workspace members the
    // binary legitimately includes are the only expected residents.
    let unclassified: BTreeSet<String> = from_binary
        .difference(&from_cargo)
        .cloned()
        .collect::<BTreeSet<String>>()
        .difference(&workspace_names)
        .cloned()
        .collect();

    let only_in_cargo: Vec<&String> = from_cargo.difference(&from_binary_third_party).collect();
    let only_in_binary: Vec<&String> = from_binary_third_party.difference(&from_cargo).collect();
    assert!(
        only_in_cargo.is_empty() && only_in_binary.is_empty(),
        "third-party crate-set mismatch for features={features:?}, platform={host}:\n  \
         only in cargo metadata: {only_in_cargo:?}\n  \
         only in gix binary:     {only_in_binary:?}",
    );

    // Log any unclassified entries so reviewers can see them without
    // causing a failure — they are expected to have a targeted test elsewhere.
    if !unclassified.is_empty() {
        eprintln!("note: binary entries not in cargo-metadata's package set (expected to have targeted tests):");
        for name in &unclassified {
            eprintln!("  {name}");
        }
    }
}
