//! Prelude types: units a unit reaches without writing `uses` (#3955, #4231).
//!
//! A prelude type ([`bynk_std::PRELUDE_TYPES`]) is nameable with no import in
//! every context and commons, as `Option` is. Its declaring first-party commons
//! is reached as if the unit had written `uses <unit>` whenever the unit names
//! the type or writes a spelling that yields one (`Ordering`: `.compare`,
//! `.sortWith`). The compiler adds that line to the parsed unit, so name
//! resolution, `uses` composition, emission and a declaration of the unit's own
//! shadowing the name all behave as for a written `uses`. A unit that mentions
//! none of the words is left alone, so nothing is injected or emitted for a
//! project that never needs one.
//!
//! Two rules keep a prelude type from disturbing a program: its variants resolve
//! only from context when another sum in scope shares the name, and it is not
//! rebranded per context ([`crate::resolver::is_prelude_type`]).
//!
//! This is the one place that decides it. A new compiler-known sum is a row in
//! the table and its unit.

use bynk_project::ParsedFile;
use bynk_std::PRELUDE_TYPES;
use bynk_syntax::ast::CommonsItem;

/// Add the implicit `uses` to every parsed file of the project's own that needs
/// one: a file that names a prelude type, or writes a spelling that yields one
/// ([`bynk_std::PreludeType`]). First-party units are left as written. A file
/// that declares a type of that name, or `uses`/`consumes` a unit that does, has
/// shadowed the prelude's: it keeps the declaration the project wrote, and the
/// two never meet.
pub(crate) fn add_implicit_uses(parsed: &mut [ParsedFile]) {
    for prelude in PRELUDE_TYPES {
        let declaring: std::collections::HashSet<String> = parsed
            .iter()
            .filter(|pf| !pf.is_synthetic() && declares_type(pf, prelude.name))
            .map(|pf| pf.unit().name().joined())
            .collect();
        for pf in parsed.iter_mut().filter(|pf| !pf.is_synthetic()) {
            let shadowed = declaring.contains(&pf.unit().name().joined())
                || pf
                    .uses()
                    .iter()
                    .any(|u| declaring.contains(&u.target.joined()))
                || pf
                    .consumes()
                    .iter()
                    .any(|c| declaring.contains(&c.target.joined()));
            if shadowed {
                continue;
            }
            if std::iter::once(&prelude.name)
                .chain(prelude.triggers)
                .any(|w| mentions_word(pf.source(), w))
            {
                pf.add_implicit_uses(prelude.unit);
            }
        }
    }
}

/// Whether the file declares a type called `name`.
fn declares_type(pf: &ParsedFile, name: &str) -> bool {
    pf.items()
        .iter()
        .any(|item| matches!(item, CommonsItem::Type(t) if t.name.name == name))
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
