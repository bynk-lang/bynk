//! The versions of the external tools the toolchain fetches on demand.
//!
//! `WRANGLER_SPEC` is written here first for the test harness (1D.3d,
//! #4043 Decision J). The production spawns still spell `wrangler@4` out until
//! 1D.3b routes them through this constant.

/// The TypeScript package a runner fetches when `tsc` is not installed:
/// `typescript@<major>`, the major the generated output is verified against
/// (`bynk_emit::TYPESCRIPT_MAJOR_TESTED`, #1672). Also the package the install
/// advice names.
pub fn typescript_spec() -> String {
    format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED)
}

/// The `tsx` package a runner fetches when `tsx` is not installed. Unpinned:
/// pinning it would change what a user's `bynk test` fetches (#4003,
/// Decision G).
pub const TSX_SPEC: &str = "tsx";

/// The wrangler package a runner fetches when wrangler is not installed:
/// `wrangler@<major>`, so a new major cannot arrive unannounced. The test
/// harness provisions exactly this spec before its smokes run, and CI's
/// pre-warm step names it too (`bynk-tests/tests/system/wrangler_prewarm.rs`).
pub const WRANGLER_SPEC: &str = "wrangler@4";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typescript_is_pinned_to_the_tested_major() {
        assert_eq!(
            typescript_spec(),
            format!("typescript@{}", bynk_emit::TYPESCRIPT_MAJOR_TESTED)
        );
        let major = typescript_spec()
            .strip_prefix("typescript@")
            .map(str::to_string)
            .unwrap();
        assert!(
            !major.is_empty() && major.bytes().all(|b| b.is_ascii_digit()),
            "a major version alone: {major}"
        );
    }

    #[test]
    fn wrangler_is_pinned_to_a_major() {
        let major = WRANGLER_SPEC.strip_prefix("wrangler@").unwrap();
        assert!(
            !major.is_empty() && major.bytes().all(|b| b.is_ascii_digit()),
            "a major version alone: {major}"
        );
    }

    #[test]
    fn tsx_is_unpinned() {
        assert_eq!(TSX_SPEC, "tsx");
    }
}
