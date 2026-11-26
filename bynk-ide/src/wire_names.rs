//! The editor surfaces of wire names (#3975): `@name("…")` before a record field, a sum variant or
//! a variant's payload field, and `@discriminator("…")` before a sum's `type`.
//!
//! The annotations are parsed, not matched by spelling, so a field called `name` or a handler's own
//! annotation never gets these: hover says what the name is on the wire, the semantic token marks
//! the annotation as a decorator, and completion offers the two annotations where they can go.

use bynk_syntax::ast::{Annotation, CommonsItem, SourceUnit, TypeBody, annotation_string};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;
use bynk_syntax::span::Span;

/// What a `@name` hover says, whichever position it is in.
pub const NAME_DOC: &str = "The key (or, on a variant, the value) this has on the wire, in every encoding of the type: HTTP bodies and results, `Json`, `Yaml` and `Form`, cross-context calls, queues, events and stored state. Bynk code keeps using the Bynk name; only the codecs, OpenAPI and decode error paths use the wire name.";

/// What a `@discriminator` hover says.
pub const DISCRIMINATOR_DOC: &str = "The JSON key that carries this sum's variant, instead of `kind`. The value under it is the variant's wire name (its `@name`, or its own name). Option and Result keep their fixed shapes.";

/// One wire-name annotation of the unit, with the text its hover shows.
struct Found {
    /// The whole annotation, `@` through the closing parenthesis.
    span: Span,
    /// `@` through the annotation's name, which is what the semantic token covers.
    head: Span,
    hover: String,
}

fn found(ann: &Annotation, what: String) -> Option<Found> {
    let wire = annotation_string(std::slice::from_ref(ann), &ann.name.name)?;
    let (doc, body) = match ann.name.name.as_str() {
        "name" => (NAME_DOC, format!("`{what}` is `{wire}` on the wire.")),
        "discriminator" => (
            DISCRIMINATOR_DOC,
            format!("The variants of `{what}` are told apart by the key `{wire}`."),
        ),
        _ => return None,
    };
    Some(Found {
        span: ann.span,
        head: Span {
            file: ann.span.file,
            start: ann.span.start,
            end: ann.name.span.end,
        },
        hover: format!(
            "```bynk\n@{}(\"{wire}\")\n```\n\n{body} {doc}",
            ann.name.name
        ),
    })
}

/// Every wire-name annotation of the unit's types, in source order.
fn annotations(source: &str) -> Vec<Found> {
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
    let mut out: Vec<Found> = Vec::new();
    let mut add = |ann: &Annotation, what: String| out.extend(found(ann, what));
    for item in items {
        match item {
            CommonsItem::Type(t) => {
                for ann in &t.annotations {
                    add(ann, t.name.name.clone());
                }
                match &t.body {
                    TypeBody::Record(r) => {
                        for f in &r.fields {
                            for ann in &f.annotations {
                                add(ann, format!("{}.{}", t.name.name, f.name.name));
                            }
                        }
                    }
                    TypeBody::Sum(s) => {
                        for v in &s.variants {
                            for ann in &v.annotations {
                                add(ann, format!("{}.{}", t.name.name, v.name.name));
                            }
                            for p in &v.payload {
                                for ann in &p.annotations {
                                    add(
                                        ann,
                                        format!("{}.{}.{}", t.name.name, v.name.name, p.name.name),
                                    );
                                }
                            }
                        }
                    }
                    TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
                }
            }
            CommonsItem::Event(e) => {
                for f in &e.body.fields {
                    for ann in &f.annotations {
                        add(ann, format!("{}.{}", e.name.name, f.name.name));
                    }
                }
            }
            _ => {}
        }
    }
    out.sort_by_key(|f| f.span.start);
    out
}

/// The `@` through the name of each annotation, for the `decorator` semantic token.
pub fn wire_annotation_token_spans(source: &str) -> Vec<Span> {
    annotations(source).into_iter().map(|f| f.head).collect()
}

/// Hover anywhere within a wire-name annotation. `None` elsewhere.
pub fn describe_wire_annotation_at(source: &str, offset: usize) -> Option<String> {
    annotations(source)
        .into_iter()
        .find(|f| f.span.start <= offset && offset < f.span.end)
        .map(|f| f.hover)
}

/// What the cursor is completing after an `@`, or `None` when it is not at an annotation a wire
/// name can be: `("name", ...)` before a field or a variant, `("discriminator", ...)` before a
/// sum's `type` at the start of a line. Lexical, like the other annotation cells.
pub fn in_wire_annotation_position(line_prefix: &str) -> Option<WirePosition> {
    let at = line_prefix.rfind('@')?;
    let (head, word) = line_prefix.split_at(at);
    let word = &word[1..];
    if !word.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let before = head.trim_end();
    // Between a record's braces, a variant's parentheses or an enum's braces, at the start of a
    // field, or right after a pipe-form `|`: a field's or a variant's `@name`.
    let starts_member =
        before.is_empty() || before.ends_with(['{', ',', '|', '(']) || before.ends_with("enum {");
    if !starts_member {
        return None;
    }
    // At the very start of a line, outside any member, is an item: a `type`'s annotation.
    if before.is_empty() && head.is_empty() {
        return Some(WirePosition::Item);
    }
    // A `(` opens a variant's payload only when a capitalised name after a `|` precedes it; a
    // handler's parameter list or a call is not a field.
    if let Some(open) = before.strip_suffix('(') {
        let name = open
            .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
            .next()
            .unwrap_or("");
        if !(name.starts_with(|c: char| c.is_ascii_uppercase()) && open.contains('|')) {
            return None;
        }
    }
    Some(WirePosition::Member)
}

/// Where a wire-name annotation is being typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WirePosition {
    /// Before a record field, a variant or a payload field: `@name`.
    Member,
    /// At the start of a line, where a `type`'s annotation goes: `@discriminator`.
    Item,
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "commons shapes\n\ntype Charge = { id: String, @name(\"amount_captured\") captured: Int }\n\n@discriminator(\"type\")\ntype Hook =\n\t| @name(\"charge.succeeded\") ChargeSucceeded(@name(\"d\") data: Charge)\n\t| Ping\n";

    #[test]
    fn every_wire_name_annotation_is_a_decorator() {
        let heads: Vec<&str> = wire_annotation_token_spans(SRC)
            .iter()
            .map(|s| &SRC[s.start..s.end])
            .collect();
        assert_eq!(heads, ["@name", "@discriminator", "@name", "@name"]);
    }

    #[test]
    fn hover_says_what_the_name_is_on_the_wire() {
        let at = SRC.find("amount_captured").unwrap();
        let hover = describe_wire_annotation_at(SRC, at).unwrap();
        assert!(hover.contains("`Charge.captured` is `amount_captured` on the wire"));
        let at = SRC.find("@discriminator").unwrap() + 3;
        let hover = describe_wire_annotation_at(SRC, at).unwrap();
        assert!(hover.contains("The variants of `Hook` are told apart by the key `type`"));
        let at = SRC.find("charge.succeeded").unwrap();
        assert!(
            describe_wire_annotation_at(SRC, at)
                .unwrap()
                .contains("`Hook.ChargeSucceeded` is `charge.succeeded`")
        );
        // Not inside an annotation: no hover of ours.
        let at = SRC.find("captured: Int").unwrap();
        assert_eq!(describe_wire_annotation_at(SRC, at), None);
    }

    #[test]
    fn a_field_called_name_is_no_annotation() {
        let src = "commons c\n\ntype A = { name: String }\n";
        assert!(wire_annotation_token_spans(src).is_empty());
    }

    #[test]
    fn the_annotation_is_offered_where_it_can_go() {
        use WirePosition::*;
        assert_eq!(in_wire_annotation_position("@"), Some(Item));
        assert_eq!(in_wire_annotation_position("@disc"), Some(Item));
        assert_eq!(
            in_wire_annotation_position("type A = { id: String, @"),
            Some(Member)
        );
        assert_eq!(in_wire_annotation_position("\t| @na"), Some(Member));
        assert_eq!(
            in_wire_annotation_position("type A = enum { @"),
            Some(Member)
        );
        assert_eq!(in_wire_annotation_position("\t| Circle(@"), Some(Member));
        // After a field's type, or inside a handler's parameters, it is not.
        assert_eq!(in_wire_annotation_position("type A = { id: String @"), None);
        assert_eq!(in_wire_annotation_position("fn f(@"), None);
    }
}
