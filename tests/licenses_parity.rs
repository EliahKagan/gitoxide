//! Verify that the crate set the compiled `gix` binary reports via
//! `gix licenses --format json` matches what `cargo metadata` resolves for
//! the same feature profile and host platform. This is the build-time-to-
//! runtime parity check: if `build.rs`'s feature or platform filtering
//! drifts, the two crate sets will diverge and this test fails.
//!
//! Rust-side replacement for what was previously a `licenses-parity` CI
//! step implemented as a Bash + Python heredoc. Now it lives with the rest
//! of the integration tests and runs whenever `cargo test` does.

use std::collections::BTreeSet;
use std::process::Command;

/// Host triple as reported by the `rustc` this `cargo test` is using.
///
/// For non-cross test runs (which is what we have in CI and locally) this
/// equals the `TARGET` env `build.rs` saw, so `cargo metadata` filtered by
/// this triple should agree with the binary's embedded manifest.
fn host_target() -> String {
    let out = Command::new("rustc").arg("-vV").output().expect("run `rustc -vV`");
    let text = String::from_utf8(out.stdout).expect("rustc output is UTF-8");
    match text.lines().find_map(|l| l.strip_prefix("host: ")) {
        Some(host) => host.trim().to_string(),
        None => panic!("no `host:` line in rustc -vV output:\n{text}"),
    }
}

/// The top-level feature-profile flags this test binary was compiled with,
/// as a slice of names suitable for `cargo metadata --features ...`.
///
/// `build.rs::enabled_top_level_features` walks every `CARGO_FEATURE_*` env
/// set at build time; here we key off `cfg!(feature = X)` which reflects the
/// same truth for feature X from the perspective of the test binary.
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

fn cargo_metadata_crate_names(features: &[&str], platform: &str) -> BTreeSet<String> {
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
fn gix_manifest_matches_cargo_metadata_for_build_config() {
    let features = enabled_feature_profiles();
    let host = host_target();
    let from_cargo = cargo_metadata_crate_names(&features, &host);
    let from_binary = gix_manifest_crate_names();

    let only_in_cargo: Vec<&String> = from_cargo.difference(&from_binary).collect();
    let only_in_binary: Vec<&String> = from_binary.difference(&from_cargo).collect();
    assert!(
        only_in_cargo.is_empty() && only_in_binary.is_empty(),
        "crate-set mismatch for features={features:?}, platform={host}:\n  \
         only in cargo metadata: {only_in_cargo:?}\n  \
         only in gix binary:     {only_in_binary:?}",
    );
}
