//! End-to-end checks of the `gix licenses` (and where applicable
//! `ein licenses`) subcommand's output shape under different flag
//! combinations. The unit tests in `gitoxide-core/src/licenses/render.rs`
//! and `src/licenses/cli.rs` already cover the rendering and JSON-emission
//! logic against hand-crafted manifests; this file exercises the same
//! surface against the actually built binaries so the embedded manifest,
//! the clap argument parser, and the summary/`--all`/single-crate dispatch
//! are all in the loop.
//!
//! Each test runs a binary as a subprocess via `CARGO_BIN_EXE_gix` /
//! `CARGO_BIN_EXE_ein` (the same env var the existing `licenses_parity.rs`
//! and `licenses_workspace_attribution.rs` integration tests use), captures
//! both stdout and (for error-path tests) stderr and exit status, and
//! asserts on string-level invariants. We deliberately avoid asserting on
//! specific third-party crate names from the real manifest — those would
//! drift as dependencies change — and instead assert on section headers,
//! synthesized-entry contents, exit codes, and help text, which are part
//! of the CLI's stable contract.

use std::process::{Command, Output};

/// Run `gix licenses` with the given arguments and return stdout as a String.
/// Asserts the subprocess exited successfully — a non-zero exit is a hard
/// failure of the contract under test.
fn run_gix_licenses(args: &[&str]) -> String {
    let output = run_licenses_raw(env!("CARGO_BIN_EXE_gix"), args);
    assert!(
        output.status.success(),
        "`gix licenses {args:?}` failed ({}):\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout).expect("stdout is valid UTF-8")
}

/// Like [`run_gix_licenses`] but doesn't assert on exit status — used by
/// tests that pin error-path behavior (non-zero exit, error text on stderr).
fn run_licenses_raw(binary: &str, args: &[&str]) -> Output {
    let mut cmd = Command::new(binary);
    cmd.arg("licenses");
    cmd.args(args);
    cmd.output()
        .unwrap_or_else(|e| panic!("run `{binary} licenses {args:?}`: {e}"))
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

// ---------------------------------------------------------------------------
// `ein licenses` parity — the two binaries share `licenses::cli::Command` and
// `licenses::cli::run`, so the surface should be identical. The unit tests
// only ever drive the gix binary, so a future build-time divergence (e.g. a
// feature gate that drops the subcommand from one of the two binaries)
// would slip through without an end-to-end check on `ein` too.

/// `ein licenses` and `gix licenses` should produce structurally identical
/// summary output: same section headers, same legend, same footer. The
/// per-section crate counts must match too — both binaries embed the same
/// manifest. We don't compare byte-for-byte because the footer text
/// references both binaries' names equally, which is fine.
#[test]
fn ein_and_gix_licenses_share_the_same_summary_surface() {
    let gix = run_gix_licenses(&[]);
    let ein_output = run_licenses_raw(env!("CARGO_BIN_EXE_ein"), &[]);
    assert!(
        ein_output.status.success(),
        "`ein licenses` failed ({}):\nstderr: {}",
        ein_output.status,
        String::from_utf8_lossy(&ein_output.stderr),
    );
    let ein = String::from_utf8(ein_output.stdout).expect("ein stdout is valid UTF-8");
    // The structural section headers must agree between the two binaries.
    for marker in [
        "Third-party crates linked into this binary",
        "Workspace members with separate attribution",
        "Use `gix licenses <CRATE>` or `ein licenses <CRATE>`",
    ] {
        assert!(
            gix.contains(marker) && ein.contains(marker),
            "both binaries should advertise the marker {marker:?}\n\
             in gix: {}\nin ein: {}",
            gix.contains(marker),
            ein.contains(marker),
        );
    }
    // Per-section crate counts must match — they read from the same
    // embedded manifest. Extract `(N):` from the third-party section header
    // in both outputs and compare.
    let count_in = |s: &str, header: &str| -> Option<String> {
        s.lines().find_map(|l| {
            l.strip_prefix(header)
                .and_then(|rest| rest.trim().strip_prefix('('))
                .and_then(|rest| rest.split(')').next().map(str::to_string))
        })
    };
    let gix_third = count_in(&gix, "Third-party crates linked into this binary ");
    let ein_third = count_in(&ein, "Third-party crates linked into this binary ");
    assert_eq!(gix_third, ein_third, "third-party counts differ between gix and ein");
}

/// `gix` and `ein` share the entire `licenses::cli` dispatch and read the
/// same embedded manifest, so for data-only invocations their stdout must
/// be byte-identical. Stderr may differ (ein emits a progress-bar
/// clear-line escape on shutdown, gix doesn't), but the user-facing data
/// — be it the human summary, the full attribution, or the JSON manifest
/// — is the same content for the same build.
///
/// This test is the byte-for-byte counterpart to
/// [`ein_and_gix_licenses_share_the_same_summary_surface`], which only
/// checks structural markers. Together they catch (a) divergence in the
/// rendered text content and (b) divergence in which markers are present.
#[test]
fn ein_and_gix_licenses_produce_identical_stdout_for_data_invocations() {
    let invocations: &[&[&str]] = &[
        &[],                               // default summary
        &["--verbose"],                    // verbose summary
        &["--all"],                        // full attribution
        &["--format", "json"],             // JSON manifest
        &["--format", "json", "gitoxide"], // synthesized JSON entry
        &["gitoxide"],                     // root-license human path
        &["gix-sec"],                      // same-attribution workspace member
    ];
    for args in invocations {
        let gix = run_licenses_raw(env!("CARGO_BIN_EXE_gix"), args);
        let ein = run_licenses_raw(env!("CARGO_BIN_EXE_ein"), args);
        assert!(gix.status.success(), "gix licenses {args:?} failed");
        assert!(ein.status.success(), "ein licenses {args:?} failed");
        assert_eq!(
            gix.stdout,
            ein.stdout,
            "gix and ein produced different stdout for `licenses {args:?}` \
             (gix={} bytes, ein={} bytes)",
            gix.stdout.len(),
            ein.stdout.len(),
        );
    }
}

// ---------------------------------------------------------------------------
// `--all` end-to-end — never asserted on the real binary before. The unit
// tests cover render_all against a hand-crafted fixture; this covers it
// against the real embedded manifest.

/// `gix licenses --all` must include every section header (third-party,
/// separate-attribution workspace, same-attribution workspace) and at least
/// one full per-crate body — recognisable by a `License: ` prefix line that
/// the per-crate writer emits.
#[test]
fn all_emits_full_per_crate_attribution_for_real_manifest() {
    let out = run_gix_licenses(&["--all"]);
    assert!(
        out.contains("Third-party crates linked into this binary"),
        "--all should include the third-party section header"
    );
    assert!(
        out.contains("Workspace members with separate attribution"),
        "--all should include the separate-attribution section header"
    );
    assert!(
        out.contains("Workspace members covered by"),
        "--all must always include the same-attribution section (regardless of --verbose)"
    );
    assert!(
        out.lines().filter(|l| l.starts_with("License: ")).count() > 10,
        "--all should emit one `License: ` line per crate, not just a summary table:\n{}",
        out.lines().take(20).collect::<Vec<_>>().join("\n"),
    );
}

// ---------------------------------------------------------------------------
// Error-path behaviour — never end-to-end-tested before. Both human and
// JSON output paths must fail loudly (non-zero exit, message naming the
// missing crate) when the requested name doesn't exist.

#[test]
fn unknown_crate_name_exits_nonzero_with_a_message_naming_the_crate() {
    let bogus = "this-crate-cannot-possibly-exist-xyzzy";
    let output = run_licenses_raw(env!("CARGO_BIN_EXE_gix"), &[bogus]);
    assert!(
        !output.status.success(),
        "unknown crate name should produce a non-zero exit"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(bogus),
        "stderr should name the missing crate so the user can spot the typo:\n{stderr}"
    );
}

#[test]
fn unknown_crate_name_in_json_path_also_exits_nonzero() {
    let bogus = "this-crate-cannot-possibly-exist-xyzzy";
    let output = run_licenses_raw(env!("CARGO_BIN_EXE_gix"), &["--format", "json", bogus]);
    assert!(
        !output.status.success(),
        "unknown crate name in JSON mode should produce a non-zero exit"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(bogus),
        "stderr should name the missing crate in JSON mode:\n{stderr}"
    );
}

// ---------------------------------------------------------------------------
// JSON output end-to-end — the unit tests in `src/licenses/cli.rs` cover
// the JSON shape against synthetic manifests, but never against the real
// embedded one. A schema regression in `Manifest` or `CrateLicense` could
// be caught at compile time, but a missing field, a serialization typo,
// or a subtle deviation from the documented array-of-CrateLicense shape
// for single-crate JSON queries would not.

/// `gix licenses --format json` (no name) emits the full embedded manifest
/// as a single JSON object with the documented top-level fields. The exact
/// crate count is implementation-determined; we only assert the top-level
/// shape and that at least one crate is present.
#[test]
fn full_manifest_json_shape() {
    let out = run_gix_licenses(&["--format", "json"]);
    let data: serde_json::Value = serde_json::from_str(&out).expect("valid JSON manifest");
    let obj = data.as_object().expect("manifest is a JSON object");
    for key in ["crates", "workspace_members_same_attribution", "target_triple"] {
        assert!(obj.contains_key(key), "manifest JSON should contain `{key}`");
    }
    assert!(
        data["crates"].as_array().is_some_and(|a| !a.is_empty()),
        "manifest must list at least one crate"
    );
    assert!(
        data["workspace_members_same_attribution"].as_array().is_some(),
        "workspace_members_same_attribution must be an array"
    );
}

/// `gix licenses --format json <NAME>` emits a JSON ARRAY (not a single
/// object), even for a single match. This is the documented shape — the
/// array form survives the multi-version case without consumer code
/// branching on hit count.
#[test]
fn single_crate_json_is_always_an_array() {
    // The root `gitoxide` package is always queryable via the synthesized-
    // entry path, so we don't depend on any specific third-party crate.
    let out = run_gix_licenses(&["--format", "json", "gitoxide"]);
    let data: serde_json::Value = serde_json::from_str(&out).expect("valid JSON for `gitoxide`");
    let arr = data
        .as_array()
        .unwrap_or_else(|| panic!("single-crate JSON must be a top-level array, got: {data}"));
    assert_eq!(arr.len(), 1, "expected exactly one synthesized entry for gitoxide");
    let entry = &arr[0];
    assert_eq!(entry["name"], "gitoxide");
    assert!(
        entry["files"].as_array().is_some_and(|f| f.len() >= 2),
        "synthesized gitoxide entry should carry both LICENSE files"
    );
}

// ---------------------------------------------------------------------------
// Real-manifest path coverage — the synthesized-entry, same-attribution,
// and workspace-with-separate-attribution paths are unit-tested against
// fixtures, but never against the real embedded blob. A drift in `build.rs`
// that mis-categorises a real workspace member would show up here.

/// `gix licenses gitoxide` (the root package itself, not in `manifest.crates`)
/// must produce the actual MIT and Apache-2.0 license file bodies inline,
/// not a pointer to where to find them.
#[test]
fn licenses_gitoxide_renders_real_license_text_inline() {
    let out = run_gix_licenses(&["gitoxide"]);
    assert!(
        out.contains("Permission is hereby granted"),
        "gix licenses gitoxide must contain the canonical MIT permission clause inline:\n\
         (head of output)\n{}",
        out.lines().take(10).collect::<Vec<_>>().join("\n"),
    );
    assert!(
        out.contains("Apache License") && out.contains("Version 2.0"),
        "gix licenses gitoxide must contain the canonical Apache-2.0 markers inline"
    );
}

/// `gix licenses gix-sec` (a same-attribution workspace member) must
/// likewise produce the actual root MIT and Apache-2.0 file bodies inline,
/// with a note that the crate is covered by the gitoxide root licenses.
#[test]
fn licenses_same_attribution_workspace_member_renders_real_license_text() {
    let out = run_gix_licenses(&["gix-sec"]);
    assert!(
        out.contains("Permission is hereby granted"),
        "gix licenses gix-sec must contain canonical MIT body inline"
    );
    assert!(
        out.contains("Apache License") && out.contains("Version 2.0"),
        "gix licenses gix-sec must contain canonical Apache-2.0 markers inline"
    );
    assert!(
        out.to_lowercase().contains("gitoxide"),
        "output should mention gitoxide so a reader knows whose root license covers gix-sec"
    );
}

// ---------------------------------------------------------------------------
// `--help` content — clap-derive can silently drop the second paragraph of
// `long_about` if the string is shaped wrong, so a regression that
// suppresses the flag-introduction text would be invisible without an
// explicit assertion. We pin the most user-facing fact: that every flag
// the subcommand supports is *advertised somewhere* in the long help.

/// `gix licenses --help` must mention every public flag — `--all`,
/// `--verbose` (with its `-v` short form), and `--format`. Any future
/// regression that drops one of these from the help text (e.g. a `#[clap]`
/// attribute change that hides the option) is caught here.
#[test]
fn help_advertises_all_public_flags() {
    let output = run_licenses_raw(env!("CARGO_BIN_EXE_gix"), &["--help"]);
    assert!(output.status.success(), "--help should exit 0");
    let help = String::from_utf8(output.stdout).expect("--help is valid UTF-8");
    for flag in ["--all", "--verbose", "--format", "-v", "-f", "-h"] {
        assert!(help.contains(flag), "--help should advertise `{flag}`:\n{help}");
    }
}

/// `gix licenses --help` must explain the relationship between the three
/// invocation modes (no args / single crate name / `--all`), not just list
/// the flags. clap-derive silently drops everything after the first
/// `.\n\n` in `long_about`, so a poorly-shaped attribute can hide this
/// guidance from the user; this test catches that regression.
#[test]
fn help_describes_summary_single_crate_and_all_modes() {
    let output = run_licenses_raw(env!("CARGO_BIN_EXE_gix"), &["--help"]);
    let help = String::from_utf8(output.stdout).expect("--help is valid UTF-8");
    // Some phrase that explains the no-args summary mode.
    assert!(
        help.contains("summary table"),
        "--help should explain the summary mode (no args):\n{help}"
    );
    // Some phrase that explains the single-crate-name mode.
    assert!(
        help.to_lowercase().contains("single crate"),
        "--help should explain the single-crate-name mode:\n{help}"
    );
    // Some phrase that explains --all.
    assert!(
        help.contains("THIRD-PARTY-LICENSES.txt"),
        "--help should explain the --all mode by referencing THIRD-PARTY-LICENSES.txt:\n{help}"
    );
}
