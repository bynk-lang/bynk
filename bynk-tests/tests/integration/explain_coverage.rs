//! #3163: every error code is explained, or is on a named exception list with
//! a reason. A new error code that ships with neither fails here, so the
//! backlog of unexplained codes can only shrink.
//!
//! `bynk explain <code>` is where the longer lesson behind a rule lives
//! (`design/proposals/README.md`, "Documentation is part of done"). A code earns
//! an entry on the exception list only when it cannot yet have one, and the reason
//! says why. The explained set is backed separately, by a compiled pair per
//! code (`explain_examples.rs`). Warnings are exempt from the rule.
//!
//! Remove a code's entry here in the PR that explains it: the second test
//! refuses an entry for a code that now has an explanation.

use std::collections::BTreeSet;

use bynk_syntax::diagnostics::{EXPLANATIONS, REGISTRY};
use bynk_syntax::error::Severity;

/// Error codes with no explanation yet, each with the reason: one `code: reason`
/// per line, `#` comments, in `design/explain-exceptions.txt`.
fn exceptions() -> Vec<(&'static str, &'static str)> {
    include_str!("../../../design/explain-exceptions.txt")
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| {
            l.split_once(": ")
                .unwrap_or_else(|| panic!("explain-exceptions.txt: `{l}` is not `code: reason`"))
        })
        .collect()
}

#[test]
fn every_error_code_is_explained_or_a_named_exception() {
    let explained: BTreeSet<&str> = EXPLANATIONS.iter().map(|e| e.code).collect();
    let excepted: BTreeSet<&str> = exceptions().iter().map(|(code, _)| *code).collect();
    let missing: Vec<&str> = REGISTRY
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .map(|d| d.code)
        .filter(|code| !explained.contains(code) && !excepted.contains(code))
        .collect();
    assert!(
        missing.is_empty(),
        "error codes with no `bynk explain` entry (add one in `EXPLANATIONS` in \
         bynk-syntax/src/diagnostics.rs, with its fixture pair under \
         bynk-tests/tests/fixtures/explain/, or, if it cannot have a compiled \
         example, name it with a reason in design/explain-exceptions.txt):\n  {}",
        missing.join("\n  ")
    );
}

/// The list names real, unexplained error codes, each with a reason.
#[test]
fn the_exception_list_names_unexplained_error_codes() {
    let explained: BTreeSet<&str> = EXPLANATIONS.iter().map(|e| e.code).collect();
    let mut seen = BTreeSet::new();
    for (code, reason) in exceptions() {
        assert!(seen.insert(code), "`{code}` is listed twice");
        let info = REGISTRY
            .iter()
            .find(|d| d.code == code)
            .unwrap_or_else(|| panic!("`{code}` is not a registry code"));
        assert_eq!(info.severity, Severity::Error, "`{code}` is not an error");
        assert!(
            !explained.contains(code),
            "`{code}` now has an explanation: drop it from design/explain-exceptions.txt"
        );
        assert!(!reason.trim().is_empty(), "`{code}` has no reason");
    }
}
