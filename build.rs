//! Build script for the top-level `gitoxide` crate.
//!
//! Responsibilities:
//!
//! * Derive the user-visible version string for `gix --version` / `ein --version`
//!   from `git describe`, falling back to `CARGO_PKG_VERSION`.
//! * Generate a third-party dependency license manifest as two artifacts in
//!   `OUT_DIR`:
//!   - `third_party_licenses.json` — structured data, embedded via
//!     `include_str!` for the `gix licenses` / `ein licenses` subcommands.
//!   - `third_party_licenses.txt`  — the same data pre-rendered as plain
//!     text so the `release.yml` workflow can copy it straight into the
//!     archive as `THIRD-PARTY-LICENSES.txt`.
//!
//! The license logic degrades gracefully: if `cargo metadata` cannot be run
//! (e.g. because the registry cache is unavailable), a minimal stub manifest
//! is emitted so `cargo build` still succeeds for end users. Under CI (`CI=1`)
//! we fail hard instead — regressions must be caught, not silently swallowed.
//!
//! For the rationale behind implementing this pipeline ourselves rather than
//! driving `cargo-about`, `cargo-bundle-licenses`, or similar from
//! `build.rs`, see the module-level docs on
//! [`gitoxide_core::licenses`](../gitoxide-core/src/licenses/mod.rs).
//! The short version: every user running `cargo install gitoxide` must get
//! complete attribution without any auxiliary CLI tool installed.

// These modules live in `gitoxide-core` so the runtime (consumed via
// `gitoxide_core::licenses`) and `build.rs` (the producer) share one
// source of truth. We include them here with `#[path]` rather than adding
// `gitoxide-core` as a build-dependency because `gitoxide-core` pulls in
// the full `gix` stack — compiling that an extra time just to run the
// build script would be disproportionate.
//
// `render.rs` uses `super::types`, which resolves to this build script's
// crate root (where `mod types;` is declared below), and at runtime
// resolves to `gitoxide_core::licenses::types`. The files themselves are
// compiled unchanged in either environment. Items the build script does
// not use are allowed without warning — they belong to the runtime side.
#[allow(dead_code)]
#[path = "gitoxide-core/src/licenses/types.rs"]
mod types;

#[allow(dead_code)]
#[path = "gitoxide-core/src/licenses/render.rs"]
mod render;

#[allow(dead_code)]
#[path = "gitoxide-core/src/licenses/spdx_texts.rs"]
mod spdx_texts;

#[allow(dead_code)]
#[path = "gitoxide-core/src/licenses/build_support.rs"]
mod build_support;

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use types::{CrateLicense, Manifest};

fn main() {
    set_gix_version();

    // We need the manifest to rebuild whenever the dep graph or the resolved
    // feature/target set changes. `Cargo.lock` captures dep version moves,
    // `TARGET` captures cross-builds, and the render/types source files are
    // pulled in via `#[path]` — changes to them must also trigger rebuilds.
    println!("cargo:rerun-if-changed=Cargo.lock");
    // Every source file we include via `#[path]` needs an explicit rerun
    // trigger — Cargo only tracks files it discovered through the normal
    // module tree, and build.rs pulls these in out-of-band.
    println!("cargo:rerun-if-changed=gitoxide-core/src/licenses/types.rs");
    println!("cargo:rerun-if-changed=gitoxide-core/src/licenses/render.rs");
    println!("cargo:rerun-if-changed=gitoxide-core/src/licenses/spdx_texts.rs");
    println!("cargo:rerun-if-changed=gitoxide-core/src/licenses/build_support.rs");
    // The MIT / Apache-2.0 fallback texts live in repo-root files that
    // `spdx_texts.rs` pulls in with `include_str!`. Changes to them must
    // re-emit the manifest too.
    println!("cargo:rerun-if-changed=LICENSE-MIT");
    println!("cargo:rerun-if-changed=LICENSE-APACHE");
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-env-changed=CI");

    if let Err(e) = generate_license_manifest() {
        let in_ci = std::env::var_os("CI").is_some();
        if in_ci {
            // In CI we want regressions to fail builds loudly.
            panic!("license manifest generation failed (CI=1): {e}");
        }
        // For user machines, degrade gracefully: emit a stub that at least
        // names gitoxide itself and explains how to regenerate a full one.
        println!("cargo:warning=license manifest generation failed (emitting stub): {e}");
        emit_stub_manifest(&e.to_string()).expect("stub manifest emission must not fail");
    }
}

fn set_gix_version() {
    let version = Command::new(if cfg!(windows) { "git.exe" } else { "git" })
        .args(["describe", r"--match=v*\.*\.*"])
        .output()
        .ok()
        .and_then(|out| {
            if !out.status.success() {
                return None;
            }
            try_parse_describe(&out.stdout)
        })
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").into());
    println!("cargo:rustc-env=GIX_VERSION={version}");
}

fn try_parse_describe(input: &[u8]) -> Option<String> {
    let input = std::str::from_utf8(input).ok()?;
    let trimmed = input.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

/// Gather the third-party dependency manifest and write both artifacts into
/// `OUT_DIR`. Returns an error string instead of panicking so the caller can
/// decide whether this is fatal.
fn generate_license_manifest() -> Result<(), String> {
    let out_dir = out_dir()?;
    let manifest = collect_manifest()?;
    write_manifest(&out_dir, &manifest).map_err(|e| format!("writing manifest failed: {e}"))
}

fn out_dir() -> Result<PathBuf, String> {
    std::env::var_os("OUT_DIR")
        .map(PathBuf::from)
        .ok_or_else(|| "OUT_DIR env var is not set".to_string())
}

fn collect_manifest() -> Result<Manifest, String> {
    let mut cmd = cargo_metadata::MetadataCommand::new();
    cmd.features(cargo_metadata::CargoOpt::NoDefaultFeatures);
    let enabled_features = enabled_top_level_features();
    if !enabled_features.is_empty() {
        cmd.features(cargo_metadata::CargoOpt::SomeFeatures(enabled_features));
    }
    // Scope the graph to the actual build target so
    // `[target.'cfg(...)'.dependencies]` tables are honoured.
    let mut extra: Vec<String> = Vec::new();
    if let Some(target) = std::env::var_os("TARGET") {
        extra.push("--filter-platform".into());
        extra.push(target.to_string_lossy().into_owned());
    }
    if !extra.is_empty() {
        cmd.other_options(extra);
    }

    let metadata = cmd.exec().map_err(|e| format!("cargo metadata failed: {e}"))?;

    let workspace_members: HashSet<_> = metadata.workspace_members.iter().cloned().collect();

    // Determine which packages are reachable from the `gitoxide` binary's
    // dependency graph (as opposed to other workspace members' graphs).
    // This matters because some workspace members (e.g. test crates) are
    // never linked into gix/ein and shouldn't appear in the manifest.
    let reachable = reachable_from_root(&metadata, "gitoxide")?;

    let root_pkg = metadata
        .packages
        .iter()
        .find(|p| p.name == "gitoxide")
        .ok_or("gitoxide package not found in metadata")?;

    // Include a package iff it is actually reachable from the `gitoxide`
    // binary via non-dev edges AND either:
    //
    //   - it is a third-party (non-workspace, sourced) dep, or
    //   - it is a workspace member whose license or authorship differs
    //     from the root `gitoxide` package's.
    //
    // The reachability gate ensures dev-only transitive crates (e.g.
    // `gix-testtools`, which is a `[dev-dependencies]` of most `gix-*`
    // crates but never linked into the binary) never appear regardless of
    // their attribution. Without it, a future change to such a crate's
    // license or authors would silently surface it in the manifest.
    let mut to_attribute: Vec<&cargo_metadata::Package> = metadata
        .packages
        .iter()
        .filter(|p| {
            if !reachable.contains(&p.id) {
                return false;
            }
            if workspace_members.contains(&p.id) {
                build_support::needs_separate_attribution(
                    p.license.as_deref(),
                    &p.authors,
                    root_pkg.license.as_deref(),
                    &root_pkg.authors,
                )
            } else {
                p.source.is_some()
            }
        })
        .collect();
    to_attribute.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.version.cmp(&b.version)));

    let crates: Vec<CrateLicense> = to_attribute.into_iter().map(build_crate_entry).collect();

    Ok(Manifest {
        crates,
        generated_at: now_stamp(),
        feature_profile: detect_feature_profile(),
        target_triple: std::env::var("TARGET").unwrap_or_default(),
    })
}

/// Build a [`CrateLicense`] entry for one third-party dependency, discovering
/// license text from the crate's source tree and falling back to bundled
/// canonical SPDX text only when nothing else is available.
fn build_crate_entry(p: &cargo_metadata::Package) -> CrateLicense {
    // `manifest_path` is a `camino::Utf8PathBuf`; `as_std_path` borrows a
    // regular `&Path` without allocating, which is what `collect_crate_license_files` wants.
    let files_from_source = p
        .manifest_path
        .parent()
        .map(|p| build_support::collect_crate_license_files(p.as_std_path()))
        .unwrap_or_default();

    let (files, used_spdx_fallback) = if !files_from_source.is_empty() {
        (files_from_source, false)
    } else if let Some(spdx) = p.license.as_deref() {
        let fallback = build_support::spdx_fallback_files(spdx, spdx_texts::text_for);
        let used = !fallback.is_empty();
        (fallback, used)
    } else {
        (Vec::new(), false)
    };

    CrateLicense {
        name: p.name.to_string(),
        version: p.version.to_string(),
        spdx: p.license.clone(),
        authors: p.authors.clone(),
        repository: p.repository.clone(),
        homepage: p.homepage.clone(),
        files,
        used_spdx_fallback,
    }
}

/// BFS from the named root package through the resolve graph, returning
/// every package ID reachable from it via `normal` or `build` dep edges.
/// Dev-dep edges are excluded: a crate that is only reachable through
/// `[dev-dependencies]` somewhere in the tree (e.g. `gix-testtools`) is
/// used for testing other crates and does not end up in the `gix` / `ein`
/// binary, so it does not need its license attributed here.
///
/// Build-dep edges are included: a build-dep's code runs at compile time
/// and its output is absorbed into its parent crate's library artefact,
/// which is then linked into the binary. Attributing it is at worst
/// harmless and at best correct for transitive-generated-code cases.
fn reachable_from_root(
    metadata: &cargo_metadata::Metadata,
    root_name: &str,
) -> Result<HashSet<cargo_metadata::PackageId>, String> {
    let resolve = metadata
        .resolve
        .as_ref()
        .ok_or("cargo metadata produced no resolve graph")?;

    let root_id = metadata
        .packages
        .iter()
        .find(|p| p.name == root_name)
        .map(|p| &p.id)
        .ok_or_else(|| format!("package `{root_name}` not found in metadata"))?;

    // Follow only edges that are at least partly `normal` or `build`.
    // If every `dep_kinds` entry is `Development`, the edge leads to a
    // dev-only dep and we skip it.
    let deps_of: HashMap<&cargo_metadata::PackageId, Vec<&cargo_metadata::PackageId>> = resolve
        .nodes
        .iter()
        .map(|n| {
            let linked_deps: Vec<&cargo_metadata::PackageId> = n
                .deps
                .iter()
                .filter(|d| {
                    // Older cargo_metadata versions did not populate
                    // `dep_kinds`; treat an empty list as "normal" for
                    // back-compat so we don't silently drop edges.
                    d.dep_kinds.is_empty()
                        || d.dep_kinds
                            .iter()
                            .any(|dk| !matches!(dk.kind, cargo_metadata::DependencyKind::Development))
                })
                .map(|d| &d.pkg)
                .collect();
            (&n.id, linked_deps)
        })
        .collect();

    let mut reachable = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(root_id.clone());
    while let Some(id) = queue.pop_front() {
        if !reachable.insert(id.clone()) {
            continue;
        }
        if let Some(deps) = deps_of.get(&id) {
            for dep_id in deps {
                queue.push_back((*dep_id).clone());
            }
        }
    }
    Ok(reachable)
}

fn enabled_top_level_features() -> Vec<String> {
    // Cargo exposes each enabled feature as `CARGO_FEATURE_<NAME>` where the
    // name is uppercased and hyphens become underscores. These are features of
    // the `gitoxide` package specifically — transitive crate features are not
    // exposed this way — so passing them back to `cargo metadata` as the
    // feature set is sound.
    let mut features: Vec<String> = std::env::vars_os()
        .filter_map(|(k, _)| {
            let k = k.into_string().ok()?;
            let rest = k.strip_prefix("CARGO_FEATURE_")?;
            Some(rest.to_ascii_lowercase().replace('_', "-"))
        })
        .collect();
    features.sort();
    features.dedup();
    features
}

/// Return the top-level feature profile (`max`, `max-pure`, `lean`, `small`,
/// `lean-async`) that is enabled, if one can be identified unambiguously.
fn detect_feature_profile() -> Option<String> {
    let candidates = ["max", "max-pure", "lean", "small", "lean-async"];
    // `max-pure` implies `max-control` but not `max`, so we can key off the
    // env vars directly. If more than one top-level profile is enabled we
    // report `None` rather than pick arbitrarily.
    let enabled: Vec<&str> = candidates
        .iter()
        .copied()
        .filter(|name| {
            let env = format!("CARGO_FEATURE_{}", name.to_ascii_uppercase().replace('-', "_"));
            std::env::var_os(&env).is_some()
        })
        .collect();
    match enabled.as_slice() {
        [one] => Some((*one).to_string()),
        _ => None,
    }
}

fn now_stamp() -> String {
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs()))
        .unwrap_or(0);
    format!("unix={secs}")
}

fn write_manifest(out_dir: &Path, manifest: &Manifest) -> std::io::Result<()> {
    // The uncompressed `.json` is what the release workflow copies into the
    // archive as `THIRD-PARTY-LICENSES.json` for users who want to parse it
    // without running the binary.
    let json_path = out_dir.join("third_party_licenses.json");
    let json = serde_json::to_string(manifest).map_err(std::io::Error::other)?;
    std::fs::write(&json_path, &json)?;

    // The compressed form is what the binary embeds. Compressing with max
    // level keeps binary growth under a few hundred kilobytes even for the
    // `max` profile, where the uncompressed manifest approaches 3 MB.
    let json_gz_path = out_dir.join("third_party_licenses.json.gz");
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(json.as_bytes(), 10);
    std::fs::write(&json_gz_path, &compressed)?;

    // The uncompressed `.txt` mirrors what `gix licenses --all` would print
    // byte-for-byte, and the release workflow ships it as
    // `THIRD-PARTY-LICENSES.txt` in the archive. The binary does not embed
    // it — `render_all` can regenerate the same bytes from the loaded JSON.
    let txt_path = out_dir.join("third_party_licenses.txt");
    let mut txt = std::fs::File::create(&txt_path)?;
    render::render_all(&mut txt, manifest)?;
    Ok(())
}

/// Emit a minimal manifest used only when the real one fails to generate.
/// It must still produce valid JSON/text that `include_str!` can consume, so
/// the runtime subcommand can tell users the situation plainly instead of
/// panicking on deserialisation.
fn emit_stub_manifest(err: &str) -> std::io::Result<()> {
    let out_dir = out_dir().map_err(std::io::Error::other)?;
    let manifest = Manifest {
        crates: Vec::new(),
        generated_at: format!("stub; {err}"),
        feature_profile: detect_feature_profile(),
        target_triple: std::env::var("TARGET").unwrap_or_default(),
    };
    write_manifest(&out_dir, &manifest)
}
