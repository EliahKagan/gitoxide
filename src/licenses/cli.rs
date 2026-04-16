//! Shared clap argument struct and dispatch for the `licenses` subcommand
//! exposed by both `gix` and `ein`. Keeping the surface area identical in one
//! place ensures the two binaries evolve together: flag names, help text,
//! and output formatting stay in lock-step.

use std::io::Write;

use anyhow::{Context, Result};
use gitoxide_core::licenses::{render, CrateLicense};
use gitoxide_core::OutputFormat;

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
    // and emitting it avoids an unnecessary parse+reserialise round-trip.
    let text = embedded::json().context("decompressing embedded license manifest")?;
    out.write_all(text.as_bytes())?;
    writeln!(out)?;
    Ok(())
}

fn emit_single_crate_json(out: &mut dyn Write, name: &str) -> Result<()> {
    let manifest = embedded::load().context("decoding embedded license manifest")?;
    let crate_entry: &CrateLicense = manifest
        .find(name)
        .with_context(|| format!("no dependency named `{name}` in the manifest"))?;
    serde_json::to_writer(&mut *out, crate_entry)
        .with_context(|| format!("encoding attribution for `{name}` as JSON"))?;
    writeln!(out)?;
    Ok(())
}
