//! End-to-end checks of the `gix licenses` subcommand's output shape under
//! different flag combinations. The unit tests in
//! `gitoxide-core/src/licenses/render.rs` and `src/licenses/cli.rs` already
//! cover the rendering and JSON-emission logic against hand-crafted
//! manifests; this file exercises the same surface against the actual built
//! `gix` binary so the embedded manifest, the clap argument parser, and the
//! summary/`--all`/single-crate dispatch are all in the loop.
//!
//! Each test runs the binary as a subprocess via `CARGO_BIN_EXE_gix` (the
//! same env var the existing `licenses_parity.rs` and
//! `licenses_workspace_attribution.rs` integration tests use), captures its
//! stdout, and asserts on string-level invariants. We deliberately avoid
//! asserting on specific crate names from the real manifest — those would
//! drift as dependencies change — and instead assert on section headers and
//! help text, which are part of the CLI's stable contract.

use std::process::Command;

/// Run `gix licenses` with the given arguments and return stdout as a String.
/// Asserts the subprocess exited successfully — a non-zero exit is a hard
/// failure of the contract under test.
fn run_gix_licenses(args: &[&str]) -> String {
    let gix = env!("CARGO_BIN_EXE_gix");
    let mut cmd = Command::new(gix);
    cmd.arg("licenses");
    cmd.args(args);
    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("run `gix licenses {args:?}`: {e}"));
    assert!(
        output.status.success(),
        "`gix licenses {args:?}` failed ({}):\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).expect("stdout is valid UTF-8")
}

/// `gix licenses` with no arguments must show the third-party and
/// workspace-with-separate-attribution sections, and must NOT show the
/// same-attribution names listing — that listing is the largest of the
/// three groups for `gitoxide` and would clutter the at-a-glance summary.
#[test]
fn default_summary_omits_same_attribution_section() {
    let out = run_gix_licenses(&[]);
    assert!(
        out.contains("Third-party crates linked into this binary"),
        "default summary should include the third-party section header:\n{out}"
    );
    assert!(
        out.contains("Workspace members with separate attribution"),
        "default summary should include the separate-attribution section header:\n{out}"
    );
    assert!(
        !out.contains("Workspace members covered by"),
        "default summary should not include the same-attribution section header:\n{out}"
    );
}

/// `gix licenses --verbose` reverses the omission above: every section
/// header (including the same-attribution names listing) appears, in the
/// canonical order — third-party, separate attribution, then same
/// attribution.
#[test]
fn verbose_summary_includes_same_attribution_section() {
    let out = run_gix_licenses(&["--verbose"]);
    let third_party = out
        .find("Third-party crates linked into this binary")
        .unwrap_or_else(|| panic!("verbose summary missing third-party header:\n{out}"));
    let separate = out
        .find("Workspace members with separate attribution")
        .unwrap_or_else(|| panic!("verbose summary missing separate-attribution header:\n{out}"));
    let same = out
        .find("Workspace members covered by")
        .unwrap_or_else(|| panic!("verbose summary missing same-attribution header:\n{out}"));
    assert!(
        third_party < separate && separate < same,
        "verbose summary sections must be in canonical order \
         (third-party before separate-attribution before same-attribution):\n{out}",
    );
}

/// The short form `-v` must be wired to the same flag as `--verbose`.
#[test]
fn short_verbose_flag_matches_long_form() {
    let long = run_gix_licenses(&["--verbose"]);
    let short = run_gix_licenses(&["-v"]);
    assert_eq!(long, short, "`-v` and `--verbose` should produce byte-identical output",);
}

/// `gix licenses --all` always includes every section, regardless of
/// `--verbose`. The full attribution view is exhaustive by construction;
/// gating one of its sections on a flag would make the file shipped as
/// `THIRD-PARTY-LICENSES.txt` non-deterministic.
#[test]
fn all_includes_same_attribution_section_without_verbose() {
    let out = run_gix_licenses(&["--all"]);
    assert!(
        out.contains("Workspace members covered by"),
        "`--all` must include the same-attribution section unconditionally:\n\
         (head of output below)\n{}",
        out.lines().take(40).collect::<Vec<_>>().join("\n"),
    );
}

/// `--verbose` is a no-op when combined with `--all` — `--all` already
/// implies the full attribution. Both invocations must produce identical
/// output, since the full attribution view is byte-deterministic for a
/// given build.
#[test]
fn verbose_has_no_effect_when_all_is_set() {
    let plain = run_gix_licenses(&["--all"]);
    let verbose = run_gix_licenses(&["--all", "--verbose"]);
    assert_eq!(
        plain, verbose,
        "`--all` and `--all --verbose` should produce byte-identical output",
    );
}

/// Querying a single crate by name does not consult `--verbose`. The flag
/// only governs the summary view's same-attribution section. The single-
/// crate output should be identical with or without `--verbose`.
#[test]
fn verbose_has_no_effect_when_a_crate_name_is_given() {
    // Pick a crate that is reliably present in any build of `gix`: the
    // root package itself is queryable via the same-attribution path.
    let plain = run_gix_licenses(&["gitoxide"]);
    let verbose = run_gix_licenses(&["--verbose", "gitoxide"]);
    assert_eq!(
        plain, verbose,
        "single-crate output must not be affected by `--verbose`",
    );
}

/// The summary's footer guidance — "use `gix licenses <CRATE>` …" — must
/// appear at the bottom of the output, after every section. A reader
/// scrolling through long lists (the third-party section in particular)
/// expects "what to do next" to land at the end, not buried mid-output
/// where it's easy to miss.
#[test]
fn summary_footer_appears_after_all_sections() {
    let out = run_gix_licenses(&["--verbose"]);
    let third_party = out
        .find("Third-party crates linked into this binary")
        .expect("third-party header present");
    let separate = out
        .find("Workspace members with separate attribution")
        .expect("separate-attribution header present");
    let same = out
        .find("Workspace members covered by")
        .expect("same-attribution header present");
    let footer = out
        .find("Use `gix licenses <CRATE>`")
        .unwrap_or_else(|| panic!("summary should include footer guidance:\n{out}"));
    assert!(
        third_party < footer && separate < footer && same < footer,
        "footer guidance must come after every section header:\n{out}"
    );
}

/// The summary's notes column must not carry the verbose pre-footnote
/// phrasing ("bundled SPDX fallback", "no license text available").
/// Compact `[*]`/`[!]` marks plus a legend is the new contract.
#[test]
fn summary_notes_column_uses_footnote_marks_not_verbose_phrases() {
    let out = run_gix_licenses(&[]);
    assert!(
        !out.contains("bundled SPDX fallback"),
        "verbose SPDX-fallback prose must not appear in the summary table:\n{out}"
    );
    assert!(
        !out.contains("no license text available"),
        "verbose missing-text prose must not appear in the summary table:\n{out}"
    );
}

/// Whenever a footnote mark (`[*]` or `[!]`) appears in the summary, a
/// legend defining what that mark means must also appear — and the legend
/// must precede the footer guidance so that the help text remains the
/// last thing on screen.
///
/// When neither mark is present in the actual built manifest (likely the
/// common case), the legend is omitted and this assertion holds
/// vacuously. Either way, the output must be self-explanatory.
#[test]
fn legend_accompanies_any_footnote_mark_used_and_precedes_footer() {
    let out = run_gix_licenses(&[]);
    let footer_pos = out
        .find("Use `gix licenses <CRATE>`")
        .unwrap_or_else(|| panic!("summary should include footer guidance:\n{out}"));
    // Slice the body that precedes the footer; mark and legend both have
    // to live in this region.
    let head = &out[..footer_pos];

    if head.contains("[*]") {
        assert!(
            head.lines().any(|l| {
                let t = l.trim_start();
                t.starts_with("[*]") && (t.to_lowercase().contains("spdx") || t.to_lowercase().contains("bundled"))
            }),
            "[*] mark used but no legend line explaining it (above footer):\n{head}"
        );
    }
    if head.contains("[!]") {
        assert!(
            head.lines().any(|l| {
                let t = l.trim_start();
                t.starts_with("[!]")
                    && (t.to_lowercase().contains("no license text") || t.to_lowercase().contains("missing"))
            }),
            "[!] mark used but no legend line explaining it (above footer):\n{head}"
        );
    }
}
