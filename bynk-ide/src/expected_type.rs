//! #4227: completion where an **expected type** is known — offer that type's
//! variants first.
//!
//! The checker records each expression's own type, not the type a position
//! expects, so the expected type is read here from the three places the text
//! names it:
//!
//! - after `lhs ==` / `lhs !=`: the left operand's type (found by the caller
//!   from the round's `expr_types`, at the offset [`comparison_lhs_end`] gives);
//! - after `let x: T =`: the annotation `T`;
//! - in a call argument: the declared type of that parameter of a free
//!   function or static the call names ([`argument_type`]).
//!
//! A sum's variants — and `Result`/`Option`'s — are written bare in all three
//! (a bare variant takes its owner from the expected type: #3344, #3987), so
//! the offered labels are bare.
//!
//! #4334 adds two more: a record field's value (`Rec { field: ‸`, the field's
//! declared type) and the tail of a function or handler body, where the value
//! is the declared return type (Bynk has no `return` statement, so the tail is
//! the return position). A `with`-style update of a bare `{ ...base, f: ‸` has no
//! type name to look the field up by and is not covered.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::completion::{Completion, built_in_variants, for_each_unit, sum_type_variants};
use crate::signature_help::{call_context, param_ranges, resolve_label};
use crate::symbols::type_ref_str;
use bynk_syntax::ast::{CommonsItem, Handler, SourceUnit, TypeBody, TypeRef};

/// After `lhs ==` / `lhs !=` and an optional partial word, the offset of the
/// left operand's last character; `None` anywhere else. `<=`, `>=`, `=>` and a
/// plain `=` are not comparisons.
pub fn comparison_lhs_end(text: &str, offset: usize) -> Option<usize> {
    let before = text.get(..offset)?;
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    let line = &before[line_start..];
    let head = line
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .trim_end();
    let rest = head
        .strip_suffix("==")
        .or_else(|| head.strip_suffix("!="))?;
    // `===`, `<==`, `>==`: not a comparison operator.
    if rest.ends_with(['=', '<', '>', '!']) {
        return None;
    }
    let lhs = rest.trim_end();
    let (i, _) = lhs.char_indices().next_back()?;
    Some(line_start + i)
}

/// After `let name: T =` (and an optional partial word), the text of `T`.
pub fn let_annotation_type(line_prefix: &str) -> Option<String> {
    let head = line_prefix
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .trim_end();
    let before_eq = head.strip_suffix('=')?;
    if before_eq.ends_with(['=', '<', '>', '!']) {
        return None;
    }
    let after_let = before_eq.trim_start().strip_prefix("let ")?;
    let (name, ty) = after_let.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let ty = ty.trim();
    (!ty.is_empty()).then(|| ty.to_string())
}

/// In a call's argument list, the declared type of the parameter being typed,
/// when the callee is a free function or static whose signature resolves.
pub fn argument_type(
    text: &str,
    offset: usize,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    let ctx = call_context(text, offset)?;
    // Only where the argument is just being started: after an operator the
    // operand being typed is not of the parameter's type.
    let typed = text.get(ctx.open_paren + 1..offset)?;
    let current = typed.rsplit(',').next().unwrap_or(typed).trim();
    if !current.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let label = resolve_label(&ctx.callee, text, files)?;
    let (start, end) = *param_ranges(&label).get(ctx.active_param)?;
    let param = label.get(start..end)?;
    let (_, ty) = param.split_once(':')?;
    let ty = ty.trim();
    (!ty.is_empty()).then(|| ty.to_string())
}

/// In `Rec { …, field: ‸` (the field's value just being started, the entries
/// on one line or one per line), the record type's name and the field's.
/// `before` is the document text up to the cursor. Lexical only, so it is cheap
/// enough to ask on every completion request.
pub fn record_field_site(before: &str) -> Option<(&str, &str)> {
    // The innermost `{` still open at the cursor.
    let bytes = before.as_bytes();
    let mut depth = 0i32;
    let mut open = None;
    for i in (0..bytes.len()).rev() {
        match bytes[i] {
            b'}' => depth += 1,
            b'{' if depth == 0 => {
                open = Some(i);
                break;
            }
            b'{' => depth -= 1,
            _ => {}
        }
    }
    let open = open?;
    // The current entry (since the last comma) is `field: <partial word>`.
    let current = before[open + 1..].rsplit(',').next()?;
    let (field, value) = current.split_once(':')?;
    let field = field.trim();
    if field.is_empty() || !field.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    if !value
        .trim()
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    // The record type is the uppercase name just before the `{`.
    let head = before[..open].trim_end();
    let start = head
        .char_indices()
        .rfind(|&(_, c)| !(c.is_alphanumeric() || c == '_'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let record = &head[start..];
    record
        .chars()
        .next()?
        .is_ascii_uppercase()
        .then_some((record, field))
}

/// The declared type of `field` in the record type `record` (a
/// [`record_field_site`]), when `record` is declared in the unit or the project.
pub fn record_field_type(
    (record, field): (&str, &str),
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    let mut found = None;
    for_each_unit(doc_text, files, |unit| {
        let items: &[CommonsItem] = match unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            SourceUnit::Suite(_) => return,
        };
        for item in items {
            if let CommonsItem::Type(t) = item
                && t.name.name == record
                && let TypeBody::Record(r) = &t.body
                && let Some(f) = r.fields.iter().find(|f| f.name.name == field)
            {
                found.get_or_insert_with(|| type_ref_str(&f.type_ref));
            }
        }
    });
    found
}

/// At the tail of a function's or handler's body, where the cursor starts the
/// body's last expression (only a partial word on its line, and nothing but
/// whitespace between it and the closing `}`), the declared return type. A
/// handler's `Effect[T]` is the effect its tail is lifted into, so its tail's
/// expected type is `T`.
pub fn tail_position_type(text: &str, offset: usize) -> Option<String> {
    let line_start = text[..offset].rfind('\n').map_or(0, |i| i + 1);
    let before = &text[line_start..offset];
    if !before
        .trim_start()
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_')
    {
        return None;
    }
    // The rest of the word under the cursor, then only whitespace to the `}`.
    let word_end = offset
        + text[offset..]
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(text.len() - offset);
    let tokens = bynk_syntax::lexer::tokenize(text).ok()?;
    let (Some(unit), _) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, text) else {
        return None;
    };
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => return None,
    };
    let at_tail = |body: &bynk_syntax::ast::Block| {
        body.span.start <= offset
            && offset <= body.span.end
            && text
                .get(word_end..body.span.end.saturating_sub(1))
                .is_some_and(|rest| rest.trim().is_empty())
    };
    let handler_type = |h: &Handler| {
        at_tail(&h.body).then(|| match &h.return_type {
            TypeRef::Effect(inner, _) => type_ref_str(inner),
            other => type_ref_str(other),
        })
    };
    for item in items {
        let found = match item {
            CommonsItem::Fn(f) => at_tail(&f.body).then(|| type_ref_str(&f.return_type)),
            CommonsItem::Service(s) => s.handlers.iter().find_map(handler_type),
            CommonsItem::Agent(a) => a.handlers.iter().find_map(handler_type),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// The variants of the type `ty` is written as: a declared sum's, or the
/// built-in `Result`/`Option`'s. Empty for any other type.
pub fn variants_of_type_text(
    ty: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Vec<Completion> {
    let head = ty.split('[').next().unwrap_or(ty).trim();
    match head {
        "Result" => built_in_variants(&["Ok", "Err"], "Result"),
        "Option" => built_in_variants(&["Some", "None"], "Option"),
        _ => sum_type_variants(head, doc_text, files),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comparison_names_the_left_operands_last_character() {
        let t = "  e == Fa";
        assert_eq!(comparison_lhs_end(t, t.len()), Some(2));
        let t = "f(a) != ";
        assert_eq!(comparison_lhs_end(t, t.len()), Some(3));
        for no in ["x <= ", "x >= ", "x => ", "x = ", "x === "] {
            assert_eq!(comparison_lhs_end(no, no.len()), None, "{no}");
        }
    }

    #[test]
    fn an_annotated_let_names_its_type() {
        assert_eq!(
            let_annotation_type("  let x: Level = Hi").as_deref(),
            Some("Level")
        );
        assert_eq!(
            let_annotation_type("let r: Result[Int, E] =").as_deref(),
            Some("Result[Int, E]")
        );
        assert_eq!(let_annotation_type("let x = "), None);
        assert_eq!(let_annotation_type("let x: Level == "), None);
        assert_eq!(let_annotation_type("x: Level = "), None);
    }

    #[test]
    fn an_argument_names_the_parameters_declared_type() {
        let doc = "commons m\n\ntype Level = | Low | High\n\nfn pick(n: Int, l: Level) -> Int { n }\n\nfn go() -> Int { pick(1, ";
        assert_eq!(
            argument_type(doc, doc.len(), None).as_deref(),
            Some("Level")
        );
        // After an operator the operand is not of the parameter's type.
        let after_op = format!("{doc}n + ");
        assert_eq!(argument_type(&after_op, after_op.len(), None), None);
        let first = doc.replace("pick(1, ", "pick(");
        assert_eq!(
            argument_type(&first, first.len(), None).as_deref(),
            Some("Int")
        );
    }

    #[test]
    fn a_record_field_value_names_the_fields_declared_type() {
        let doc =
            "commons m\n\ntype Level = | Low | High\n\ntype Order = { id: Int, level: Level }\n";
        let at = |before: &str| {
            record_field_site(before).and_then(|site| record_field_type(site, doc, None))
        };
        assert_eq!(
            at("  let o = Order { id: 1, level: Hi").as_deref(),
            Some("Level")
        );
        assert_eq!(at("  Order { level: ").as_deref(), Some("Level"));
        assert_eq!(at("  Order { id: ").as_deref(), Some("Int"));
        // Not a field's value, not a record, or after an operator.
        assert_eq!(at("  Order { lev"), None);
        assert_eq!(at("  Nope { level: "), None);
        assert_eq!(at("  Order { level: a + "), None);
        assert_eq!(at("  Order { missing: "), None);
        // One entry per line, a trailing comma and a partial word.
        assert_eq!(
            at("  let o = Order {\n    id: 1,\n    level: ").as_deref(),
            Some("Level")
        );
        assert_eq!(
            at("  let o = Order {\n    level: Hi").as_deref(),
            Some("Level")
        );
        assert_eq!(at("  let o = Order {\n    id: 1,\n    ").as_deref(), None);
    }

    #[test]
    fn the_tail_of_a_body_names_the_declared_return_type() {
        let src = |body: &str| {
            format!(
                "commons m\n\ntype Level = | Low | High\n\nfn pick(n: Int) -> Level {{\n{body}}}\n"
            )
        };
        let tail = |s: &str, marker: &str| {
            let at = s.find(marker).unwrap() + marker.len();
            tail_position_type(s, at)
        };
        // The tail expression, on an empty line or with a word started.
        let s = src("  let x = n\n  ");
        assert_eq!(tail(&s, "let x = n\n  ").as_deref(), Some("Level"));
        let s = src("  let x = n\n  Lo\n");
        assert_eq!(tail(&s, "  Lo").as_deref(), Some("Level"));
        // Not at the tail: a statement follows, or the line is an expression.
        let s = src("  \n  let x = n\n  x\n");
        assert_eq!(tail(&s, "{\n  "), None);
        let s = src("  let x = Lo\n");
        assert_eq!(tail(&s, "let x = Lo"), None);
    }

    #[test]
    fn a_handlers_tail_is_the_value_inside_its_effect() {
        let src = "context shop\n\ntype Level = | Low | High\n\nservice s {\n  on call(n: Int) -> Effect[Level] by Caller {\n    let x = n\n    \n  }\n}\n";
        let at = src.find("let x = n\n    ").unwrap() + "let x = n\n    ".len();
        assert_eq!(tail_position_type(src, at).as_deref(), Some("Level"));
    }

    #[test]
    fn variants_come_from_the_named_type() {
        let doc = "commons m\n\ntype Level = | Low | High\n";
        let labels = |ty: &str| -> Vec<String> {
            variants_of_type_text(ty, doc, None)
                .into_iter()
                .map(|c| c.label)
                .collect()
        };
        assert_eq!(labels("Level"), ["Low", "High"]);
        assert_eq!(labels("Result[Int, String]"), ["Ok", "Err"]);
        assert_eq!(labels("Option[Level]"), ["Some", "None"]);
        assert!(labels("Int").is_empty());
    }
}
