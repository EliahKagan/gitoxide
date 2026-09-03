//! Verify that the crate set the compiled `gix` binary reports via
//! `gix licenses --format json` is exactly what cargo links into it for the
//! features and target it was built with.
//!
//! `cargo tree -p gitoxide` is the oracle: it resolves features for the
//! `gitoxide` package alone, as `cargo build -p gitoxide` does. `build.rs`
//! asks `cargo tree` the same question, but from a different starting point
//! — the `CARGO_FEATURE_*` and `TARGET` variables cargo hands a build script
//! — while this test derives the features from its own `cfg!(feature = …)`
//! flags and the target from the binary's self-report. A mistake in that
//! plumbing, or in how `build.rs` reads the answer, shows up as a set
//! difference here.

mod support;

use std::collections::BTreeSet;

use support::{cargo_tree_crates, enabled_feature_profiles, gix_manifest, manifest_crate_names, target_triple};

#[test]
fn manifest_lists_exactly_the_crates_cargo_links() {
    let manifest = gix_manifest();
    let features = enabled_feature_profiles();
    let target = target_triple(&manifest);
    let linked = cargo_tree_crates(&features, target);

    let linked_names: BTreeSet<&str> = linked.iter().map(|(name, _)| name.as_str()).collect();
    let from_binary = manifest_crate_names(&manifest);
    let from_binary: BTreeSet<&str> = from_binary.iter().map(String::as_str).collect();

    let only_in_cargo: Vec<&&str> = linked_names.difference(&from_binary).collect();
    let only_in_binary: Vec<&&str> = from_binary.difference(&linked_names).collect();
    assert!(
        only_in_cargo.is_empty() && only_in_binary.is_empty(),
        "crate-set mismatch for features={features:?}, target={target}:\n  \
         linked per cargo tree but missing from the manifest: {only_in_cargo:?}\n  \
         in the manifest but not linked per cargo tree:      {only_in_binary:?}",
    );

    // Full entries carry a version; each must be a version cargo links, so
    // that two versions of one crate are told apart correctly.
    let unlinked_versions: Vec<String> = manifest["crates"]
        .as_array()
        .expect("`crates` is an array")
        .iter()
        .map(|c| {
            (
                c["name"].as_str().unwrap_or("").to_owned(),
                c["version"].as_str().unwrap_or("").to_owned(),
            )
        })
        .filter(|entry| !linked.contains(entry))
        .map(|(name, version)| format!("{name} {version}"))
        .collect();
    assert!(
        unlinked_versions.is_empty(),
        "manifest entries whose version cargo tree does not link for features={features:?}, target={target}: \
         {unlinked_versions:?}",
    );
}
