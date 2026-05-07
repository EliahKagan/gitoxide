//! Rendering of the dependency-license manifest for human consumption.
//!
//! Four top-level outputs are offered, all writing into an arbitrary
//! [`Write`] sink:
//!
//! * [`render_summary`] — a header followed by two column-aligned tables:
//!   true third-party crates, and gitoxide's own workspace members that
//!   need their own attribution (because their license or authorship
//!   differs from the root). Followed by a legend (only when any crate
//!   carries a footnote mark in the notes column) and a footer pointing
//!   the reader at `gix licenses <CRATE>` and `--all`. Convenience
//!   wrapper around [`render_summary_with_options`] with default
//!   options.
//! * [`render_summary_with_options`] — identical to [`render_summary`]
//!   but takes a [`SummaryOptions`] knob. With [`SummaryOptions::verbose`]
//!   set, a third names-only listing is appended: workspace members
//!   whose attribution matches the root and so are covered by the
//!   repository's `LICENSE-MIT` / `LICENSE-APACHE` files. The default
//!   omits that listing because for many manifests it is the largest of
//!   the three groups and clutters the at-a-glance summary.
//! * [`render_crate`] — one crate's attribution in full: version, SPDX,
//!   repository/homepage, authors, and every LICENSE / NOTICE / AUTHORS /
//!   COPYRIGHT / COPYING file found in its source tree. For the root
//!   `gitoxide` package (or any same-attribution workspace member),
//!   prints the actual MIT and Apache-2.0 file bodies inline.
//! * [`render_all`] — header plus full per-crate attribution for every
//!   crate, in three sections — third-party crates, workspace-with-
//!   separate-attribution, and (always, regardless of any verbose knob)
//!   the same-attribution names listing — separated by dividers. This
//!   is the canonical form that is also emitted to
//!   `THIRD-PARTY-LICENSES.txt` in release archives, so the subcommand
//!   output and the archive file match byte-for-byte for a given build.
//!
//! Footnote marks `[*]` (SPDX fallback) and `[!]` (no license text)
//! appear in the summary table's notes column for crates that carry the
//! corresponding flag, and a legend explaining each used mark is printed
//! between the data tables and the footer guidance.
//!
//! None of these functions allocate a full string copy of the manifest; they
//! stream formatted output through the [`Write`] sink.

use std::io::{self, Write};

use super::spdx_texts;
use super::types::{CrateLicense, Manifest};

const CRATE_DIVIDER: &str = "================================================================";
const FILE_DIVIDER: &str = "----------------------------------------------------------------";

/// Write the manifest header: profile, target, generation timestamp.
///
/// Per-section counts are printed at each section header rather than here,
/// so that a reader doesn't have to mentally subtract one count from another
/// to reason about how many crates fall into which group.
fn write_header(w: &mut (impl Write + ?Sized), manifest: &Manifest) -> io::Result<()> {
    writeln!(w, "gitoxide license attribution")?;
    let profile = manifest.feature_profile.as_deref().unwrap_or("(custom features)");
    writeln!(w, "Feature profile: {profile}")?;
    writeln!(w, "Target triple:   {}", manifest.target_triple)?;
    writeln!(w, "Generated at:    {}", manifest.generated_at)?;
    writeln!(w)?;
    Ok(())
}

/// Footnote mark for a crate whose license text was sourced from a bundled
/// canonical SPDX copy because no LICENSE/COPYING/NOTICE file was present in
/// the crate source. Defined explicitly so the table-cell writer and the
/// legend writer cannot drift.
const SPDX_FALLBACK_MARK: &str = "[*]";
/// Footnote mark for a crate that has no license text at all — neither a
/// discovered file nor a bundled SPDX fallback.
const MISSING_TEXT_MARK: &str = "[!]";

/// Compact footnote string for the notes column of a single row. Combines
/// applicable marks with a space separator. Returns an empty string when
/// the crate carries no marks at all.
fn marks_for(c: &CrateLicense) -> String {
    let mut s = String::new();
    if c.used_spdx_fallback {
        s.push_str(SPDX_FALLBACK_MARK);
    }
    if c.is_missing_text() {
        if !s.is_empty() {
            s.push(' ');
        }
        s.push_str(MISSING_TEXT_MARK);
    }
    s
}

/// Write a column-aligned table for one of the per-section crate slices in
/// the summary view. Column widths are computed from the slice itself so each
/// section aligns within itself; this keeps the third-party section's name
/// column from being padded out to fit a long workspace-member name from a
/// different section.
fn write_summary_table(w: &mut (impl Write + ?Sized), crates: &[&CrateLicense]) -> io::Result<()> {
    if crates.is_empty() {
        return Ok(());
    }
    // Column width = widest cell or the header, whichever is longer. The
    // fallback floors only matter when `crates` is empty (and we've already
    // returned), but they keep the closure honest.
    let col_width = |default: usize, header: &str, cell: fn(&CrateLicense) -> usize| -> usize {
        crates
            .iter()
            .copied()
            .map(cell)
            .max()
            .unwrap_or(default)
            .max(header.len())
    };
    let name_width = col_width(4, "NAME", |c| c.name.len());
    let version_width = col_width(7, "VERSION", |c| c.version.len());
    let spdx_width = col_width(7, "LICENSE", |c| c.spdx.as_deref().unwrap_or("(none)").len());

    writeln!(
        w,
        "{name:<name_width$}  {version:<version_width$}  {spdx:<spdx_width$}  NOTES",
        name = "NAME",
        version = "VERSION",
        spdx = "LICENSE",
    )?;

    for c in crates {
        let spdx = c.spdx.as_deref().unwrap_or("(none)");
        writeln!(
            w,
            "{name:<name_width$}  {version:<version_width$}  {spdx:<spdx_width$}  {notes}",
            name = c.name,
            version = c.version,
            spdx = spdx,
            notes = marks_for(c),
        )?;
    }
    Ok(())
}

/// Write a legend explaining any footnote marks that appeared in the data
/// tables, gated on whether each mark was actually used. When no crate in
/// the manifest carries any mark, the entire legend is skipped — there is
/// nothing to explain.
fn write_legend(w: &mut (impl Write + ?Sized), manifest: &Manifest) -> io::Result<()> {
    let any_spdx_fallback = manifest.crates.iter().any(|c| c.used_spdx_fallback);
    let any_missing_text = manifest.crates.iter().any(CrateLicense::is_missing_text);
    if !any_spdx_fallback && !any_missing_text {
        return Ok(());
    }
    writeln!(w)?;
    if any_spdx_fallback {
        writeln!(
            w,
            "{SPDX_FALLBACK_MARK} license text was sourced from a bundled canonical SPDX copy \
             (the crate's source had no LICENSE/COPYING/NOTICE file)"
        )?;
    }
    if any_missing_text {
        writeln!(
            w,
            "{MISSING_TEXT_MARK} no license text available — neither a discovered file nor a bundled SPDX fallback applied"
        )?;
    }
    Ok(())
}

/// Partition the manifest's `crates` into third-party crates and gitoxide
/// workspace members with separate attribution. Both halves preserve the
/// (name, version) order in which they appeared in the source manifest.
fn partition_crates(manifest: &Manifest) -> (Vec<&CrateLicense>, Vec<&CrateLicense>) {
    let mut third_party = Vec::new();
    let mut workspace_members = Vec::new();
    for c in &manifest.crates {
        if c.is_workspace_member {
            workspace_members.push(c);
        } else {
            third_party.push(c);
        }
    }
    (third_party, workspace_members)
}

/// Write the section listing workspace members whose attribution matches the
/// root package's (recorded by name only — the actual license text comes from
/// the root LICENSE-MIT / LICENSE-APACHE files).
fn write_same_attribution_workspace_section(w: &mut (impl Write + ?Sized), manifest: &Manifest) -> io::Result<()> {
    if manifest.workspace_members_same_attribution.is_empty() {
        return Ok(());
    }
    writeln!(w)?;
    writeln!(
        w,
        "Workspace members covered by gitoxide's LICENSE-MIT / LICENSE-APACHE ({}):",
        manifest.workspace_members_same_attribution.len(),
    )?;
    for name in &manifest.workspace_members_same_attribution {
        writeln!(w, "  {name}")?;
    }
    Ok(())
}

/// Knobs the caller can pass to [`render_summary_with_options`] to control
/// what the summary view includes.
#[derive(Debug, Default, Clone, Copy)]
pub struct SummaryOptions {
    /// When `true`, include the same-attribution-workspace-members listing
    /// (the third group of names) alongside the third-party and
    /// workspace-with-separate-attribution sections. When `false` (the
    /// default), that listing is omitted — for many projects it is the
    /// largest of the three groups and clutters the at-a-glance summary
    /// with names a reader rarely needs to look at one by one.
    pub verbose: bool,
}

/// Render a column-aligned summary of every crate linked into this binary,
/// grouped into three sections: true third-party crates, gitoxide's own
/// workspace members that have separate attribution, and (under
/// [`SummaryOptions::verbose`]) the workspace members whose attribution
/// matches the root, listed by name.
pub fn render_summary_with_options(
    w: &mut (impl Write + ?Sized),
    manifest: &Manifest,
    opts: SummaryOptions,
) -> io::Result<()> {
    write_header(w, manifest)?;

    let (third_party, workspace_members) = partition_crates(manifest);

    writeln!(w, "Third-party crates linked into this binary ({}):", third_party.len())?;
    write_summary_table(w, &third_party)?;
    writeln!(w)?;
    writeln!(
        w,
        "Workspace members with separate attribution ({}):",
        workspace_members.len(),
    )?;
    write_summary_table(w, &workspace_members)?;
    if opts.verbose {
        write_same_attribution_workspace_section(w, manifest)?;
    }
    write_legend(w, manifest)?;

    writeln!(w)?;
    writeln!(
        w,
        "Use `gix licenses <CRATE>` or `ein licenses <CRATE>` for the full attribution of a single crate,"
    )?;
    writeln!(w, "or pass `--all` to print every license and notice in full.")?;
    Ok(())
}

/// Convenience wrapper around [`render_summary_with_options`] that uses
/// default options (no verbose listing of same-attribution workspace
/// members). Equivalent to passing [`SummaryOptions::default()`].
pub fn render_summary(w: &mut (impl Write + ?Sized), manifest: &Manifest) -> io::Result<()> {
    render_summary_with_options(w, manifest, SummaryOptions::default())
}

/// Write one crate's full attribution, including every license/notice file.
fn write_crate(w: &mut (impl Write + ?Sized), c: &CrateLicense) -> io::Result<()> {
    writeln!(w, "{} {}", c.name, c.version)?;
    let spdx = c.spdx.as_deref().unwrap_or("(none)");
    writeln!(w, "License: {spdx}")?;
    if let Some(repo) = &c.repository {
        writeln!(w, "Repository: {repo}")?;
    }
    if let Some(hp) = &c.homepage {
        writeln!(w, "Homepage: {hp}")?;
    }
    if !c.authors.is_empty() {
        writeln!(w, "Authors:")?;
        for a in &c.authors {
            writeln!(w, "  {a}")?;
        }
    }
    if c.used_spdx_fallback {
        writeln!(
            w,
            "Note: the license text below was sourced from a bundled canonical SPDX copy,"
        )?;
        writeln!(
            w,
            "      because no LICENSE/COPYING/NOTICE file was present in the crate source."
        )?;
    }
    writeln!(w)?;

    if c.files.is_empty() {
        writeln!(w, "(No license text available for this crate.)")?;
    } else {
        for (i, file) in c.files.iter().enumerate() {
            if i > 0 {
                writeln!(w, "{FILE_DIVIDER}")?;
            }
            writeln!(w, "-- {} --", file.name)?;
            writeln!(w)?;
            w.write_all(file.text.as_bytes())?;
            if !file.text.ends_with('\n') {
                writeln!(w)?;
            }
        }
    }
    Ok(())
}

/// Write the full root-license attribution for `name` — used both for
/// `name == "gitoxide"` (the root package itself) and for any same-
/// attribution workspace member, since both are covered by the same
/// LICENSE-MIT and LICENSE-APACHE files at the repository root.
///
/// The actual file contents come from [`spdx_texts::MIT`] and
/// [`spdx_texts::APACHE_2_0`], which `include_str!` the repository's
/// top-level files at compile time.
fn write_root_license_attribution(w: &mut (impl Write + ?Sized), name: &str) -> io::Result<()> {
    writeln!(w, "{name}")?;
    if name == "gitoxide" {
        writeln!(w, "License files for the gitoxide root package:")?;
    } else {
        writeln!(
            w,
            "Workspace member with license and authorship identical to the root `gitoxide` package.",
        )?;
        writeln!(
            w,
            "Covered by the gitoxide repository's LICENSE-MIT and LICENSE-APACHE files (shown below):"
        )?;
    }
    writeln!(w)?;

    let files: [(&str, &str); 2] = [
        ("LICENSE-MIT", spdx_texts::MIT),
        ("LICENSE-APACHE", spdx_texts::APACHE_2_0),
    ];
    for (i, (filename, body)) in files.iter().enumerate() {
        if i > 0 {
            writeln!(w, "{FILE_DIVIDER}")?;
        }
        writeln!(w, "-- {filename} --")?;
        writeln!(w)?;
        w.write_all(body.as_bytes())?;
        if !body.ends_with('\n') {
            writeln!(w)?;
        }
    }
    Ok(())
}

/// Render the full attribution for every entry whose name matches.
///
/// Resolution order:
///
/// 1. If [`Manifest::find_all`] returns one or more matches, write each
///    matching entry. The single-match case is the per-crate attribution
///    by itself; the multi-match case (cargo resolved multiple versions
///    of the same crate, possibly under different licenses) prints every
///    matching version separated by a divider, with a header
///    announcing the multi-version case so a reader does not mistake the
///    output for duplicated rendering.
/// 2. If the name is the root `gitoxide` package or a same-attribution
///    workspace member (per [`Manifest::is_covered_by_root_license`]),
///    write the gitoxide-root license attribution: the actual MIT and
///    Apache-2.0 text, plus a one-line note explaining that the named
///    crate is covered by these files.
/// 3. Otherwise return [`io::ErrorKind::NotFound`].
pub fn render_crate(w: &mut (impl Write + ?Sized), manifest: &Manifest, name: &str) -> io::Result<()> {
    let hits = manifest.find_all(name);
    match hits.len() {
        0 => {
            if manifest.is_covered_by_root_license(name) {
                return write_root_license_attribution(w, name);
            }
            Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("no dependency named {name:?} in the manifest"),
            ))
        }
        1 => write_crate(w, hits[0]),
        n => {
            writeln!(w, "`{name}` is linked into this binary in {n} versions:")?;
            writeln!(w)?;
            for (i, c) in hits.iter().enumerate() {
                if i > 0 {
                    writeln!(w, "{CRATE_DIVIDER}")?;
                    writeln!(w)?;
                }
                write_crate(w, c)?;
            }
            Ok(())
        }
    }
}

/// Render the full attribution for every crate, separated by dividers,
/// grouped into three sections — third-party crates, workspace-with-
/// separate-attribution, and the same-attribution workspace-member names
/// listing. The same-attribution section is always present (unlike in the
/// summary, where it is gated on [`SummaryOptions::verbose`]) — `render_all`
/// is the byte-for-byte form shipped as `THIRD-PARTY-LICENSES.txt` in
/// release archives, and gating any of its sections on a flag would make
/// that archived file non-deterministic.
pub fn render_all(w: &mut (impl Write + ?Sized), manifest: &Manifest) -> io::Result<()> {
    write_header(w, manifest)?;

    let (third_party, workspace_members) = partition_crates(manifest);

    writeln!(w, "Third-party crates linked into this binary ({}):", third_party.len())?;
    writeln!(w)?;
    for (i, c) in third_party.iter().enumerate() {
        if i > 0 {
            writeln!(w, "{CRATE_DIVIDER}")?;
            writeln!(w)?;
        }
        write_crate(w, c)?;
    }

    writeln!(w)?;
    writeln!(
        w,
        "Workspace members with separate attribution ({}):",
        workspace_members.len(),
    )?;
    writeln!(w)?;
    for (i, c) in workspace_members.iter().enumerate() {
        if i > 0 {
            writeln!(w, "{CRATE_DIVIDER}")?;
            writeln!(w)?;
        }
        write_crate(w, c)?;
    }

    write_same_attribution_workspace_section(w, manifest)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::licenses::types::{CrateLicense, LicenseFile};

    fn sample_manifest() -> Manifest {
        Manifest {
            crates: vec![
                CrateLicense {
                    name: "anyhow".into(),
                    version: "1.0.98".into(),
                    spdx: Some("MIT OR Apache-2.0".into()),
                    authors: vec!["David Tolnay <dtolnay@gmail.com>".into()],
                    repository: Some("https://github.com/dtolnay/anyhow".into()),
                    homepage: None,
                    files: vec![LicenseFile {
                        name: "LICENSE-MIT".into(),
                        text: "MIT body text\n".into(),
                    }],
                    used_spdx_fallback: false,
                    is_workspace_member: false,
                },
                CrateLicense {
                    name: "mpl-example".into(),
                    version: "0.1.0".into(),
                    spdx: Some("MPL-2.0".into()),
                    authors: vec![],
                    repository: None,
                    homepage: None,
                    files: vec![],
                    used_spdx_fallback: false,
                    is_workspace_member: false,
                },
                // A workspace member that needs its own attribution entry —
                // mirrors the real-world `gix-imara-diff` case (vendored
                // upstream Apache-2.0, different from gitoxide's root
                // MIT-or-Apache-2.0).
                CrateLicense {
                    name: "gix-imara-diff".into(),
                    version: "0.2.1".into(),
                    spdx: Some("Apache-2.0".into()),
                    authors: vec!["Pascal Kuthe".into()],
                    repository: Some("https://github.com/pascalkuthe/imara-diff".into()),
                    homepage: None,
                    files: vec![LicenseFile {
                        name: "LICENSE-APACHE".into(),
                        text: "Apache body text\n".into(),
                    }],
                    used_spdx_fallback: false,
                    is_workspace_member: true,
                },
            ],
            workspace_members_same_attribution: vec!["gix-alpha".into(), "gix-beta".into()],
            generated_at: "2026-04-15T00:00:00Z".into(),
            feature_profile: Some("max".into()),
            target_triple: "aarch64-apple-darwin".into(),
        }
    }

    fn render_to_string(manifest: &Manifest, f: impl Fn(&mut Vec<u8>, &Manifest) -> io::Result<()>) -> String {
        let mut buf = Vec::new();
        f(&mut buf, manifest).expect("render to a Vec<u8> is infallible except for the manifest content");
        String::from_utf8(buf).expect("render output is valid UTF-8")
    }

    #[test]
    fn summary_includes_header_and_listed_crates() {
        let out = render_to_string(&sample_manifest(), render_summary);
        assert!(out.contains("Feature profile: max"));
        assert!(out.contains("Target triple:   aarch64-apple-darwin"));
        // Per-section counts are written at section headers, not in the top
        // header. Verify the section-level counts are present and reasonable.
        assert!(
            out.contains("Third-party crates linked into this binary (2):"),
            "expected third-party section header with count 2:\n{out}"
        );
        assert!(
            out.contains("Workspace members with separate attribution (1):"),
            "expected separate-attribution section header with count 1:\n{out}"
        );
        assert!(out.contains("anyhow"));
        assert!(out.contains("mpl-example"));
        assert!(out.contains("gix-imara-diff"));
    }

    /// The notes column uses footnote-style marks (`[!]` for missing
    /// license text) rather than full-prose phrases, so the table stays
    /// compact even when many crates carry notes. Wide prose pushed the
    /// LICENSE column off-screen on standard 80-column terminals.
    #[test]
    fn summary_marks_crates_with_no_license_text() {
        let out = render_to_string(&sample_manifest(), render_summary);
        let mpl_line = out
            .lines()
            .find(|l| l.starts_with("mpl-example"))
            .expect("mpl-example row");
        assert!(
            mpl_line.contains("[!]"),
            "missing-text row should carry the [!] mark:\n{mpl_line}"
        );
        assert!(
            !mpl_line.contains("no license text available"),
            "footnote-style mark should replace literal text in the notes column:\n{mpl_line}"
        );
    }

    /// Crates with both their license file present and no SPDX fallback
    /// carry no marks in the notes column at all.
    #[test]
    fn summary_does_not_mark_present_license() {
        let out = render_to_string(&sample_manifest(), render_summary);
        let anyhow_line = out.lines().find(|l| l.starts_with("anyhow")).expect("anyhow row");
        assert!(
            !anyhow_line.contains("[!]"),
            "fully-attributed row should not carry the [!] mark:\n{anyhow_line}"
        );
        assert!(
            !anyhow_line.contains("[*]"),
            "row without an SPDX fallback should not carry the [*] mark:\n{anyhow_line}"
        );
    }

    /// SPDX-fallback crates are marked with `[*]` in the notes column. The
    /// canonical bundled text is what the per-crate view shows, but in the
    /// summary view a compact mark conveys the same diagnostic without
    /// stretching the table.
    #[test]
    fn summary_marks_spdx_fallback_with_asterisk() {
        let mut manifest = sample_manifest();
        // anyhow already has license text; here we hand-flag it as having
        // come from the SPDX fallback so the [*] codepath fires regardless
        // of the file-presence heuristic.
        manifest.crates[0].used_spdx_fallback = true;
        let out = render_to_string(&manifest, render_summary);
        let anyhow_line = out.lines().find(|l| l.starts_with("anyhow")).expect("anyhow row");
        assert!(
            anyhow_line.contains("[*]"),
            "SPDX-fallback row should carry the [*] mark:\n{anyhow_line}"
        );
        assert!(
            !anyhow_line.contains("bundled SPDX fallback"),
            "footnote-style mark should replace literal text in the notes column:\n{anyhow_line}"
        );
    }

    /// When at least one crate carries a footnote mark, the summary
    /// includes a legend block explaining what the marks mean. The legend
    /// avoids forcing readers to guess at `[*]`/`[!]` semantics on first
    /// encounter.
    #[test]
    fn summary_includes_legend_for_marks_in_use() {
        let mut manifest = sample_manifest();
        manifest.crates[0].used_spdx_fallback = true;
        let out = render_to_string(&manifest, render_summary);
        // The legend's [*] line introduces the SPDX-fallback meaning.
        let star_legend_pos = out
            .lines()
            .position(|l| l.trim_start().starts_with("[*]"))
            .unwrap_or_else(|| panic!("legend should explain [*]:\n{out}"));
        let bang_legend_pos = out
            .lines()
            .position(|l| l.trim_start().starts_with("[!]"))
            .unwrap_or_else(|| panic!("legend should explain [!]:\n{out}"));
        // Both legend lines should mention the diagnostic concept they
        // stand for so a reader can interpret the marks without deeper
        // documentation.
        let legend = out
            .lines()
            .skip(star_legend_pos.min(bang_legend_pos))
            .take(2 + (star_legend_pos.max(bang_legend_pos) - star_legend_pos.min(bang_legend_pos)))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            legend.to_lowercase().contains("spdx") || legend.to_lowercase().contains("bundled"),
            "[*] legend line should mention SPDX/bundled:\n{legend}"
        );
        assert!(
            legend.to_lowercase().contains("no license text") || legend.to_lowercase().contains("missing"),
            "[!] legend line should mention missing/no license text:\n{legend}"
        );
    }

    /// When no crate carries any mark, the legend block is omitted to
    /// avoid clutter. There is nothing for the legend to explain.
    #[test]
    fn summary_omits_legend_when_no_marks_used() {
        let manifest = Manifest {
            crates: vec![CrateLicense {
                name: "all-good".into(),
                version: "1.0.0".into(),
                spdx: Some("MIT".into()),
                authors: vec![],
                repository: None,
                homepage: None,
                files: vec![LicenseFile {
                    name: "LICENSE-MIT".into(),
                    text: "MIT body\n".into(),
                }],
                used_spdx_fallback: false,
                is_workspace_member: false,
            }],
            workspace_members_same_attribution: vec![],
            generated_at: "t".into(),
            feature_profile: None,
            target_triple: "x86_64-unknown-linux-gnu".into(),
        };
        let out = render_to_string(&manifest, render_summary);
        assert!(
            !out.contains("[*]"),
            "no [*] mark should appear when no crate has the SPDX fallback:\n{out}"
        );
        assert!(
            !out.contains("[!]"),
            "no [!] mark should appear when every crate has license text:\n{out}"
        );
    }

    /// The legend appears after the data tables but BEFORE the footer
    /// guidance ("Use `gix licenses <CRATE>` …"), so the help text
    /// remains the very last thing on screen as the user requested.
    #[test]
    fn legend_appears_above_footer_guidance() {
        let mut manifest = sample_manifest();
        manifest.crates[0].used_spdx_fallback = true;
        let out = render_to_string(&manifest, render_summary);
        let legend_pos = out
            .find("[*]")
            .unwrap_or_else(|| panic!("legend should be present:\n{out}"));
        let footer_pos = out
            .find("Use `gix licenses <CRATE>`")
            .unwrap_or_else(|| panic!("footer should be present:\n{out}"));
        assert!(
            legend_pos < footer_pos,
            "legend should precede the footer guidance:\n{out}"
        );
    }

    /// The legend scans every section's crates: a footnote mark on a
    /// workspace-with-separate-attribution crate should still surface the
    /// legend even if no third-party crate uses any mark.
    #[test]
    fn legend_fires_for_marks_in_workspace_section_too() {
        let mut manifest = sample_manifest();
        // Clear the third-party flag and only flag the workspace member.
        for c in &mut manifest.crates {
            c.used_spdx_fallback = false;
        }
        let workspace_idx = manifest
            .crates
            .iter()
            .position(|c| c.is_workspace_member)
            .expect("sample manifest has a workspace member");
        manifest.crates[workspace_idx].used_spdx_fallback = true;
        let out = render_to_string(&manifest, render_summary);
        assert!(
            out.contains("[*]"),
            "workspace-section mark should still trigger the legend:\n{out}"
        );
    }

    #[test]
    fn crate_full_attribution_shows_file_text() {
        let manifest = sample_manifest();
        let mut buf = Vec::new();
        render_crate(&mut buf, &manifest, "anyhow").unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("anyhow 1.0.98"));
        assert!(s.contains("License: MIT OR Apache-2.0"));
        assert!(s.contains("Repository: https://github.com/dtolnay/anyhow"));
        assert!(s.contains("David Tolnay"));
        assert!(s.contains("-- LICENSE-MIT --"));
        assert!(s.contains("MIT body text"));
    }

    #[test]
    fn crate_missing_returns_not_found() {
        let err = render_crate(&mut Vec::new(), &sample_manifest(), "absent").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn crate_with_no_files_still_renders_header() {
        let mut buf = Vec::new();
        render_crate(&mut buf, &sample_manifest(), "mpl-example").unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("mpl-example 0.1.0"));
        assert!(s.contains("License: MPL-2.0"));
        assert!(s.contains("(No license text available"));
    }

    #[test]
    fn render_all_includes_dividers_between_crates() {
        let out = render_to_string(&sample_manifest(), render_all);
        assert!(out.contains(CRATE_DIVIDER), "should separate crates with divider");
        let first_pos = out.find("anyhow").unwrap();
        let second_pos = out.find("mpl-example").unwrap();
        assert!(first_pos < second_pos, "crates should appear in manifest order");
    }

    #[test]
    fn render_all_on_empty_manifest_has_just_header_and_empty_section_headers() {
        let manifest = Manifest {
            crates: vec![],
            workspace_members_same_attribution: vec![],
            generated_at: "t".into(),
            feature_profile: None,
            target_triple: "x86_64-unknown-linux-gnu".into(),
        };
        let out = render_to_string(&manifest, render_all);
        assert!(
            out.contains("Third-party crates linked into this binary (0):"),
            "empty manifest still announces the third-party section with count 0:\n{out}"
        );
        assert!(
            out.contains("Workspace members with separate attribution (0):"),
            "empty manifest still announces the separate-attribution section with count 0:\n{out}"
        );
        assert!(
            !out.contains("Workspace members covered by"),
            "same-attribution section should be absent when the list is empty"
        );
        assert!(!out.contains(CRATE_DIVIDER));
    }

    #[test]
    fn summary_lists_same_attribution_workspace_members_when_verbose() {
        // The same-attribution-workspace-members listing is gated on the
        // verbose flag in the summary view; with the flag set the listing
        // appears verbatim (with its count, name lines, and section header).
        let mut buf = Vec::new();
        render_summary_with_options(&mut buf, &sample_manifest(), SummaryOptions { verbose: true }).expect("render");
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("Workspace members covered by gitoxide's LICENSE-MIT / LICENSE-APACHE (2):"),
            "verbose summary should introduce the same-attribution section with its count:\n{out}",
        );
        assert!(
            out.contains("gix-alpha"),
            "verbose summary should list gix-alpha:\n{out}"
        );
        assert!(out.contains("gix-beta"), "verbose summary should list gix-beta:\n{out}");
    }

    #[test]
    fn summary_omits_same_attribution_names_by_default() {
        // Without verbose, the default summary omits the same-attribution
        // names listing entirely — its own section header included. This is
        // the behavior `gix licenses` (no further arguments) produces.
        let out = render_to_string(&sample_manifest(), render_summary);
        assert!(
            !out.contains("Workspace members covered by"),
            "default summary should not include the same-attribution section header:\n{out}"
        );
        assert!(
            !out.contains("gix-alpha"),
            "default summary should not list gix-alpha:\n{out}"
        );
        assert!(
            !out.contains("gix-beta"),
            "default summary should not list gix-beta:\n{out}"
        );
        // The third-party and separate-attribution sections still appear.
        assert!(out.contains("Third-party crates linked into this binary"));
        assert!(out.contains("Workspace members with separate attribution"));
    }

    #[test]
    fn render_all_lists_same_attribution_workspace_members() {
        let out = render_to_string(&sample_manifest(), render_all);
        assert!(
            out.contains("Workspace members covered by gitoxide's LICENSE-MIT / LICENSE-APACHE (2):"),
            "render_all should include the same-attribution section with its count:\n{out}",
        );
        assert!(out.contains("gix-alpha"));
        assert!(out.contains("gix-beta"));
    }

    #[test]
    fn same_attribution_section_absent_when_list_is_empty() {
        let mut manifest = sample_manifest();
        manifest.workspace_members_same_attribution.clear();
        let out = render_to_string(&manifest, render_summary);
        assert!(
            !out.contains("Workspace members covered by"),
            "same-attribution section should be absent when its list is empty"
        );
    }

    /// Same-attribution workspace members must show the actual MIT and
    /// Apache-2.0 license text from the gitoxide repository root, not just
    /// a pointer to the files. A reader running `gix licenses gix-sec`
    /// should be able to read the licenses without leaving the binary.
    #[test]
    fn render_crate_for_same_attribution_includes_real_license_text() {
        let manifest = sample_manifest();
        let mut buf = Vec::new();
        render_crate(&mut buf, &manifest, "gix-alpha").expect("same-attribution ws member must render");
        let s = String::from_utf8(buf).unwrap();

        assert!(s.contains("gix-alpha"), "output should name the crate:\n{s}");

        // Canonical MIT permission clause body — distinguishes the actual
        // LICENSE-MIT contents from a mere pointer at it.
        assert!(
            s.contains("Permission is hereby granted"),
            "MIT license body should appear inline:\n{s}"
        );
        // Canonical Apache-2.0 marker.
        assert!(
            s.contains("Apache License") && s.contains("Version 2.0"),
            "Apache-2.0 license body should appear inline:\n{s}"
        );

        // The output should still be labeled as the root-license case so a
        // reader knows why these files apply.
        assert!(
            s.to_lowercase().contains("gitoxide"),
            "output should mention gitoxide so a reader knows whose root license covers gix-alpha:\n{s}"
        );
    }

    /// `gix licenses gitoxide` should produce the gitoxide root package's
    /// license attribution, even though the root is not stored in
    /// `manifest.crates`. The same MIT + Apache-2.0 text the same-attribution
    /// path uses is the right answer here too: gitoxide is itself covered by
    /// those files.
    #[test]
    fn render_crate_for_root_package_name_includes_real_license_text() {
        let manifest = sample_manifest();
        let mut buf = Vec::new();
        render_crate(&mut buf, &manifest, "gitoxide").expect("`gitoxide` should be queryable");
        let s = String::from_utf8(buf).unwrap();

        assert!(s.contains("gitoxide"), "output should name the package:\n{s}");
        assert!(
            s.contains("Permission is hereby granted"),
            "MIT license body should appear inline for the root:\n{s}"
        );
        assert!(
            s.contains("Apache License") && s.contains("Version 2.0"),
            "Apache-2.0 license body should appear inline for the root:\n{s}"
        );
    }

    #[test]
    fn render_crate_still_not_found_for_unknown_name() {
        let manifest = sample_manifest();
        let err = render_crate(&mut Vec::new(), &manifest, "nonexistent-xyz").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    /// Build a small manifest containing two distinct versions of the same
    /// crate `dual-version` under different license expressions. Used by
    /// the multi-version-handling tests.
    fn manifest_with_two_versions_of(name: &str, spdx_a: &str, spdx_b: &str) -> Manifest {
        let mk = |version: &str, spdx: &str, body: &str| CrateLicense {
            name: name.to_string(),
            version: version.into(),
            spdx: Some(spdx.into()),
            authors: vec![],
            repository: None,
            homepage: None,
            files: vec![LicenseFile {
                name: format!("LICENSE-{version}"),
                text: format!("{body}\n"),
            }],
            used_spdx_fallback: false,
            is_workspace_member: false,
        };
        Manifest {
            crates: vec![mk("1.0.0", spdx_a, "v1 body"), mk("2.0.0", spdx_b, "v2 body")],
            workspace_members_same_attribution: Vec::new(),
            generated_at: "t".into(),
            feature_profile: None,
            target_triple: "x86_64-unknown-linux-gnu".into(),
        }
    }

    #[test]
    fn render_crate_shows_all_versions_when_name_is_ambiguous() {
        let manifest = manifest_with_two_versions_of("dual-version", "MIT", "Apache-2.0");
        let mut buf = Vec::new();
        render_crate(&mut buf, &manifest, "dual-version").unwrap();
        let s = String::from_utf8(buf).unwrap();

        // Both versions must appear.
        assert!(s.contains("dual-version 1.0.0"), "v1.0.0 missing:\n{s}");
        assert!(s.contains("dual-version 2.0.0"), "v2.0.0 missing:\n{s}");
        // Both license expressions must appear.
        assert!(s.contains("License: MIT"), "MIT line missing:\n{s}");
        assert!(s.contains("License: Apache-2.0"), "Apache-2.0 line missing:\n{s}");
        // Both license texts must appear.
        assert!(s.contains("v1 body"), "v1 license body missing:\n{s}");
        assert!(s.contains("v2 body"), "v2 license body missing:\n{s}");
        // The output should clearly indicate that this name has multiple
        // versions, so a reader doesn't think they're seeing duplicate output.
        assert!(
            s.contains("2 versions") || s.contains("two versions"),
            "expected an indication that the name resolves to multiple versions:\n{s}"
        );
    }

    #[test]
    fn render_crate_single_match_renders_one_entry_only() {
        // Single-match path should not gain a "multi versions" header.
        let manifest = sample_manifest();
        let mut buf = Vec::new();
        render_crate(&mut buf, &manifest, "anyhow").unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("anyhow 1.0.98"));
        assert!(
            !s.contains("versions"),
            "single-match output should not announce multiple versions:\n{s}"
        );
    }

    /// The summary view groups crates into three categories: third-party
    /// crates (sourced from elsewhere), gitoxide workspace members whose
    /// license or authorship differs from the root and so need their own
    /// attribution entry, and gitoxide workspace members whose attribution
    /// matches the root's (recorded by name only). Each category gets its
    /// own section header so a reader can tell at a glance which crates
    /// in the binary are third-party and which come from gitoxide itself.
    ///
    /// Verbose mode is used so the same-attribution section is present and
    /// the three-section ordering can be checked end-to-end. The default
    /// summary's omission of that section is verified separately by
    /// [`summary_omits_same_attribution_names_by_default`].
    #[test]
    fn summary_separates_third_party_from_workspace_member_attribution() {
        let mut buf = Vec::new();
        render_summary_with_options(&mut buf, &sample_manifest(), SummaryOptions { verbose: true })
            .expect("render to a Vec<u8> is infallible except for the manifest content");
        let out = String::from_utf8(buf).expect("render output is valid UTF-8");

        let third_party_pos = out
            .find("Third-party crates")
            .unwrap_or_else(|| panic!("expected a 'Third-party crates' section header in:\n{out}"));
        let separate_attrib_pos = out
            .find("Workspace members with separate attribution")
            .unwrap_or_else(|| panic!("expected a separate-attribution section header in:\n{out}"));
        let same_attrib_pos = out
            .find("Workspace members covered by")
            .unwrap_or_else(|| panic!("expected a same-attribution section header in:\n{out}"));

        assert!(
            third_party_pos < separate_attrib_pos,
            "third-party section must precede separate-attribution workspace section"
        );
        assert!(
            separate_attrib_pos < same_attrib_pos,
            "separate-attribution section must precede same-attribution section"
        );

        // Third-party slice must list anyhow and mpl-example but NOT gix-imara-diff
        // (which is a workspace member with separate attribution, not a
        // third-party crate).
        let third_party_slice = &out[third_party_pos..separate_attrib_pos];
        assert!(
            third_party_slice.contains("anyhow"),
            "anyhow missing from third-party section"
        );
        assert!(
            third_party_slice.contains("mpl-example"),
            "mpl-example missing from third-party section"
        );
        assert!(
            !third_party_slice.contains("gix-imara-diff"),
            "gix-imara-diff is a workspace member; must not appear in third-party section"
        );

        // Separate-attribution slice must list gix-imara-diff and not
        // anyhow/mpl-example.
        let separate_slice = &out[separate_attrib_pos..same_attrib_pos];
        assert!(
            separate_slice.contains("gix-imara-diff"),
            "gix-imara-diff missing from separate-attribution workspace section"
        );
        assert!(
            !separate_slice.contains("anyhow"),
            "anyhow is third-party; must not appear in workspace-member section"
        );

        // Same-attribution slice must list gix-alpha and gix-beta.
        let same_slice = &out[same_attrib_pos..];
        assert!(same_slice.contains("gix-alpha"));
        assert!(same_slice.contains("gix-beta"));
    }

    #[test]
    fn render_all_separates_third_party_from_workspace_member_attribution() {
        let out = render_to_string(&sample_manifest(), render_all);

        let third_party_pos = out
            .find("Third-party crates")
            .unwrap_or_else(|| panic!("expected a 'Third-party crates' section header in:\n{out}"));
        let separate_attrib_pos = out
            .find("Workspace members with separate attribution")
            .unwrap_or_else(|| panic!("expected a separate-attribution section header in:\n{out}"));

        // anyhow's full attribution should be in the third-party slice; gix-imara-diff's
        // full attribution should be in the separate-attribution slice.
        let third_party_slice = &out[third_party_pos..separate_attrib_pos];
        assert!(
            third_party_slice.contains("MIT body text"),
            "anyhow's license text should appear under third-party"
        );
        let separate_slice = &out[separate_attrib_pos..];
        assert!(
            separate_slice.contains("Apache body text"),
            "gix-imara-diff's license text should appear under separate-attribution workspace section"
        );
    }
}
