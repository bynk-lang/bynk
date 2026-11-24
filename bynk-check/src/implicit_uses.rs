//! Units a unit reaches without writing `uses` (#3955).
//!
//! `Ordering` is what `a.compare(b)` and a `sortWith` comparator give back, so a
//! unit that mentions either (or names `Ordering`) needs the `bynk.ordering`
//! commons in scope as if it had written `uses bynk.ordering`. The compiler adds
//! that line to the parsed unit, so name resolution, `uses` composition,
//! emission and a user declaration shadowing a name all behave as for a written
//! `uses`. A unit that mentions none of the words is left alone, so nothing is
//! injected or emitted for a project that never orders anything.
//!
//! This is the one place that decides it; the prelude-type hook that makes
//! `Ordering` nameable without a `uses` (#4231) replaces it.

use bynk_project::ParsedFile;
use bynk_std::ORDERING_UNIT;

/// The words whose presence in a source file make `bynk.ordering` implicit: the
/// two methods (as a call, after a `.`, so a function of the author's own named
/// `compare` does not count) and the type's name.
const ORDERING_TRIGGERS: &[&str] = &[".compare", ".sortWith", "Ordering"];

/// Add the implicit `uses` to every parsed file of the project's own that needs
/// one. First-party units are left as written.
pub(crate) fn add_implicit_uses(parsed: &mut [ParsedFile]) {
    for pf in parsed.iter_mut().filter(|pf| !pf.is_synthetic()) {
        if ORDERING_TRIGGERS
            .iter()
            .any(|w| mentions_word(pf.source(), w))
        {
            pf.add_implicit_uses(ORDERING_UNIT);
        }
    }
}

/// Whether `word` appears in `source` as a whole identifier (a leading `.` is
/// part of the word, so it asks for a method call).
fn mentions_word(source: &str, word: &str) -> bool {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let leading_dot = word.starts_with('.');
    source.match_indices(word).any(|(i, _)| {
        let before = source[..i].chars().next_back();
        let after = source[i + word.len()..].chars().next();
        (leading_dot || !before.is_some_and(is_ident)) && !after.is_some_and(is_ident)
    })
}

#[cfg(test)]
mod tests {
    use super::mentions_word;

    #[test]
    fn a_word_is_a_whole_identifier() {
        assert!(mentions_word("a.compare(b)", ".compare"));
        assert!(mentions_word("xs.sortWith(f)", ".sortWith"));
        assert!(!mentions_word("a.compared(b)", ".compare"));
        assert!(!mentions_word("fn compare(a: Int) {}", ".compare"));
        assert!(mentions_word("fn x() -> Ordering {}", "Ordering"));
        assert!(!mentions_word("fn x() -> MyOrdering {}", "Ordering"));
    }
}
