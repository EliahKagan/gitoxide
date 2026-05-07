//! Canonical SPDX license texts used as a fallback when a crate declares a
//! license in `Cargo.toml` but ships no `LICENSE` / `COPYING` / `NOTICE` file.
//!
//! Only SPDX identifiers that appear on the `deny.toml` allowlist are covered;
//! anything else is treated as "missing text" so that CI can catch it and we
//! can extend this table deliberately, never silently.
//!
//! The texts for `MIT` and `Apache-2.0` are reused from the repository's own
//! top-level `LICENSE-MIT` / `LICENSE-APACHE` files to guarantee that a single
//! canonical copy is maintained. Other identifiers return `None`; the
//! `every_crate_has_at_least_one_license_file` test fails loudly if a real
//! crate lands on this path with no LICENSE file in its source tree, so the
//! table can be extended deliberately rather than silently.

/// Canonical text of the MIT License, reused from the repository root.
///
/// The text intentionally does not include a copyright line; when we use this
/// as a fallback for a crate whose source tree omits the LICENSE file, we
/// have no way to know the crate's specific copyright holder — callers will
/// mark the entry so users know the attribution is less precise than if the
/// crate had shipped its own LICENSE.
pub const MIT: &str = include_str!("../../../LICENSE-MIT");

/// Canonical text of the Apache License, Version 2.0, reused from the
/// repository root.
pub const APACHE_2_0: &str = include_str!("../../../LICENSE-APACHE");

/// Return the canonical SPDX license text for a single SPDX identifier, or
/// `None` if we don't (yet) bundle one. The input must be a bare identifier
/// such as `"MIT"` or `"Apache-2.0"` — callers are expected to split SPDX
/// expressions like `"MIT OR Apache-2.0"` into individual identifiers before
/// asking.
pub fn text_for(spdx_id: &str) -> Option<&'static str> {
    match spdx_id {
        "MIT" => Some(MIT),
        "Apache-2.0" => Some(APACHE_2_0),
        _ => None,
    }
}

/// SPDX identifiers for which we currently bundle canonical text. Used by
/// tests and by the CI allowlist-subset check once `build.rs` lands.
pub const COVERED: &[&str] = &["Apache-2.0", "MIT"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mit_and_apache_texts_are_non_empty() {
        assert!(!MIT.trim().is_empty(), "MIT license text must be non-empty");
        assert!(
            !APACHE_2_0.trim().is_empty(),
            "Apache-2.0 license text must be non-empty"
        );
    }

    #[test]
    fn mit_text_resembles_mit_license() {
        // A sanity check that LICENSE-MIT at the repo root is the MIT text.
        assert!(
            MIT.contains("Permission is hereby granted"),
            "MIT text should contain the canonical permission clause"
        );
    }

    #[test]
    fn apache_text_resembles_apache_license() {
        assert!(
            APACHE_2_0.contains("Apache License"),
            "Apache-2.0 text should contain the canonical title"
        );
        assert!(
            APACHE_2_0.contains("Version 2.0"),
            "Apache-2.0 text should identify itself as Version 2.0"
        );
    }

    #[test]
    fn text_for_returns_some_for_covered_ids() {
        for id in COVERED {
            assert!(text_for(id).is_some(), "expected bundled text for {id}");
        }
    }

    #[test]
    fn text_for_returns_none_for_unknown_ids() {
        assert!(text_for("WTFPL").is_none());
        assert!(text_for("").is_none());
        assert!(text_for("not-an-spdx-id").is_none());
    }
}
