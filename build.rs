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

// The runtime shares these modules; including them here keeps the producer
// (build.rs) and consumer (the `gitoxide` library) on the same schema.
// Modules using `super::types` resolve it to the crate root in both
// contexts (here the build script's root, in the library `src/licenses/`),
// so the files are compiled unchanged in either environment. The items the
// build script doesn't use are allowed without warning — they belong to the
// runtime side of the shared schema.
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

use std::collections::HashSet;
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
    let mut third_party: Vec<&cargo_metadata::Package> = metadata
        .packages
        .iter()
        .filter(|p| !workspace_members.contains(&p.id) && p.source.is_some())
        .collect();
    third_party.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.version.cmp(&b.version)));

    let crates: Vec<CrateLicense> = third_party.into_iter().map(build_crate_entry).collect();

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
