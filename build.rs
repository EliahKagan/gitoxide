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
//! The license logic degrades gracefully: if `cargo` cannot answer
//! (e.g. because the registry cache is unavailable), a minimal stub manifest
//! is emitted so `cargo build` still succeeds for end users. Under CI (`CI=1`)
//! we fail hard instead — regressions must be caught, not silently swallowed.
//!
//! For the rationale behind implementing this pipeline ourselves rather than
//! driving `cargo-about`, `cargo-bundle-licenses`, or similar from
//! `build.rs`, see the module-level docs of `src/licenses/mod.rs`.
//! The short version: every user running `cargo install gitoxide` must get
//! complete attribution without any auxiliary CLI tool installed.

// These modules are shared with the runtime side in this package's library
// (`crate::licenses`), so the producer and the consumer of the manifest have
// one source of truth. A build script cannot depend on the library of the
// package it builds, so they are compiled a second time here via `#[path]`.
//
// `render.rs` uses `super::types`, which resolves to this build script's
// crate root (where `mod types;` is declared below), and in the library
// resolves to `crate::licenses::types`. The files themselves are compiled
// unchanged in either environment. Items the build script does not use are
// allowed without warning — they belong to the runtime side.
#[allow(dead_code)]
#[path = "src/licenses/types.rs"]
mod types;

#[allow(dead_code)]
#[path = "src/licenses/render.rs"]
mod render;

#[allow(dead_code)]
#[path = "src/licenses/spdx_texts.rs"]
mod spdx_texts;

#[allow(dead_code)]
#[path = "src/licenses/build_support.rs"]
mod build_support;

use std::collections::{BTreeSet, HashSet};
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
    println!("cargo:rerun-if-changed=src/licenses/types.rs");
    println!("cargo:rerun-if-changed=src/licenses/render.rs");
    println!("cargo:rerun-if-changed=src/licenses/spdx_texts.rs");
    println!("cargo:rerun-if-changed=src/licenses/build_support.rs");
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
    let features = enabled_top_level_features();
    let target = std::env::var("TARGET").map_err(|_| "TARGET env var is not set".to_string())?;
    let linked = linked_packages(&features, &target)?;

    // `cargo metadata` supplies what `cargo tree` does not print: each
    // package's license, authors, links and, above all, the path to its
    // source tree, where the license files are. Its resolve graph is not
    // consulted — it unifies features across the whole workspace and so
    // over-approximates what this package links.
    let mut cmd = cargo_metadata::MetadataCommand::new();
    cmd.features(cargo_metadata::CargoOpt::NoDefaultFeatures);
    if !features.is_empty() {
        cmd.features(cargo_metadata::CargoOpt::SomeFeatures(features));
    }
    let metadata = cmd.exec().map_err(|e| format!("cargo metadata failed: {e}"))?;

    let workspace_members: HashSet<_> = metadata.workspace_members.iter().cloned().collect();
    let root_pkg = metadata
        .packages
        .iter()
        .find(|p| p.name == "gitoxide")
        .ok_or("gitoxide package not found in metadata")?;

    // Partition the linked packages into three buckets:
    //
    //   - full-attribution entries: third-party (non-workspace, sourced)
    //     deps, and workspace members whose license or authorship differs
    //     from the root `gitoxide` package's. These get a full
    //     `CrateLicense` entry with the discovered license files.
    //   - name-only workspace members: workspace members whose attribution
    //     matches the root's exactly. Listed by name only so readers can
    //     see the whole workspace footprint without re-reading the root's
    //     own LICENSE-MIT / LICENSE-APACHE for each one.
    //   - the root itself: excluded entirely.
    //
    // Each entry records both the package and whether it is a workspace
    // member (`true`) or a third-party crate (`false`); that is what
    // `CrateLicense::is_workspace_member` carries into the rendered
    // manifest, so consumers can group attribution without consulting
    // cargo at runtime.
    let mut to_attribute: Vec<(&cargo_metadata::Package, bool)> = Vec::new();
    let mut same_attribution_ws: Vec<String> = Vec::new();

    for (name, version) in &linked {
        let p = metadata
            .packages
            .iter()
            .find(|p| p.name == name.as_str() && p.version.to_string() == *version)
            .ok_or_else(|| format!("`cargo tree` lists `{name} {version}`, which `cargo metadata` does not know"))?;
        if p.id == root_pkg.id {
            continue;
        }
        if workspace_members.contains(&p.id) {
            if build_support::needs_separate_attribution(
                p.license.as_deref(),
                &p.authors,
                root_pkg.license.as_deref(),
                &root_pkg.authors,
            ) {
                to_attribute.push((p, true));
            } else {
                same_attribution_ws.push(p.name.to_string());
            }
        } else if p.source.is_some() {
            to_attribute.push((p, false));
        }
    }
    to_attribute.sort_by(|(a, _), (b, _)| a.name.cmp(&b.name).then_with(|| a.version.cmp(&b.version)));
    same_attribution_ws.sort();

    let crates: Vec<CrateLicense> = to_attribute
        .into_iter()
        .map(|(p, is_workspace_member)| build_crate_entry(p, is_workspace_member))
        .collect();

    Ok(Manifest {
        crates,
        workspace_members_same_attribution: same_attribution_ws,
        generated_at: now_stamp(),
        feature_profile: detect_feature_profile(),
        target_triple: target,
    })
}

/// The packages linked into this build of `gitoxide`, as name/version pairs,
/// according to `cargo tree`.
///
/// `cargo tree -p gitoxide` resolves features for this package alone, the
/// way `cargo build -p gitoxide` does. The other stable view of the graph,
/// `cargo metadata` (and the libraries built on it), reports the
/// workspace-wide resolve, in which a feature enabled by any workspace
/// member counts for all, and so over-approximates what is linked.
/// `normal` and `build` edges are followed; `dev` edges are not.
fn linked_packages(features: &[String], target: &str) -> Result<BTreeSet<(String, String)>, String> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut cmd = Command::new(cargo);
    cmd.args(["tree", "--package", "gitoxide", "--edges", "normal,build"])
        .args(["--prefix", "none", "--format", "{p}"])
        .args(["--no-default-features", "--target", target])
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }
    let output = cmd.output().map_err(|e| format!("running `cargo tree` failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "`cargo tree` failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let stdout = String::from_utf8(output.stdout).map_err(|e| format!("`cargo tree` output is not UTF-8: {e}"))?;
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            build_support::parse_cargo_tree_line(line)
                .map(|(name, version)| (name.to_owned(), version.to_owned()))
                .ok_or_else(|| format!("unexpected `cargo tree` output line: {line:?}"))
        })
        .collect()
}

/// Build a [`CrateLicense`] entry for one third-party dependency, discovering
/// license text from the crate's source tree and falling back to bundled
/// canonical SPDX text only when nothing else is available.
fn build_crate_entry(p: &cargo_metadata::Package, is_workspace_member: bool) -> CrateLicense {
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
        is_workspace_member,
    }
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
/// panicking on deserialization.
fn emit_stub_manifest(err: &str) -> std::io::Result<()> {
    let out_dir = out_dir().map_err(std::io::Error::other)?;
    let manifest = Manifest {
        crates: Vec::new(),
        workspace_members_same_attribution: Vec::new(),
        generated_at: format!("stub; {err}"),
        feature_profile: detect_feature_profile(),
        target_triple: std::env::var("TARGET").unwrap_or_default(),
    };
    write_manifest(&out_dir, &manifest)
}
