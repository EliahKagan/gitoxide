//! Spot checks that the manifest embedded in the `gix` binary reflects the
//! feature profile and target it was built with.
//!
//! `tests/licenses_parity.rs` compares the whole crate set against
//! `cargo tree`. The checks here pin a few well-known discriminators by
//! name instead, so that a wrong edge kind or target passed to `cargo tree`
//! on both sides would still be caught, and so that a failure names the
//! crate that is unexpectedly present or missing.
//!
//! Each `cargo test` run builds one binary for the host target and the
//! feature profiles it was given — usually one, but `cargo test --features
//! max-pure` without `--no-default-features` also enables `max` — and CI
//! runs this file for every profile on every OS. The assertions therefore
//! branch on `cfg!` rather than probing other profiles, and derive each
//! expectation from the set of enabled profiles.

mod support;

use std::collections::BTreeSet;

use support::{gix_manifest, manifest_crate_names};

fn assert_present(names: &BTreeSet<String>, expected: &[&str], why: &str) {
    let missing: Vec<&&str> = expected.iter().filter(|name| !names.contains(**name)).collect();
    assert!(missing.is_empty(), "{why}, but the manifest lacks {missing:?}");
}

fn assert_absent(names: &BTreeSet<String>, forbidden: &[&str], why: &str) {
    let present: Vec<&&str> = forbidden.iter().filter(|name| names.contains(**name)).collect();
    assert!(present.is_empty(), "{why}, but the manifest lists {present:?}");
}

/// Whether the enabled profiles link `curl`; `openssl-sys` comes with it
/// except on Windows, where curl uses Schannel.
const CURL: bool = cfg!(any(feature = "max", feature = "lean"));
/// Whether the enabled profiles link `reqwest` over `rustls`.
const REQWEST: bool = cfg!(feature = "max-pure");
/// Whether the enabled profiles include `gix-archive`, an optional
/// dependency of `gix` that every profile except `small` enables.
const ARCHIVE: bool = cfg!(any(
    feature = "max",
    feature = "max-pure",
    feature = "lean",
    feature = "lean-async"
));

#[test]
fn http_backend_follows_the_feature_profile() {
    let names = manifest_crate_names(&gix_manifest());
    if CURL {
        assert_present(&names, &["curl", "curl-sys"], "`max` and `lean` use curl");
        if cfg!(windows) {
            assert_absent(&names, &["openssl-sys"], "curl uses Schannel on Windows");
        } else {
            assert_present(&names, &["openssl-sys"], "curl uses openssl off Windows");
        }
    } else {
        assert_absent(
            &names,
            &["curl", "curl-sys", "openssl-sys"],
            "only `max` and `lean` use curl",
        );
    }
    if REQWEST {
        assert_present(
            &names,
            &["reqwest", "rustls", "hyper"],
            "`max-pure` uses reqwest over rustls",
        );
    } else {
        assert_absent(&names, &["reqwest", "rustls"], "only `max-pure` uses reqwest");
    }
}

#[test]
fn platform_specific_crates_follow_the_target() {
    let names = manifest_crate_names(&gix_manifest());
    if cfg!(windows) {
        assert_present(&names, &["windows-sys"], "every profile needs `windows-sys` on Windows");
        assert_absent(&names, &["xattr"], "`xattr` is Unix-only");
    } else {
        assert_absent(
            &names,
            &["windows-sys", "winapi"],
            "Windows bindings are not linked elsewhere",
        );
        if ARCHIVE {
            assert_present(&names, &["xattr"], "Unix builds beyond `small` link `xattr`");
        }
    }
    if cfg!(target_vendor = "apple") && REQWEST {
        assert_present(
            &names,
            &["security-framework"],
            "rustls uses the platform verifier on Apple targets",
        );
    } else {
        assert_absent(
            &names,
            &["security-framework"],
            "`security-framework` is Apple-and-rustls-only",
        );
    }
}

#[test]
fn optional_workspace_crates_follow_the_feature_profile() {
    let names = manifest_crate_names(&gix_manifest());
    if ARCHIVE {
        assert_present(
            &names,
            &["gix-archive"],
            "profiles other than `small` include archive support",
        );
    } else {
        assert_absent(&names, &["gix-archive"], "`small` has no archive support");
    }
}

#[test]
fn dev_only_workspace_crates_are_absent() {
    let names = manifest_crate_names(&gix_manifest());
    assert_absent(
        &names,
        &["gix-testtools", "gix-config-tests", "gix-status-tests"],
        "crates that exist only as dev-dependencies are never linked",
    );
}
