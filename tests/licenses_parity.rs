//! Verify that the crate set the compiled `gix` binary reports via
//! `gix licenses --format json` matches what `cargo metadata` resolves for
//! the same feature profile and host platform. This is the build-time-to-
//! runtime parity check: if `build.rs`'s feature or platform filtering
//! drifts, the two crate sets will diverge and this test fails.
//!
//! The comparison is scoped to *third-party* (non-workspace) crates.
//! Workspace members with different attribution are also included in the
//! binary's manifest, but that behaviour is tested separately by the
//! `workspace_members_with_different_attribution_are_included` unit test
//! in `embedded.rs` — mixing them into this parity comparison would
//! require duplicating `build.rs`'s attribution-diffing logic in the test.
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

/// Run `cargo metadata` and return the names of workspace members and
/// third-party (non-workspace, sourced) crates as two separate sets.
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

    let mut workspace_names = BTreeSet::new();
    let mut third_party_names = BTreeSet::new();
    for p in metadata["packages"].as_array().expect("packages is an array") {
        let id = p["id"].as_str().unwrap_or("");
        let name = p["name"].as_str().unwrap_or("").to_string();
        if workspace_member_ids.contains(id) {
            workspace_names.insert(name);
        } else if !p["source"].is_null() {
            third_party_names.insert(name);
        }
    }
    (workspace_names, third_party_names)
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

    // Filter the binary's output to only third-party crates for this
    // comparison. Workspace members with different attribution are also
    // in the binary's manifest, but they're tested separately.
    let from_binary_third_party: BTreeSet<String> = from_binary.difference(&workspace_names).cloned().collect();

    let only_in_cargo: Vec<&String> = from_cargo.difference(&from_binary_third_party).collect();
    let only_in_binary: Vec<&String> = from_binary_third_party.difference(&from_cargo).collect();
    assert!(
        only_in_cargo.is_empty() && only_in_binary.is_empty(),
        "third-party crate-set mismatch for features={features:?}, platform={host}:\n  \
         only in cargo metadata: {only_in_cargo:?}\n  \
         only in gix binary:     {only_in_binary:?}",
    );
}
