//! `bynk book` — the Bynk Book as this toolchain documents it (#2463).
//!
//! The editor's `bynk_search_book` language-model tool needs Book text that
//! matches the `bynk` the user runs, not the live site (which may describe
//! features this build lacks) and not the extension's own version. The driver
//! embeds the whole Book at build time (`build.rs`: the `llms-full.txt` bundle,
//! every page in sidebar order) and prints it here, under a first line naming
//! the toolchain version. The caller searches it; keeping the search out of the
//! driver means the hosted fallback is ranked the same way.
//!
//! A build made without the bundle (a plain `cargo build`) carries none:
//! `bynk book` then says so and exits non-zero, and the caller falls back to
//! the hosted Book with a version-mismatch note.

use std::io::Write;
use std::process::ExitCode;

/// The Book bundle embedded at build time; empty when the build had none.
const BUNDLE: &str = include_str!(concat!(env!("OUT_DIR"), "/book.txt"));

/// The text `bynk book` prints for `bundle`, or `None` when there is none.
fn render(bundle: &str) -> Option<String> {
    if bundle.trim().is_empty() {
        return None;
    }
    Some(format!(
        "Bynk Book v{}\n\n{bundle}",
        env!("CARGO_PKG_VERSION")
    ))
}

/// Run `bynk book`. Exits `0` with the Book on stdout, or `1` when this build
/// carries no Book.
pub fn run() -> ExitCode {
    match render(BUNDLE) {
        Some(text) => {
            // A closed pipe (`bynk book | head`) is the reader's choice.
            let _ = std::io::stdout().write_all(text.as_bytes());
            ExitCode::SUCCESS
        }
        None => {
            eprintln!(
                "error: this `bynk` was built without the Book bundle. Read it at \
                 {}/book/ (or {}/llms-full.txt).",
                bynk_syntax::diagnostics::BOOK_BASE_URL,
                bynk_syntax::diagnostics::BOOK_BASE_URL
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_bundle_is_reported_not_printed() {
        assert_eq!(render(""), None);
        assert_eq!(render("  \n"), None);
    }

    #[test]
    fn bundle_is_headed_by_the_toolchain_version() {
        let out = render("# Bynk — Full Documentation\n").expect("non-empty");
        assert!(out.starts_with(&format!(
            "Bynk Book v{}\n\n# Bynk",
            env!("CARGO_PKG_VERSION")
        )));
    }
}
