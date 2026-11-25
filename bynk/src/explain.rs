//! `bynk explain <code>` — the compiler's own explanation of a diagnostic code
//! (#853), the analogue of `rustc --explain`.
//!
//! Every `bynk.*` diagnostic carries a stable machine code, but a code alone is
//! a dead end for a newcomer. This subcommand prints the curated, *offline-
//! complete* explanation behind a code — what the rule is, why it exists, and a
//! minimal before/after example — plus a link to the relevant Book concept
//! page. The same [`bynk_syntax::diagnostics::EXPLANATIONS`] table backs the
//! editor's clickable diagnostic-code links (`codeDescription`), so the two
//! surfaces never drift (DECISION A).
//!
//! Coverage is incremental (DECISION B): a code with no curated explanation is
//! reported gracefully (its one-line registry summary, and a note that a longer
//! explanation is not written yet), and a code the compiler does not recognise
//! at all exits non-zero. Neither is an error state for the feature — a
//! half-covered set is a designed, documented condition.

use bynk_driver::emsg;
use std::process::ExitCode;

use bynk_syntax::diagnostics::{self, Explain, Retired};

/// What `explain` found for a requested code.
enum Lookup {
    /// A curated explanation.
    Explained(&'static Explain),
    /// A real diagnostic code with no curated explanation yet; carries its
    /// one-line registry summary.
    KnownUnexplained(&'static str),
    /// A code the compiler once emitted and no longer does (#4277).
    Retired(&'static Retired),
    /// Not a code the compiler emits.
    Unknown,
}

fn lookup(code: &str) -> Lookup {
    if let Some(e) = diagnostics::explain(code) {
        Lookup::Explained(e)
    } else if let Some(info) = diagnostics::REGISTRY.iter().find(|d| d.code == code) {
        Lookup::KnownUnexplained(info.summary)
    } else if let Some(r) = diagnostics::retired(code) {
        Lookup::Retired(r)
    } else {
        Lookup::Unknown
    }
}

/// The tombstone text for a retired code: when it went, why, and what replaced it.
fn render_retired(r: &Retired) -> String {
    let mut out = format!(
        "{}\n\nThis diagnostic code was retired after v{}, its last release, and the compiler no longer emits it.\n\n{}\n",
        r.code, r.last_release, r.why
    );
    if let Some(by) = r.replacement {
        out.push_str(&format!("\nReplaced by: {by}\n"));
    }
    out
}

/// The full explanation text for a curated code (the offline-complete answer).
fn render_explained(e: &Explain) -> String {
    let mut out = String::new();
    out.push_str(e.code);
    out.push_str("\n\n");
    out.push_str(e.blurb);
    out.push_str("\n\nExample:\n\n");
    for line in e.example.lines() {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("\nLearn more: ");
    out.push_str(&e.href());
    out.push('\n');
    out
}

/// The graceful message for a real code that has no curated explanation yet.
fn render_known_unexplained(code: &str, summary: &str) -> String {
    format!(
        "{code}\n\n{summary}\n\n\
         No extended explanation is written for this code yet. Its page links the \
         other codes in its category:\n\n    {}\n",
        diagnostics::href(code)
    )
}

/// Run `bynk explain <code>`. Exits `0` for a recognised code (explained or
/// not) and non-zero for an unrecognised one.
pub fn run(code: &str) -> ExitCode {
    match lookup(code) {
        Lookup::Explained(e) => {
            print!("{}", render_explained(e));
            ExitCode::SUCCESS
        }
        Lookup::KnownUnexplained(summary) => {
            print!("{}", render_known_unexplained(code, summary));
            ExitCode::SUCCESS
        }
        Lookup::Retired(r) => {
            print!("{}", render_retired(r));
            ExitCode::SUCCESS
        }
        Lookup::Unknown => {
            emsg!(
                "error: `{code}` is not a diagnostic code the compiler emits.\n\
                 Diagnostic codes look like `bynk.resolve.unknown_type`; see \
                 {} for the full list.",
                diagnostics::href("")
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explained_code_prints_blurb_example_and_href() {
        let e = diagnostics::explain("bynk.resolve.unknown_type").expect("curated");
        let out = render_explained(e);
        assert!(out.contains("bynk.resolve.unknown_type"));
        assert!(out.contains(e.blurb));
        assert!(out.contains("Example:"));
        // The hosted href is the offline-visible "learn more" target.
        assert!(out.contains("https://bynk-lang.org/d/bynk.resolve.unknown_type"));
    }

    #[test]
    fn known_but_unexplained_code_is_graceful() {
        // Pick a real registry code with no curated explanation. If this ever
        // gains one, swap it — the point is a code that exists but isn't curated.
        let code = "bynk.wasm.panic";
        assert!(diagnostics::explain(code).is_none());
        assert!(matches!(lookup(code), Lookup::KnownUnexplained(_)));
        let out = render_known_unexplained(code, "The in-browser compiler panicked internally.");
        assert!(out.contains(code));
        assert!(out.contains("No extended explanation"));
        assert!(
            out.contains("https://bynk-lang.org/d/bynk.wasm.panic\n"),
            "{out}"
        );
    }

    #[test]
    fn a_retired_code_says_when_why_and_what_replaced_it() {
        let r = Retired {
            code: "bynk.http.multipart_body_unbounded",
            last_release: "1.2.3",
            why: "The default body cap makes it unreachable.",
            replacement: Some("the default body cap (#3921)"),
        };
        let out = render_retired(&r);
        assert!(out.contains("bynk.http.multipart_body_unbounded"), "{out}");
        assert!(out.contains("retired after v1.2.3"), "{out}");
        assert!(
            out.contains("default body cap makes it unreachable"),
            "{out}"
        );
        assert!(
            out.contains("Replaced by: the default body cap (#3921)"),
            "{out}"
        );
        let bare = Retired {
            replacement: None,
            ..r
        };
        assert!(!render_retired(&bare).contains("Replaced by"));
    }

    #[test]
    fn unknown_code_is_not_found() {
        assert!(matches!(lookup("bynk.not.a_real_code"), Lookup::Unknown));
        assert!(matches!(lookup("nonsense"), Lookup::Unknown));
    }
}
