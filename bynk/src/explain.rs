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

use std::process::ExitCode;

use bynk_syntax::diagnostics::{self, Explain};

/// What `explain` found for a requested code.
enum Lookup {
    /// A curated explanation.
    Explained(&'static Explain),
    /// A real diagnostic code with no curated explanation yet; carries its
    /// one-line registry summary.
    KnownUnexplained(&'static str),
    /// Not a code the compiler emits.
    Unknown,
}

fn lookup(code: &str) -> Lookup {
    if let Some(e) = diagnostics::explain(code) {
        Lookup::Explained(e)
    } else if let Some(info) = diagnostics::REGISTRY.iter().find(|d| d.code == code) {
        Lookup::KnownUnexplained(info.summary)
    } else {
        Lookup::Unknown
    }
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
         No extended explanation is written for this code yet. See the diagnostic \
         index for every code and its summary:\n\n    {}/book/reference/diagnostics/\n",
        diagnostics::BOOK_BASE_URL
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
        Lookup::Unknown => {
            eprintln!(
                "error: `{code}` is not a diagnostic code the compiler emits.\n\
                 Diagnostic codes look like `bynk.resolve.unknown_type`; see \
                 {}/book/reference/diagnostics/ for the full list.",
                diagnostics::BOOK_BASE_URL
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
        assert!(out.contains("https://bynk-lang.org/book/reference/types/"));
    }

    #[test]
    fn known_but_unexplained_code_is_graceful() {
        // Pick a real registry code with no curated explanation. If this ever
        // gains one, swap it — the point is a code that exists but isn't curated.
        let code = "bynk.resolve.duplicate_fn";
        assert!(diagnostics::explain(code).is_none());
        assert!(matches!(lookup(code), Lookup::KnownUnexplained(_)));
        let out = render_known_unexplained(code, "Two functions share a name.");
        assert!(out.contains(code));
        assert!(out.contains("No extended explanation"));
    }

    #[test]
    fn unknown_code_is_not_found() {
        assert!(matches!(lookup("bynk.not.a_real_code"), Lookup::Unknown));
        assert!(matches!(lookup("nonsense"), Lookup::Unknown));
    }
}
