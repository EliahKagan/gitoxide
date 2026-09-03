//! Helpers shared by the `licenses_*` integration tests.
#![allow(dead_code)]

use std::collections::BTreeSet;
use std::process::Command;

/// The manifest embedded in the `gix` binary under test, as parsed JSON.
pub fn gix_manifest() -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_gix"))
        .args(["licenses", "--format", "json"])
        .output()
        .expect("run `gix licenses --format json`");
    assert!(
        output.status.success(),
        "`gix licenses --format json` failed ({}):\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    serde_json::from_slice(&output.stdout).expect("parse `gix licenses` JSON")
}

/// Names of every crate the manifest attributes: the full entries plus the
/// workspace members listed by name only.
pub fn manifest_crate_names(manifest: &serde_json::Value) -> BTreeSet<String> {
    let full = manifest["crates"]
        .as_array()
        .expect("`crates` is an array")
        .iter()
        .filter_map(|c| c["name"].as_str());
    let by_name = manifest["workspace_members_same_attribution"]
        .as_array()
        .expect("`workspace_members_same_attribution` is an array")
        .iter()
        .filter_map(|v| v.as_str());
    full.chain(by_name).map(String::from).collect()
}

/// The target triple the binary under test was built for, as it recorded
/// in its manifest from the `TARGET` variable cargo hands build scripts.
///
/// Asking the binary is more reliable than asking `rustc -vV`: the `rustc`
/// on `PATH` need not be the one cargo used, and the `RUSTC` variable cargo
/// exports to build scripts is not exported to integration tests.
pub fn target_triple(manifest: &serde_json::Value) -> &str {
    let triple = manifest["target_triple"]
        .as_str()
        .expect("manifest has a target_triple string");
    assert!(
        !triple.is_empty(),
        "the gix binary reports an empty target_triple; build.rs's TARGET passthrough may have regressed",
    );
    triple
}

/// The top-level feature profiles this test crate was compiled with — the
/// same ones the binary under test was built with.
pub fn enabled_feature_profiles() -> Vec<&'static str> {
    [
        (cfg!(feature = "max"), "max"),
        (cfg!(feature = "max-pure"), "max-pure"),
        (cfg!(feature = "lean"), "lean"),
        (cfg!(feature = "small"), "small"),
        (cfg!(feature = "lean-async"), "lean-async"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect()
}

/// Every package `cargo tree` reports as linked into `gitoxide` through
/// `normal` or `build` edges for the given features and target, as
/// name/version pairs, without `gitoxide` itself.
///
/// This is the oracle for what the manifest must list: `cargo tree -p
/// gitoxide` resolves features for the `gitoxide` package alone, the way
/// `cargo build -p gitoxide` does, whereas `cargo metadata` reports the
/// workspace-wide resolve and over-approximates.
pub fn cargo_tree_crates(features: &[&str], target: &str) -> BTreeSet<(String, String)> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args(["tree", "--package", "gitoxide", "--edges", "normal,build"])
        .args(["--prefix", "none", "--format", "{p}"])
        .args(["--no-default-features", "--target", target])
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    for feature in features {
        cmd.args(["--features", feature]);
    }
    let output = cmd.output().expect("run `cargo tree`");
    assert!(
        output.status.success(),
        "`cargo tree` failed ({}):\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    let stdout = String::from_utf8(output.stdout).expect("`cargo tree` output is UTF-8");
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            // `name vX.Y.Z`, then optional annotations such as a path, `(proc-macro)` or `(*)`.
            let mut words = line.split_whitespace();
            let name = words.next().expect("a crate name");
            let version = words
                .next()
                .and_then(|v| v.strip_prefix('v'))
                .unwrap_or_else(|| panic!("`cargo tree` line without a version: {line:?}"));
            (name.to_owned(), version.to_owned())
        })
        .filter(|(name, _)| name != "gitoxide")
        .collect()
}
