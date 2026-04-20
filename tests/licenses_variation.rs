//! Verify that feature profiles and target platforms change which third-party
//! crates `cargo metadata` resolves for the `gitoxide` package. `build.rs`
//! uses the same mechanism to decide which crates' licenses end up embedded
//! in the `gix` and `ein` binaries, so if these checks drift the embedded
//! manifest drifts too — silently including deps that aren't actually in
//! the build, or omitting deps that are. We sample a handful of well-known
//! crates that act as discriminators:
//!
//! * `max` vs `max-pure`: the former uses `curl` + `openssl`, the latter
//!   uses `reqwest` + `rustls`. Each must include its own backend and omit
//!   the other's.
//! * `small` vs `max`: the smallest profile must be a strict subset of the
//!   largest.
//! * Target platform: `winapi` / the `windows*` crate family appear only on
//!   Windows; `security-framework` / `core-foundation` appear only on Apple
//!   targets; `nix` appears on Unix targets but not on Windows.
//!
//! The tests invoke `cargo metadata` as a subprocess rather than importing
//! the library so they work regardless of which feature set the test harness
//! itself was built with. On a cold registry cache some probes will trigger
//! fetches; that is acceptable in CI and expected locally.

use std::collections::BTreeSet;
use std::process::Command;

/// Invoke `cargo metadata` for the top-level `gitoxide` package with the
/// given feature set and target platform, returning the set of names of
/// every third-party (non-workspace, sourced) crate it reports that is
/// reachable from `gitoxide` via a non-dev dep edge.
fn third_party_crate_names(features: &[&str], platform: &str) -> BTreeSet<String> {
    let (_, third_party) = reachable_crate_names(features, platform);
    third_party
}

/// Same inputs as above, but returns both reachable workspace members
/// and reachable third-party crates. Reachability excludes dev-only
/// transits so the sets reflect what is actually linked into a binary
/// built with the given feature/platform.
fn reachable_crate_names(features: &[&str], platform: &str) -> (BTreeSet<String>, BTreeSet<String>) {
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
    let workspace_members: BTreeSet<String> = metadata["workspace_members"]
        .as_array()
        .expect("workspace_members is an array")
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    let reachable = reachable_from_gitoxide_via_non_dev(&metadata);

    let mut ws_names = BTreeSet::new();
    let mut third_party_names = BTreeSet::new();
    for p in metadata["packages"].as_array().expect("packages array") {
        let id = p["id"].as_str().unwrap_or("").to_string();
        if !reachable.contains(&id) {
            continue;
        }
        let name = p["name"].as_str().unwrap_or("").to_string();
        if workspace_members.contains(&id) {
            ws_names.insert(name);
        } else if !p["source"].is_null() {
            third_party_names.insert(name);
        }
    }
    (ws_names, third_party_names)
}

/// BFS from the `gitoxide` package through `resolve.nodes`, following
/// every dep edge whose `dep_kinds` is empty or contains at least one
/// kind that is not `"dev"`. Independent from `build.rs` — operates on
/// raw JSON to rule out a whole class of coordinated bugs.
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

    let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
    for node in md["resolve"]["nodes"].as_array().expect("resolve.nodes array") {
        let owner = node["id"].as_str().unwrap_or("").to_string();
        let mut linked = Vec::new();
        for dep in node["deps"].as_array().unwrap_or(&Vec::new()) {
            let Some(target) = dep["pkg"].as_str() else { continue };
            let dep_kinds = dep["dep_kinds"].as_array();
            let has_non_dev_edge = match dep_kinds {
                None => true,
                Some(kinds) if kinds.is_empty() => true,
                Some(kinds) => kinds.iter().any(|dk| dk["kind"].as_str() != Some("dev")),
            };
            if has_non_dev_edge {
                linked.push(target.to_string());
            }
        }
        adjacency.insert(owner, linked);
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

// ---------------------------------------------------------------------------
// Feature-set variation
// ---------------------------------------------------------------------------
//
// These tests pin the platform to `FEATURE_TEST_TARGET` rather than the CI
// host, because platform-specific transitive deps can legitimately differ
// (e.g. on Windows, `curl`'s TLS layer is Schannel, so `openssl-sys` is not
// pulled in by `max`). We want to verify the FEATURE differences and leave
// the platform concerns to the platform-variation tests below.

/// Platform triple used for feature-variation probes. Chosen because its
/// `max` profile resolves to the most-tested canonical HTTP backend
/// (`curl` + `openssl-sys`), giving a stable expected set of discriminators.
const FEATURE_TEST_TARGET: &str = "x86_64-unknown-linux-gnu";

#[test]
fn max_feature_profile_pulls_in_curl_and_openssl_backends() {
    let crates = third_party_crate_names(&["max"], FEATURE_TEST_TARGET);
    for expected in ["curl", "curl-sys", "openssl-sys", "openssl-probe"] {
        assert!(
            crates.contains(expected),
            "`max` profile must include `{expected}` on {FEATURE_TEST_TARGET} \
             (got {} crates)",
            crates.len(),
        );
    }
}

#[test]
fn max_feature_profile_does_not_include_rustls_or_reqwest() {
    let crates = third_party_crate_names(&["max"], FEATURE_TEST_TARGET);
    for forbidden in ["reqwest", "rustls"] {
        assert!(
            !crates.contains(forbidden),
            "`max` profile must not include `{forbidden}` \
             (that backend belongs to `max-pure`)",
        );
    }
}

#[test]
fn max_pure_feature_profile_pulls_in_reqwest_and_rustls() {
    let crates = third_party_crate_names(&["max-pure"], FEATURE_TEST_TARGET);
    for expected in ["reqwest", "rustls", "hyper"] {
        assert!(
            crates.contains(expected),
            "`max-pure` profile must include `{expected}` on {FEATURE_TEST_TARGET} \
             (got {} crates)",
            crates.len(),
        );
    }
}

#[test]
fn max_pure_feature_profile_does_not_include_curl_or_openssl() {
    let crates = third_party_crate_names(&["max-pure"], FEATURE_TEST_TARGET);
    for forbidden in ["curl", "curl-sys", "openssl-sys"] {
        assert!(
            !crates.contains(forbidden),
            "`max-pure` profile must not include `{forbidden}` \
             (that backend belongs to `max`)",
        );
    }
}

#[test]
fn small_profile_is_strict_subset_of_max_profile() {
    let small = third_party_crate_names(&["small"], FEATURE_TEST_TARGET);
    let max = third_party_crate_names(&["max"], FEATURE_TEST_TARGET);
    let in_small_only: Vec<&String> = small.difference(&max).collect();
    assert!(
        in_small_only.is_empty(),
        "`small` should be a subset of `max` on {FEATURE_TEST_TARGET}, \
         but these are only in `small`: {in_small_only:?}",
    );
    assert!(
        small.len() < max.len(),
        "`small` ({}) should have strictly fewer dependencies than `max` ({}) on {FEATURE_TEST_TARGET}",
        small.len(),
        max.len(),
    );
}

// ---------------------------------------------------------------------------
// Platform variation
// ---------------------------------------------------------------------------

const WINDOWS_TARGET: &str = "x86_64-pc-windows-msvc";
const LINUX_TARGET: &str = "x86_64-unknown-linux-gnu";
const APPLE_TARGET: &str = "x86_64-apple-darwin";

#[test]
fn windows_target_includes_winapi_and_windows_family() {
    let crates = third_party_crate_names(&["max-pure"], WINDOWS_TARGET);
    assert!(
        crates.contains("winapi"),
        "Windows target must include `winapi`; got {} total crates",
        crates.len(),
    );
    let has_windows_family = crates.iter().any(|n| n == "windows" || n.starts_with("windows-"));
    assert!(
        has_windows_family,
        "Windows target must include at least one crate from the `windows*` family \
         (e.g. `windows`, `windows-core`, `windows-sys`)",
    );
}

#[test]
fn non_windows_targets_do_not_include_winapi_or_windows_family() {
    for platform in [LINUX_TARGET, APPLE_TARGET] {
        let crates = third_party_crate_names(&["max-pure"], platform);
        assert!(!crates.contains("winapi"), "{platform} must not include `winapi`");
        assert!(
            !crates.contains("winapi-util"),
            "{platform} must not include `winapi-util`",
        );
        let has_windows_family = crates.iter().any(|n| n == "windows" || n.starts_with("windows-"));
        assert!(!has_windows_family, "{platform} must not include any `windows*` crate");
    }
}

#[test]
fn apple_target_includes_security_framework_and_core_foundation() {
    let crates = third_party_crate_names(&["max-pure"], APPLE_TARGET);
    for expected in [
        "security-framework",
        "security-framework-sys",
        "core-foundation",
        "core-foundation-sys",
    ] {
        assert!(
            crates.contains(expected),
            "Apple target must include `{expected}`; got {} total crates",
            crates.len(),
        );
    }
}

#[test]
fn non_apple_targets_do_not_include_security_framework() {
    for platform in [LINUX_TARGET, WINDOWS_TARGET] {
        let crates = third_party_crate_names(&["max-pure"], platform);
        for forbidden in ["security-framework", "security-framework-sys"] {
            assert!(!crates.contains(forbidden), "{platform} must not include `{forbidden}`");
        }
    }
}

#[test]
fn unix_targets_include_xattr_and_windows_does_not() {
    // `xattr` wraps the `getxattr`/`setxattr` syscall family that only
    // exists on Unix. It is pulled into the `gix` / `ein` binary on Linux
    // and macOS through normal (non-dev) edges and is absent on Windows.
    for platform in [LINUX_TARGET, APPLE_TARGET] {
        let crates = third_party_crate_names(&["max-pure"], platform);
        assert!(
            crates.contains("xattr"),
            "Unix target {platform} should include `xattr` (got {} crates)",
            crates.len(),
        );
    }
    let win_crates = third_party_crate_names(&["max-pure"], WINDOWS_TARGET);
    assert!(
        !win_crates.contains("xattr"),
        "Windows target should not include `xattr`",
    );
}

// ---------------------------------------------------------------------------
// Workspace-member variation
// ---------------------------------------------------------------------------
//
// At least one workspace crate IS feature-gated for the `gitoxide`
// binary: `gix-archive` is an optional dep of `gix` behind the
// `worktree-archive` feature, which `small` does not enable but `max`
// does. `max_enables_gix_archive_but_small_does_not` pins this case so
// the feature-aware reachability filter in `build.rs` (and the
// independent implementation in `tests/licenses_parity.rs` and the
// helper here) stays honest about what actually ends up in the binary.
//
// Separately, `dev_only_gix_crates_never_reachable_from_gitoxide`
// asserts the invariant for the `gix-testtools` / `*-tests` crates
// that live only in `[dev-dependencies]`.
//
// The manifest's own author/license filter is tested by
// `tests/licenses_workspace_attribution.rs` and the sentinel tests in
// `src/licenses/embedded.rs`.

#[test]
fn max_enables_gix_archive_but_small_does_not_per_cargo_tree() {
    // `gix-archive` is an optional dep of `gix` gated on the
    // `worktree-archive` feature. `max` enables it (via `gix` default
    // features, transitively); `small` does not. This is a concrete
    // example of feature-gated workspace-crate linkage that the
    // feature-aware reachability filter in `build.rs` (and its
    // independent replica in `tests/licenses_parity.rs`) must respect.
    //
    // Uses `cargo tree -p gitoxide --edges normal --features X` as the
    // source of truth — that output reflects cargo's own resolver,
    // which is the first-principles definition of what is linked.
    fn names_in_tree(features: &str) -> std::collections::BTreeSet<String> {
        let mut cmd = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cmd.args([
            "tree",
            "-p",
            "gitoxide",
            "--prefix",
            "none",
            "--edges",
            "normal",
            "--no-default-features",
            "--features",
            features,
            "--target",
            FEATURE_TEST_TARGET,
        ]);
        cmd.current_dir(env!("CARGO_MANIFEST_DIR"));
        let output = cmd.output().expect("run cargo tree");
        assert!(
            output.status.success(),
            "`cargo tree` failed ({}):\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr),
        );
        let text = String::from_utf8(output.stdout).expect("cargo tree output is UTF-8");
        let mut set = std::collections::BTreeSet::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Lines look like `crate-name vX.Y.Z (path-or-source) (*)`.
            // The first whitespace-delimited token is the crate name.
            if let Some(name) = line.split_whitespace().next() {
                set.insert(name.to_string());
            }
        }
        set
    }

    let max_tree = names_in_tree("max");
    let small_tree = names_in_tree("small");
    assert!(
        max_tree.contains("gix-archive"),
        "`max` profile should pull in `gix-archive` per cargo tree (got {} crates)",
        max_tree.len(),
    );
    assert!(
        !small_tree.contains("gix-archive"),
        "`small` profile should NOT pull in `gix-archive` per cargo tree (got {} crates)",
        small_tree.len(),
    );
}

#[test]
fn dev_only_gix_crates_never_reachable_from_gitoxide() {
    // `gix-testtools` and the `*-tests` crates exist only as
    // dev-dependencies of other workspace members. They are not linked
    // into any build of `gix` / `ein`, so they must never show up in the
    // reachable workspace-member set, regardless of feature or platform.
    let dev_only = ["gix-testtools", "gix-config-tests", "gix-status-tests"];
    for feat in ["max", "max-pure", "lean", "small", "lean-async"] {
        for platform in [LINUX_TARGET, WINDOWS_TARGET, APPLE_TARGET] {
            let (ws, _) = reachable_crate_names(&[feat], platform);
            for name in dev_only {
                assert!(
                    !ws.contains(name),
                    "dev-only workspace crate `{name}` must not be reachable from `gitoxide` \
                     with features={feat} platform={platform} (got {} reachable workspace members)",
                    ws.len(),
                );
            }
        }
    }
}
