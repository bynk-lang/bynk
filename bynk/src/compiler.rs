//! `BYNK_BYNKC`, retired (#2998, ADR 0736).
//!
//! `bynk` used to locate a separate `bynkc` (`BYNK_BYNKC`, then `PATH`, then a
//! sibling of itself) and gate on its version skew (ADR 0434). Every command now
//! runs the compiler in-process, so there is nothing to locate and nothing to
//! skew against. The variable is still read, once, to say it no longer does
//! anything: a pinned editor or CI script that sets it should hear that, not
//! have the pin quietly stop governing the result. The warning and the variable
//! go with the `bynkc` alias.

/// The variable that used to pin an external `bynkc`.
pub const RETIRED_COMPILER_ENV: &str = "BYNK_BYNKC";

/// The one-line warning for a set (non-empty) `BYNK_BYNKC`, or `None`.
pub fn ignored_override_warning(value: Option<&std::ffi::OsStr>) -> Option<String> {
    let value = value.filter(|v| !v.is_empty())?;
    Some(format!(
        "bynk: warning: {RETIRED_COMPILER_ENV}={} is ignored: `bynk` compiles in-process, so no \
         separate `bynkc` is run or needed. Unset it.",
        value.to_string_lossy()
    ))
}

/// Print [`ignored_override_warning`] for this process's environment.
pub fn warn_if_override_set() {
    let value = std::env::var_os(RETIRED_COMPILER_ENV);
    if let Some(message) = ignored_override_warning(value.as_deref()) {
        eprintln!("{message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn a_set_override_is_named_in_the_warning() {
        let w = ignored_override_warning(Some(OsStr::new("/opt/bynkc"))).unwrap();
        assert!(
            w.contains("BYNK_BYNKC=/opt/bynkc") && w.contains("ignored"),
            "{w}"
        );
    }

    #[test]
    fn unset_or_empty_says_nothing() {
        assert!(ignored_override_warning(None).is_none());
        assert!(ignored_override_warning(Some(OsStr::new(""))).is_none());
    }
}
