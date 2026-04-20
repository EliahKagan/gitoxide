//! Rendering of the third-party dependency manifest for human consumption.
//!
//! Three outputs are offered, all writing into an arbitrary [`Write`] sink:
//!
//! * [`render_summary`] — a header and a column-aligned table listing every
//!   third-party crate, its version, its declared SPDX expression, and a
//!   note column indicating crates that have no license text embedded.
//! * [`render_crate`] — one crate's attribution in full: version, SPDX,
//!   repository/homepage, authors, and every LICENSE / NOTICE / AUTHORS /
//!   COPYRIGHT / COPYING file found in its source tree.
//! * [`render_all`] — header plus [`render_crate`] for every crate, separated
//!   by dividers. This is the canonical form that is also emitted to
//!   `THIRD-PARTY-LICENSES.txt` in release archives, so the subcommand output
//!   and the archive file match byte-for-byte for a given build.
//!
//! None of these functions allocate a full string copy of the manifest; they
//! stream formatted output through the [`Write`] sink.

use std::io::{self, Write};

use super::types::{CrateLicense, Manifest};

const CRATE_DIVIDER: &str = "================================================================";
const FILE_DIVIDER: &str = "----------------------------------------------------------------";

/// Write the manifest header describing which feature profile, target, and
/// generation timestamp the manifest corresponds to.
fn write_header(w: &mut (impl Write + ?Sized), manifest: &Manifest) -> io::Result<()> {
    writeln!(w, "gitoxide third-party dependencies")?;
    let profile = manifest.feature_profile.as_deref().unwrap_or("(custom features)");
    writeln!(w, "Feature profile: {profile}")?;
    writeln!(w, "Target triple:   {}", manifest.target_triple)?;
    writeln!(w, "Generated at:    {}", manifest.generated_at)?;
    writeln!(w, "Dependency count: {}", manifest.crates.len())?;
    writeln!(
        w,
        "Workspace members with gitoxide-equivalent attribution: {}",
        manifest.workspace_members_same_attribution.len(),
    )?;
    writeln!(w)?;
    Ok(())
}

/// Write the section listing workspace members whose attribution matches
/// the root package's, if any. The caller decides where to place this —
/// at the end of [`render_summary`] and [`render_all`] output.
fn write_same_attribution_workspace_section(w: &mut (impl Write + ?Sized), manifest: &Manifest) -> io::Result<()> {
    if manifest.workspace_members_same_attribution.is_empty() {
        return Ok(());
    }
    writeln!(w)?;
    writeln!(w, "Workspace members with the same license and authorship as gitoxide")?;
    writeln!(
        w,
        "(covered by the repository's LICENSE-MIT / LICENSE-APACHE at the root):"
    )?;
    for name in &manifest.workspace_members_same_attribution {
        writeln!(w, "  {name}")?;
    }
    Ok(())
}

/// Render a column-aligned summary table of all third-party dependencies.
pub fn render_summary(w: &mut (impl Write + ?Sized), manifest: &Manifest) -> io::Result<()> {
    write_header(w, manifest)?;

    let name_width = manifest
        .crates
        .iter()
        .map(|c| c.name.len())
        .max()
        .unwrap_or(4)
        .max("NAME".len());
    let version_width = manifest
        .crates
        .iter()
        .map(|c| c.version.len())
        .max()
        .unwrap_or(7)
        .max("VERSION".len());
    let spdx_width = manifest
        .crates
        .iter()
        .map(|c| c.spdx.as_deref().unwrap_or("(none)").len())
        .max()
        .unwrap_or(7)
        .max("LICENSE".len());

    writeln!(
        w,
        "{name:<name_width$}  {version:<version_width$}  {spdx:<spdx_width$}  NOTES",
        name = "NAME",
        version = "VERSION",
        spdx = "LICENSE",
    )?;

    for c in &manifest.crates {
        let spdx = c.spdx.as_deref().unwrap_or("(none)");
        let mut notes = Vec::new();
        if c.used_spdx_fallback {
            notes.push("bundled SPDX fallback");
        }
        if c.is_missing_text() {
            notes.push("no license text available");
        }
        writeln!(
            w,
            "{name:<name_width$}  {version:<version_width$}  {spdx:<spdx_width$}  {notes}",
            name = c.name,
            version = c.version,
            spdx = spdx,
            notes = notes.join(", "),
        )?;
    }

    writeln!(w)?;
    writeln!(
        w,
        "Use `gix licenses <CRATE>` or `ein licenses <CRATE>` for the full attribution of a single crate,"
    )?;
    writeln!(w, "or pass `--all` to print every license and notice in full.")?;
    write_same_attribution_workspace_section(w, manifest)?;
    Ok(())
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

/// Render one crate's full attribution, looked up by name.
///
/// If the named crate has a full entry in the manifest, print it. If the
/// name matches a workspace member whose attribution is identical to the
/// root `gitoxide` package's, print a short note pointing at the root's
/// license files rather than duplicating them. Otherwise return an error
/// of kind [`io::ErrorKind::NotFound`].
pub fn render_crate(w: &mut (impl Write + ?Sized), manifest: &Manifest, name: &str) -> io::Result<()> {
    if let Some(c) = manifest.find(name) {
        return write_crate(w, c);
    }
    if manifest.workspace_members_same_attribution.iter().any(|n| n == name) {
        writeln!(w, "{name}")?;
        writeln!(w, "Workspace member with attribution identical to `gitoxide`.")?;
        writeln!(
            w,
            "License, copyright, and attribution are covered by the LICENSE-MIT and",
        )?;
        writeln!(
            w,
            "LICENSE-APACHE files at the root of the gitoxide source tree and release archive.",
        )?;
        return Ok(());
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        format!("no dependency named {name:?} in the manifest"),
    ))
}

/// Render the full attribution for every crate, separated by dividers.
///
/// This is the byte-for-byte form shipped as `THIRD-PARTY-LICENSES.txt` in
/// release archives.
pub fn render_all(w: &mut (impl Write + ?Sized), manifest: &Manifest) -> io::Result<()> {
    write_header(w, manifest)?;
    for (i, c) in manifest.crates.iter().enumerate() {
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
    fn summary_includes_header_and_both_crates() {
        let out = render_to_string(&sample_manifest(), render_summary);
        assert!(out.contains("Feature profile: max"));
        assert!(out.contains("Target triple:   aarch64-apple-darwin"));
        assert!(out.contains("Dependency count: 2"));
        assert!(out.contains("anyhow"));
        assert!(out.contains("mpl-example"));
    }

    #[test]
    fn summary_marks_crates_with_no_license_text() {
        let out = render_to_string(&sample_manifest(), render_summary);
        let mpl_line = out
            .lines()
            .find(|l| l.starts_with("mpl-example"))
            .expect("mpl-example row");
        assert!(mpl_line.contains("no license text available"));
    }

    #[test]
    fn summary_does_not_mark_present_license() {
        let out = render_to_string(&sample_manifest(), render_summary);
        let anyhow_line = out.lines().find(|l| l.starts_with("anyhow")).expect("anyhow row");
        assert!(!anyhow_line.contains("no license text available"));
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
    fn render_all_on_empty_manifest_has_just_header() {
        let manifest = Manifest {
            crates: vec![],
            workspace_members_same_attribution: vec![],
            generated_at: "t".into(),
            feature_profile: None,
            target_triple: "x86_64-unknown-linux-gnu".into(),
        };
        let out = render_to_string(&manifest, render_all);
        assert!(out.contains("Dependency count: 0"));
        assert!(!out.contains(CRATE_DIVIDER));
    }

    #[test]
    fn summary_lists_same_attribution_workspace_members() {
        let out = render_to_string(&sample_manifest(), render_summary);
        assert!(
            out.contains("Workspace members with the same license and authorship as gitoxide"),
            "summary should introduce the same-attribution section:\n{out}",
        );
        assert!(out.contains("gix-alpha"), "summary should list gix-alpha:\n{out}");
        assert!(out.contains("gix-beta"), "summary should list gix-beta:\n{out}");
    }

    #[test]
    fn render_all_lists_same_attribution_workspace_members() {
        let out = render_to_string(&sample_manifest(), render_all);
        assert!(
            out.contains("Workspace members with the same license and authorship as gitoxide"),
            "render_all should include the same-attribution section",
        );
        assert!(out.contains("gix-alpha"));
        assert!(out.contains("gix-beta"));
    }

    #[test]
    fn header_reports_same_attribution_workspace_count() {
        let out = render_to_string(&sample_manifest(), render_summary);
        assert!(
            out.contains("Workspace members with gitoxide-equivalent attribution: 2"),
            "header must report the count:\n{out}",
        );
    }

    #[test]
    fn same_attribution_section_absent_when_list_is_empty() {
        let mut manifest = sample_manifest();
        manifest.workspace_members_same_attribution.clear();
        let out = render_to_string(&manifest, render_summary);
        assert!(!out.contains("Workspace members with the same license"));
        assert!(out.contains("Workspace members with gitoxide-equivalent attribution: 0"));
    }

    #[test]
    fn render_crate_finds_same_attribution_workspace_member() {
        let manifest = sample_manifest();
        let mut buf = Vec::new();
        render_crate(&mut buf, &manifest, "gix-alpha").expect("same-attribution ws member must render");
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("gix-alpha"), "output should name the crate");
        assert!(
            s.contains("identical to `gitoxide`"),
            "output should note the identical-attribution case:\n{s}",
        );
        assert!(
            s.contains("LICENSE-MIT") && s.contains("LICENSE-APACHE"),
            "output should point at the root license files:\n{s}",
        );
    }

    #[test]
    fn render_crate_still_not_found_for_unknown_name() {
        let manifest = sample_manifest();
        let err = render_crate(&mut Vec::new(), &manifest, "nonexistent-xyz").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
