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
//!
//! # The `bynk` adapter's transparent types in a commons (#4181)
//!
//! A commons cannot `consumes`, so it could not name `Uuid` or `CronSchedule`,
//! and a constant a context shares (`const Nightly: CronSchedule = …`) had to
//! live in each context. [`add_implicit_consumes`] closes that: a commons that
//! names a type the `bynk` adapter exports `transparent` is given the clause
//! `consumes bynk { <those types> }`, which is type-only. The unit gains no
//! capability and stays pure. Opaque types and capabilities are not
//! `transparent` exports, so they are not reached, and naming one is still
//! `bynk.resolve.unknown_type`. As with a prelude type, a name the file (or a
//! unit it `uses`) declares itself is left alone.

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

/// The names the `bynk` adapter exports under `kind`, read from its own source,
/// so a name added there is reached with no second edit. Empty if the source
/// does not parse (`every_first_party_source_parses` rejects that in CI).
fn bynk_exports(kind: bynk_syntax::ast::ExportKind) -> Vec<String> {
    use bynk_syntax::ast::SourceUnit;
    let Some(unit) = bynk_std::UNITS
        .iter()
        .find(|u| u.name == bynk_std::BYNK_UNIT)
    else {
        return Vec::new();
    };
    let parsed = bynk_syntax::lexer::tokenize(unit.source)
        .ok()
        .and_then(|tokens| bynk_syntax::parser::parse_unit(&tokens, unit.source).ok());
    let Some(SourceUnit::Adapter(adapter)) = parsed else {
        return Vec::new();
    };
    adapter
        .exports
        .iter()
        .filter(|e| e.kind == kind)
        .flat_map(|e| e.names.iter().map(|n| n.name.name.clone()))
        .collect()
}

/// The types the `bynk` adapter exports `transparent`.
fn bynk_transparent_types() -> &'static [String] {
    use bynk_syntax::ast::{ExportKind, Visibility};
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| bynk_exports(ExportKind::Type(Visibility::Transparent)))
}

/// Whether `name` is a capability the `bynk` adapter exports (`Clock`,
/// `Logger`, …). A capability is not a type, so naming one as a type is
/// refused; this lets that error say so (#4181).
pub(crate) fn is_bynk_capability(name: &str) -> bool {
    use bynk_syntax::ast::ExportKind;
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES
        .get_or_init(|| bynk_exports(ExportKind::Capability))
        .iter()
        .any(|n| n == name)
}

/// #4181: give every commons of the project's own that names a transparent type
/// of the `bynk` adapter the type-only clause `consumes bynk { … }` for the
/// names it uses, so the type is nameable there without `consumes`. Contexts
/// keep writing their own `consumes`. First-party units are left as written. A
/// name the file declares (a type, or a variant of one of its sums), or that a
/// unit it `uses` declares, is the unit's own and is not reached.
pub(crate) fn add_implicit_consumes(parsed: &mut [ParsedFile]) {
    let candidates = bynk_transparent_types();
    if candidates.is_empty() {
        return;
    }
    let declaring: Vec<(String, std::collections::HashSet<String>)> = candidates
        .iter()
        .map(|name| {
            let units = parsed
                .iter()
                .filter(|pf| !pf.is_synthetic() && declares_name(pf, name))
                .map(|pf| pf.unit().name().joined())
                .collect();
            (name.clone(), units)
        })
        .collect();
    for pf in parsed.iter_mut() {
        if pf.is_synthetic() || !matches!(pf.unit(), bynk_syntax::ast::SourceUnit::Commons(_)) {
            continue;
        }
        let own = pf.unit().name().joined();
        let used: Vec<String> = pf.uses().iter().map(|u| u.target.joined()).collect();
        // One tokenisation per file, and only for a file that mentions a candidate.
        let idents = identifier_set(pf.source(), candidates);
        let wanted: Vec<&str> = declaring
            .iter()
            .filter(|(name, units)| {
                !units.contains(&own)
                    && !used.iter().any(|u| units.contains(u))
                    && idents.contains(name.as_str())
            })
            .map(|(name, _)| name.as_str())
            .collect();
        if !wanted.is_empty() {
            let names: Vec<String> = wanted.iter().map(|n| n.to_string()).collect();
            pf.add_implicit_consumes(bynk_std::BYNK_UNIT, &names);
        }
    }
}

/// Whether the file declares anything called `name`: a type, an event, an
/// agent or actor (they share the type namespace), or a variant of one of its
/// sums. Any of them would meet the adapter's type of that name.
fn declares_name(pf: &ParsedFile, name: &str) -> bool {
    pf.items().iter().any(|item| {
        item.name().is_some_and(|n| n.name == name)
            || matches!(item, CommonsItem::Type(t) if type_has_variant(t, name))
    })
}

/// Whether `t` is a sum with a variant called `name`.
fn type_has_variant(t: &bynk_syntax::ast::TypeDecl, name: &str) -> bool {
    use bynk_syntax::ast::TypeBody;
    match &t.body {
        TypeBody::Sum(body) => body.variants.iter().any(|v| v.name.name == name),
        _ => false,
    }
}

/// The candidates that appear in `source` as identifier tokens. Unlike
/// [`mentions_word`] this skips comments, docs and string text, so a word in
/// prose never reaches an adapter the unit does not use. The file is tokenised
/// once, and only when it mentions a candidate at all.
fn identifier_set<'a>(
    source: &str,
    candidates: &'a [String],
) -> std::collections::HashSet<&'a str> {
    use bynk_syntax::lexer::{TokenKind, tokenize};
    let mut found = std::collections::HashSet::new();
    let mentioned: Vec<&'a String> = candidates
        .iter()
        .filter(|name| mentions_word(source, name))
        .collect();
    if mentioned.is_empty() {
        return found;
    }
    if let Ok(tokens) = tokenize(source) {
        for t in tokens.iter().filter(|t| t.kind == TokenKind::Ident) {
            if let Some(text) = source.get(t.span.start..t.span.end)
                && let Some(name) = mentioned.iter().find(|n| n.as_str() == text)
            {
                found.insert(name.as_str());
            }
        }
    }
    found
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
    use super::{identifier_set, mentions_word};

    /// #4181: a name counts only as an identifier token, not in prose or a string.
    #[test]
    fn a_candidate_is_an_identifier_token_not_prose() {
        let names = vec!["Uuid".to_string(), "Request".to_string()];
        let src =
            "commons a.b\n\n// a Request is sent\nfn f(u: Uuid) -> String {\n  \"a Request\"\n}\n";
        let found = identifier_set(src, &names);
        assert!(found.contains("Uuid"), "{found:?}");
        assert!(!found.contains("Request"), "{found:?}");
        assert!(identifier_set("commons a.b\n", &names).is_empty());
    }

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
