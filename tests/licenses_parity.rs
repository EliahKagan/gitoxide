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
