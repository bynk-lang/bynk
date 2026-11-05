//! The single implementation of the `BYNK_REQUIRE_*` "is this gate required?"
//! contract, shared by the workerd smokes.
//!
//! These tests skip when their external toolchain (wrangler/workerd) cannot
//! boot, and CI turns that skip into a hard failure on the OS where the
//! toolchain is expected to work. The subtlety is what "set" means.
//!
//! CI used to select the OS with `${{ matrix.os == 'ubuntu-latest' && '1' || '' }}`,
//! which does NOT unset the variable on the other OSes — Actions exports it as
//! the empty string. A guard written as the obvious `env::var(..).is_ok()`
//! therefore read "required" on every OS, and the smoke failed on Windows
//! instead of skipping. That bug was fixed twice, in two different test
//! binaries, four days apart ([#965] then [#971]) — each time only after it had
//! already landed on `main`, because the copies of the guard were independent.
//!
//! Two things changed so it cannot recur. CI now exports the variable from the
//! shell, so on non-Linux it is genuinely absent rather than empty. And the
//! contract lives here, once, so a new workerd test inherits it instead of
//! reimplementing it.
//!
//! [#965]: https://github.com/accuser/bynk/pull/965
//! [#971]: https://github.com/accuser/bynk/pull/971

/// Is `var` set to a non-empty value?
///
/// Empty is treated as unset — see the module docs for why that is the contract
/// rather than `env::var(..).is_ok()`.
pub fn is_required(var: &str) -> bool {
    is_required_value(std::env::var(var).ok().as_deref())
}

/// The decision itself, split from the environment lookup so it can be tested
/// without mutating process state (which `cargo test`, unlike nextest, shares
/// across concurrently-running tests).
fn is_required_value(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::is_required_value;

    /// The regression this module exists for: an *empty* value is the shape CI
    /// used to produce on the OSes it deliberately lets skip, and it must read
    /// as "not required". `env::var(..).is_ok()` returns true here — which is
    /// exactly the bug fixed in #965 and again in #971.
    #[test]
    fn empty_is_not_required() {
        assert!(
            !is_required_value(Some("")),
            "an empty value must read as not-required, not merely as present"
        );
    }

    #[test]
    fn unset_is_not_required() {
        assert!(!is_required_value(None));
    }

    #[test]
    fn non_empty_is_required() {
        assert!(is_required_value(Some("1")));
    }
}
