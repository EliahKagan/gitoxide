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

/// Host triple as reported by the `rustc` that cargo would use.
fn host_target() -> String {
    let out = Command::new("rustc").arg("-vV").output().expect("run `rustc -vV`");
    let text = String::from_utf8(out.stdout).expect("rustc output is UTF-8");
    match text.lines().find_map(|l| l.strip_prefix("host: ")) {
        Some(host) => host.trim().to_string(),
        None => panic!("no `host:` line in rustc -vV output:\n{text}"),
    }
}

/// Invoke `cargo metadata` for the top-level `gitoxide` package with the
/// given feature set and target platform, returning the set of names of
/// every third-party (non-workspace, sourced) crate it reports.
fn third_party_crate_names(features: &[&str], platform: &str) -> BTreeSet<String> {
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
    metadata["packages"]
        .as_array()
        .expect("packages is an array")
        .iter()
        .filter(|p| {
            let id = p["id"].as_str().unwrap_or("");
            !workspace_members.contains(id) && !p["source"].is_null()
        })
        .filter_map(|p| p["name"].as_str().map(String::from))
        .collect()
}

// ---------------------------------------------------------------------------
// Feature-set variation
// ---------------------------------------------------------------------------

#[test]
fn max_feature_profile_pulls_in_curl_and_openssl_backends() {
    let host = host_target();
    let crates = third_party_crate_names(&["max"], &host);
    for expected in ["curl", "curl-sys", "openssl-sys", "openssl-probe"] {
        assert!(
            crates.contains(expected),
            "`max` profile must include `{expected}` on {host} \
             (got {} crates)",
            crates.len(),
        );
    }
}

#[test]
fn max_feature_profile_does_not_include_rustls_or_reqwest() {
    let host = host_target();
    let crates = third_party_crate_names(&["max"], &host);
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
    let host = host_target();
    let crates = third_party_crate_names(&["max-pure"], &host);
    for expected in ["reqwest", "rustls", "hyper"] {
        assert!(
            crates.contains(expected),
            "`max-pure` profile must include `{expected}` on {host} \
             (got {} crates)",
            crates.len(),
        );
    }
}

#[test]
fn max_pure_feature_profile_does_not_include_curl_or_openssl() {
    let host = host_target();
    let crates = third_party_crate_names(&["max-pure"], &host);
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
    let host = host_target();
    let small = third_party_crate_names(&["small"], &host);
    let max = third_party_crate_names(&["max"], &host);
    let in_small_only: Vec<&String> = small.difference(&max).collect();
    assert!(
        in_small_only.is_empty(),
        "`small` should be a subset of `max` on {host}, but these are only in `small`: {in_small_only:?}",
    );
    assert!(
        small.len() < max.len(),
        "`small` ({}) should have strictly fewer dependencies than `max` ({}) on {host}",
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
        assert!(!crates.contains("winapi"), "{platform} must not include `winapi`",);
        assert!(
            !crates.contains("winapi-util"),
            "{platform} must not include `winapi-util`",
        );
        let has_windows_family = crates.iter().any(|n| n == "windows" || n.starts_with("windows-"));
        assert!(!has_windows_family, "{platform} must not include any `windows*` crate",);
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
            assert!(!crates.contains(forbidden), "{platform} must not include `{forbidden}`",);
        }
    }
}

#[test]
fn unix_targets_include_nix_and_windows_does_not() {
    // `nix` is a thin wrapper over Unix syscalls; it's pulled in on Linux
    // and macOS but not on Windows.
    for platform in [LINUX_TARGET, APPLE_TARGET] {
        let crates = third_party_crate_names(&["max-pure"], platform);
        assert!(crates.contains("nix"), "Unix target {platform} should include `nix`",);
    }
    let win_crates = third_party_crate_names(&["max-pure"], WINDOWS_TARGET);
    assert!(!win_crates.contains("nix"), "Windows target should not include `nix`",);
}
