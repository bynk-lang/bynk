//! The editor surfaces of a field's `replaces old` clause (#3793): the `replaces` keyword's
//! semantic token, the hover on the clause, and the completion that offers it after a field's
//! type. `replaces` is a contextual keyword, so an editor grammar that matches keywords by
//! spelling cannot tell the clause from a name `replaces`; the parse can.

use bynk_syntax::ast::{CommonsItem, Ident, SourceUnit, TypeBody};
use bynk_syntax::lexer::{Token, TokenKind, tokenize};
use bynk_syntax::parser::parse_unit_with_recovery;
use bynk_syntax::span::Span;

/// What the clause's hover says, whichever field it is on.
pub const REPLACES_DOC: &str = "Renames this field from the old name after `replaces`. A `store` field's stored value, a locked event's recorded field and an incoming wire key under the old name are all read as this field; the field is written under its new name only. `bynk.schema.lock` records the rename, and the clause can be deleted once no stored version it still holds uses the old name (`bynk.field.replaces_settled`). Not for a `Map`, `Set`, `Cache` or `Log`.";

/// Every `replaces old` clause of the unit's fields: the `replaces` token and the old name.
fn clauses(source: &str) -> Vec<(Span, Ident)> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    let (unit, _errs) = parse_unit_with_recovery(&tokens, source);
    let Some(unit) = unit else {
        return Vec::new();
    };
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    let mut olds: Vec<Ident> = Vec::new();
    for item in items {
        match item {
            CommonsItem::Type(t) => {
                if let TypeBody::Record(r) = &t.body {
                    olds.extend(r.fields.iter().filter_map(|f| f.replaces.clone()));
                }
            }
            CommonsItem::Event(e) => {
                olds.extend(e.body.fields.iter().filter_map(|f| f.replaces.clone()));
            }
            CommonsItem::Agent(a) => {
                olds.extend(a.store_fields.iter().filter_map(|f| f.replaces.clone()));
            }
            _ => {}
        }
    }
    olds.into_iter()
        .filter_map(|old| keyword_before(&tokens, source, old.span).map(|kw| (kw, old)))
        .collect()
}

/// The `replaces` token immediately before the token at `name`.
fn keyword_before(tokens: &[Token], source: &str, name: Span) -> Option<Span> {
    let at = tokens.iter().position(|t| t.span == name)?;
    let kw = tokens.get(at.checked_sub(1)?)?;
    (kw.kind == TokenKind::Ident && source.get(kw.span.start..kw.span.end) == Some("replaces"))
        .then_some(kw.span)
}

/// The span of every field clause's `replaces`, for the `keyword` semantic token.
pub fn replaces_keyword_spans(source: &str) -> Vec<Span> {
    clauses(source).into_iter().map(|(kw, _)| kw).collect()
}

/// Hover on a clause's `replaces` or on its old name. `None` anywhere else, so a name
/// `replaces` keeps its own hover.
pub fn describe_replaces_at(source: &str, offset: usize) -> Option<String> {
    clauses(source)
        .into_iter()
        .find(|(kw, old)| {
            (kw.start <= offset && offset < kw.end)
                || (old.span.start <= offset && offset < old.span.end)
        })
        .map(|(_, old)| {
            format!(
                "```bynk\nreplaces {}\n```\n\nRenamed from `{}`. {REPLACES_DOC}",
                old.name, old.name
            )
        })
}

/// Whether the cursor sits after a field's complete type on its line, where `replaces` can
/// follow: `name: Type ` in a record, event or agent body, or `store name: Kind[T] `. Lexical,
/// like the other contextual-word cells; the clause is only a suggestion, so a position it does
/// not belong in (a handler parameter, say) is excluded where the line shows it.
pub fn in_replaces_position(line_prefix: &str) -> bool {
    // The word being typed, which must be the start of `replaces` (or nothing yet), after a space.
    let word_start = line_prefix
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |i| i + 1);
    let (head, word) = line_prefix.split_at(word_start);
    if !"replaces".starts_with(word) || !head.ends_with(char::is_whitespace) {
        return false;
    }
    // A parameter list or a call is open on this line: `fn f(a: Int ` is not a field.
    if head.matches('(').count() > head.matches(')').count() {
        return false;
    }
    // The field is what follows the last `{` or `,` on the line.
    let field = head.rsplit(['{', ',']).next().unwrap_or("").trim();
    let field = field.strip_prefix("store ").unwrap_or(field).trim_start();
    let Some((name, ty)) = field.split_once(':') else {
        return false;
    };
    let name = name.trim();
    let ty = ty.trim();
    !name.is_empty()
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
        && name != "key"
        && ty.starts_with(|c: char| c.is_uppercase())
        && !ty.contains(['=', '@', '"', '>'])
        && !ty
            .split_whitespace()
            .any(|w| w == "replaces" || w == "where")
        && ty.ends_with(|c: char| c.is_alphanumeric() || c == ']')
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: &str = "context shop\n\nagent Cart {\n  key id: String\n  store owner: Cell[String] replaces customer\n\n  on call ping() -> Effect[Int] {\n    1\n  }\n}\n";

    #[test]
    fn the_keyword_is_found_on_a_record_field_and_a_store_field() {
        let kw = |src: &str| -> Vec<String> {
            replaces_keyword_spans(src)
                .iter()
                .map(|s| src[s.start..s.end].to_string())
                .collect()
        };
        assert_eq!(
            kw(
                "commons shop.users\n\ntype User = { displayName: String replaces name, age: Int }\n"
            ),
            ["replaces"]
        );
        assert_eq!(kw(AGENT), ["replaces"]);
        // A name that merely is `replaces` is no clause.
        assert!(kw("commons shop.users\n\nfn f(replaces: Int) -> Int { replaces }\n").is_empty());
    }

    #[test]
    fn the_clause_hovers_on_the_keyword_and_the_old_name_only() {
        let at = |needle: &str, nth: usize| AGENT.match_indices(needle).nth(nth).unwrap().0 + 1;
        assert!(
            describe_replaces_at(AGENT, at("replaces", 0))
                .unwrap()
                .contains("Renamed from `customer`")
        );
        assert!(describe_replaces_at(AGENT, at("customer", 0)).is_some());
        assert!(describe_replaces_at(AGENT, at("owner", 0)).is_none());
    }

    #[test]
    fn replaces_is_offered_after_a_field_type_and_nowhere_it_does_not_belong() {
        for line in [
            "  displayName: String ",
            "  store owner: Cell[String] ",
            "type User = { name: String, label: Option[String] ",
            "  store owner: Cell[String] rep",
            "  total: Int where Positive ",
        ] {
            assert_eq!(
                in_replaces_position(line),
                line != "  total: Int where Positive ",
                "{line}"
            );
        }
        for line in [
            "fn f(a: Int ",
            "  key id: String ",
            "  let x: Int ",
            "  displayName: String replaces name ",
            "  displayName: String = \"x\" ",
            "  displayName: Str",
            "  displayName:",
            "  on call ping() -> Effect[Int] ",
        ] {
            assert!(!in_replaces_position(line), "{line}");
        }
    }
}
