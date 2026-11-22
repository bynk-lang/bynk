//! The versions of the external tools the toolchain fetches on demand.
//!
//! The `wrangler@4` pin is not here yet: it belongs to wrangler spawning,
//! which 1D.3's `Wrangler` client replaces whole (#4003, Decision A).

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
    fn tsx_is_unpinned() {
        assert_eq!(TSX_SPEC, "tsx");
    }
}
