//! Shared clap argument struct and dispatch for the `licenses` subcommand
//! exposed by both `gix` and `ein`. Keeping the surface area identical in one
//! place ensures the two binaries evolve together: flag names, help text,
//! and output formatting stay in lock-step.

use std::io::Write;

use anyhow::{Context, Result};
use gitoxide_core::OutputFormat;
use gitoxide_core::licenses::render;

use super::embedded;

#[derive(Debug, clap::Parser)]
#[command(
    about = "Show license and attribution information for all linked crates",
    // NOTE: clap-derive truncates `long_about` at the first sentence boundary
    // (see https://github.com/clap-rs/clap/issues/2854 and similar). A
    // multi-sentence narrative here would be silently dropped on `--help`,
    // so the per-mode guidance is intentionally pushed into the field-level
    // doc comments below — those clap renders in full.
    long_about = "Show license, copyright, attribution, and notice information for every crate \
                  statically linked into this binary — third-party dependencies as well as \
                  gitoxide's own workspace members."
)]
pub struct Command {
    /// The name of a single crate whose full attribution should be printed.
    /// May be a third-party dependency, a workspace member with separate
    /// attribution, or a workspace member whose license and authorship
    /// match the root `gitoxide` package's (in which case the root's
    /// `LICENSE-MIT` and `LICENSE-APACHE` are printed inline).
    ///
    /// When omitted, a summary table is shown — pass `--all` to print every
    /// crate's full license text instead. The summary covers third-party
    /// crates and gitoxide's own workspace members that need their own
    /// attribution; pass `--verbose` to also list the workspace members
    /// whose license and authorship match the root `gitoxide` package's.
    pub crate_name: Option<String>,
    /// Print every crate's full attribution, byte-identical to the
    /// `THIRD-PARTY-LICENSES.txt` file shipped alongside the binary in the
    /// release archive.
    ///
    /// Mutually exclusive with naming a specific crate: `--all` means
    /// "every crate" and the specific-crate argument means "this one."
    #[clap(long, conflicts_with = "crate_name")]
    pub all: bool,
    /// In the default summary view, also list workspace members whose
    /// license and authorship match the root `gitoxide` package's. Has no
    /// effect when `--all` is set (the full attribution always includes
    /// that listing) or when a crate name argument is given.
    #[clap(long, short = 'v')]
    pub verbose: bool,
    /// Override the output format for this subcommand specifically.
    ///
    /// When unset, the format is inherited from the binary's top-level
    /// `--format` flag (for `gix`) or defaults to `human` (for `ein`, which
    /// has no top-level `--format`). Valid values are `human` and `json`.
    #[clap(long, short = 'f')]
    pub format: Option<OutputFormat>,
}

/// Run the `licenses` subcommand against the given output sink.
///
/// `inherited_format` is the format determined by the binary's top-level
/// `--format` flag (or [`OutputFormat::Human`] if the binary has none); a
/// subcommand-level `--format`, if given, takes precedence. This makes
/// `gix --format json licenses`, `gix licenses --format json`, and
/// `ein licenses --format json` all behave the same way.
///
/// The runtime check against [`OutputFormat::Human`] mirrors how other
/// subcommands (e.g. `gitoxide_core::env`) handle an optional `Json` variant
/// that exists only when `gitoxide-core/serde` is enabled — it compiles
/// regardless of whether that feature is active.
pub fn run(out: &mut dyn Write, inherited_format: OutputFormat, args: Command) -> Result<()> {
    let format = args.format.unwrap_or(inherited_format);
    if format != OutputFormat::Human {
        return match args.crate_name.as_deref() {
            None => emit_full_json(out),
            Some(name) => emit_single_crate_json(out, name),
        };
    }
    match (args.all, args.crate_name.as_deref()) {
        (true, _) => render_full_text(out),
        (false, Some(name)) => render_one_crate(out, name),
        (false, None) => render_summary_table(out, args.verbose),
    }
}

fn render_summary_table(out: &mut dyn Write, verbose: bool) -> Result<()> {
    let manifest = embedded::load().context("decoding embedded license manifest")?;
    render::render_summary_with_options(out, &manifest, render::SummaryOptions { verbose })
        .context("rendering license summary")?;
    Ok(())
}

fn render_one_crate(out: &mut dyn Write, name: &str) -> Result<()> {
    let manifest = embedded::load().context("decoding embedded license manifest")?;
    render::render_crate(out, &manifest, name).with_context(|| format!("rendering attribution for `{name}`"))?;
    Ok(())
}

fn render_full_text(out: &mut dyn Write) -> Result<()> {
    let manifest = embedded::load().context("decoding embedded license manifest")?;
    render::render_all(out, &manifest).context("rendering full attribution")?;
    Ok(())
}

fn emit_full_json(out: &mut dyn Write) -> Result<()> {
    // Stream the embedded JSON verbatim — it is already the canonical form
    // and emitting it avoids an unnecessary parse+reserialize round-trip.
    let text = embedded::json().context("decompressing embedded license manifest")?;
    out.write_all(text.as_bytes())?;
    writeln!(out)?;
    Ok(())
}

fn emit_single_crate_json(out: &mut dyn Write, name: &str) -> Result<()> {
    let manifest = embedded::load().context("decoding embedded license manifest")?;
    emit_single_crate_json_against(out, &manifest, name)
}

/// Inner half of [`emit_single_crate_json`] split out for testability:
/// takes the [`Manifest`] as a parameter rather than loading it from the
/// embedded blob, so unit tests can pass hand-crafted fixtures (notably
/// the multi-version case, which the real embedded manifest typically
/// does not exhibit).
fn emit_single_crate_json_against(
    out: &mut dyn Write,
    manifest: &gitoxide_core::licenses::Manifest,
    name: &str,
) -> Result<()> {
    let hits = manifest.find_all(name);
    if !hits.is_empty() {
        // Always emit a JSON array, regardless of whether one or many
        // versions matched. A consistent shape lets typed consumers parse
        // the output without having to branch on hit count, and the array
        // form makes the multi-version case representable instead of
        // silently dropping all but the first match (which the old
        // single-object form did).
        serde_json::to_writer(&mut *out, &hits)
            .with_context(|| format!("encoding attribution for `{name}` as JSON"))?;
        writeln!(out)?;
        return Ok(());
    }
    if manifest.is_covered_by_root_license(name) {
        // Synthesize a `CrateLicense` carrying the actual gitoxide root
        // license text (MIT and Apache-2.0). This keeps the JSON shape
        // consistent with the third-party / workspace-member cases — a
        // typed consumer can parse the same `CrateLicense` array shape
        // for any name — while supplying real attribution content
        // instead of a pointer-to-files note.
        let synthesized = synthesized_root_entry(name);
        serde_json::to_writer(&mut *out, &[synthesized])
            .with_context(|| format!("encoding root-license attribution for `{name}` as JSON"))?;
        writeln!(out)?;
        return Ok(());
    }
    anyhow::bail!("no dependency named `{name}` in the manifest")
}

/// Build a synthesized [`CrateLicense`] entry for the root-license case
/// (the root `gitoxide` package itself, or a workspace member whose
/// attribution matches the root). Carries the actual MIT and Apache-2.0
/// file contents from the repository's top-level files. Fields we don't
/// have a concrete value for (version, repository, homepage, authors)
/// are left empty/None — consumers who need them can look them up
/// against the rest of the manifest or against cargo metadata.
fn synthesized_root_entry(name: &str) -> gitoxide_core::licenses::CrateLicense {
    use gitoxide_core::licenses::{CrateLicense, LicenseFile, spdx_texts};
    CrateLicense {
        name: name.to_string(),
        version: String::new(),
        spdx: Some("MIT OR Apache-2.0".to_string()),
        authors: Vec::new(),
        repository: None,
        homepage: None,
        files: vec![
            LicenseFile {
                name: "LICENSE-MIT".to_string(),
                text: spdx_texts::MIT.to_string(),
            },
            LicenseFile {
                name: "LICENSE-APACHE".to_string(),
                text: spdx_texts::APACHE_2_0.to_string(),
            },
        ],
        used_spdx_fallback: false,
        is_workspace_member: name != "gitoxide",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gitoxide_core::licenses::{CrateLicense, Manifest};

    fn crate_entry(name: &str, version: &str, spdx: &str) -> CrateLicense {
        CrateLicense {
            name: name.into(),
            version: version.into(),
            spdx: Some(spdx.into()),
            authors: vec![],
            repository: None,
            homepage: None,
            files: vec![],
            used_spdx_fallback: false,
            is_workspace_member: false,
        }
    }

    fn manifest_with(crates: Vec<CrateLicense>, ws_same: Vec<String>) -> Manifest {
        Manifest {
            crates,
            workspace_members_same_attribution: ws_same,
            generated_at: "t".into(),
            feature_profile: None,
            target_triple: "x86_64-unknown-linux-gnu".into(),
        }
    }

    /// JSON output for `gix licenses <NAME>` is always a JSON array of
    /// matching entries — even when only one version matches — so typed
    /// consumers can parse the output without branching on hit count.
    #[test]
    fn single_match_emits_json_array_of_one() {
        let manifest = manifest_with(vec![crate_entry("anyhow", "1.0.98", "MIT OR Apache-2.0")], vec![]);
        let mut buf = Vec::new();
        emit_single_crate_json_against(&mut buf, &manifest, "anyhow").unwrap();
        let s = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("output must be valid JSON");
        let arr = parsed.as_array().expect("output must be a JSON array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["name"], "anyhow");
        assert_eq!(arr[0]["version"], "1.0.98");
    }

    /// When two crates share a name (different versions, possibly different
    /// licenses), the JSON output must contain BOTH — not just the first
    /// match silently. This is the bug the multi-version-handling change
    /// fixes.
    #[test]
    fn multi_match_emits_json_array_with_every_matching_entry() {
        let manifest = manifest_with(
            vec![
                crate_entry("dual-version", "1.0.0", "MIT"),
                crate_entry("dual-version", "2.0.0", "Apache-2.0"),
            ],
            vec![],
        );
        let mut buf = Vec::new();
        emit_single_crate_json_against(&mut buf, &manifest, "dual-version").unwrap();
        let s = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("output must be valid JSON");
        let arr = parsed.as_array().expect("output must be a JSON array");
        assert_eq!(arr.len(), 2, "expected both versions in the JSON output");
        let versions: Vec<&str> = arr.iter().map(|v| v["version"].as_str().unwrap()).collect();
        assert!(versions.contains(&"1.0.0"));
        assert!(versions.contains(&"2.0.0"));
        let spdxs: Vec<&str> = arr.iter().map(|v| v["spdx"].as_str().unwrap()).collect();
        assert!(spdxs.contains(&"MIT"));
        assert!(spdxs.contains(&"Apache-2.0"));
    }

    #[test]
    fn no_match_returns_error() {
        let manifest = manifest_with(vec![crate_entry("anyhow", "1.0.98", "MIT")], vec![]);
        let mut buf = Vec::new();
        let err =
            emit_single_crate_json_against(&mut buf, &manifest, "nonexistent").expect_err("unknown name must error");
        assert!(
            err.to_string().contains("nonexistent"),
            "error must name the missing crate"
        );
    }

    /// `gix licenses --format json gix-sec` (a same-attribution workspace
    /// member) should emit a JSON array containing a synthesized entry
    /// with the actual MIT and Apache-2.0 license file contents. This
    /// gives JSON consumers the real attribution data they need, in the
    /// same shape they'd get for a third-party crate.
    #[test]
    fn same_attribution_workspace_member_emits_synthesized_entry_with_real_text() {
        let manifest = manifest_with(vec![], vec!["gix-sec".into()]);
        let mut buf = Vec::new();
        emit_single_crate_json_against(&mut buf, &manifest, "gix-sec").unwrap();
        let s = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("output must be valid JSON");
        let arr = parsed.as_array().expect("output must be a JSON array");
        assert_eq!(arr.len(), 1, "expected exactly one synthesized entry");
        let entry = &arr[0];
        assert_eq!(entry["name"], "gix-sec");
        assert_eq!(entry["spdx"], "MIT OR Apache-2.0");
        assert_eq!(entry["is_workspace_member"], true);
        let files = entry["files"].as_array().expect("files must be a JSON array");
        assert_eq!(files.len(), 2, "should include both LICENSE-MIT and LICENSE-APACHE");
        let names: Vec<&str> = files.iter().map(|f| f["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"LICENSE-MIT"));
        assert!(names.contains(&"LICENSE-APACHE"));
        // Each file must contain its actual canonical text body, not a placeholder.
        let mit_text = files
            .iter()
            .find(|f| f["name"] == "LICENSE-MIT")
            .map(|f| f["text"].as_str().unwrap())
            .unwrap();
        assert!(
            mit_text.contains("Permission is hereby granted"),
            "MIT body must be the canonical text"
        );
        let apache_text = files
            .iter()
            .find(|f| f["name"] == "LICENSE-APACHE")
            .map(|f| f["text"].as_str().unwrap())
            .unwrap();
        assert!(
            apache_text.contains("Apache License") && apache_text.contains("Version 2.0"),
            "Apache-2.0 body must be the canonical text"
        );
    }

    /// `gix licenses --format json gitoxide` should also work (the root
    /// package isn't in the manifest's `crates` list but is queryable),
    /// and the synthesized entry must mark it as the root rather than a
    /// workspace member.
    #[test]
    fn gitoxide_root_emits_synthesized_entry_marked_as_non_workspace_member() {
        let manifest = manifest_with(vec![], vec![]);
        let mut buf = Vec::new();
        emit_single_crate_json_against(&mut buf, &manifest, "gitoxide").unwrap();
        let s = String::from_utf8(buf).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&s).expect("output must be valid JSON");
        let arr = parsed.as_array().expect("output must be a JSON array");
        assert_eq!(arr.len(), 1);
        let entry = &arr[0];
        assert_eq!(entry["name"], "gitoxide");
        assert_eq!(
            entry["is_workspace_member"], false,
            "the root package itself must not be flagged as a workspace member"
        );
        let files = entry["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
    }
}
