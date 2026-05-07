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
    about = "Show license and attribution for third-party dependencies linked into this binary",
    long_about = "Show license, copyright, attribution, and notice information for every \
                  third-party dependency statically linked into this binary.\n\n\
                  With no arguments, prints a summary table. With a single crate name, \
                  prints that crate's full attribution. With `--all`, prints the full \
                  concatenated attribution for every dependency, byte-identical to the \
                  `THIRD-PARTY-LICENSES.txt` file shipped alongside the binary in release \
                  archives."
)]
pub struct Command {
    /// The name of a single third-party crate whose full attribution should
    /// be printed.
    ///
    /// When omitted, a summary table of all dependencies is shown — pass
    /// `--all` to print every crate's full license text instead.
    pub crate_name: Option<String>,
    /// Print every crate's full attribution, byte-identical to the
    /// `THIRD-PARTY-LICENSES.txt` file shipped alongside the binary in the
    /// release archive.
    #[clap(long)]
    pub all: bool,
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
        (false, None) => render_summary_table(out),
    }
}

fn render_summary_table(out: &mut dyn Write) -> Result<()> {
    let manifest = embedded::load().context("decoding embedded license manifest")?;
    render::render_summary(out, &manifest).context("rendering license summary")?;
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
    if manifest.is_same_attribution_workspace_member(name) {
        // Same-attribution workspace members aren't carried as full
        // `CrateLicense` entries — their license is `gitoxide`'s own.
        // Emit a compact JSON note so machine consumers can tell this
        // case apart from "not found".
        let note = serde_json::json!({
            "name": name,
            "kind": "workspace-member-same-attribution",
            "note": "Workspace member with attribution identical to `gitoxide`. Refer to \
                     LICENSE-MIT and LICENSE-APACHE at the root of the gitoxide source tree \
                     or release archive.",
        });
        serde_json::to_writer(&mut *out, &note)
            .with_context(|| format!("encoding same-attribution entry for `{name}` as JSON"))?;
        writeln!(out)?;
        return Ok(());
    }
    anyhow::bail!("no dependency named `{name}` in the manifest")
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
}
