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
//! Not covered: a return position and a record field's value (each needs the
//! enclosing declaration, not just the line).

use std::collections::HashMap;
use std::path::PathBuf;

use crate::completion::{Completion, built_in_variants, sum_type_variants};
use crate::signature_help::{call_context, param_ranges, resolve_label};

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
    let label = resolve_label(&ctx.callee, text, files)?;
    let (start, end) = *param_ranges(&label).get(ctx.active_param)?;
    let param = label.get(start..end)?;
    let (_, ty) = param.split_once(':')?;
    let ty = ty.trim();
    (!ty.is_empty()).then(|| ty.to_string())
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
        let first = doc.replace("pick(1, ", "pick(");
        assert_eq!(
            argument_type(&first, first.len(), None).as_deref(),
            Some("Int")
        );
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
