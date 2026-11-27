//! #3973: the literal patterns `Decimal` admits and `Float` refuses.
//!
//! A `Decimal` literal pattern matches by value, as `==` does, so `20d`, `20.0d` and `20.00d`
//! are one pattern; like a `String` literal it never completes the type. A `Float` literal is
//! parsed so that the refusal can say why: floating-point equality is exact and `NaN` never
//! equals itself, so a literal arm rarely means what it seems.

use bynk_syntax::ast::{LiteralValue, Pattern};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

/// `bynk.types.float_literal_pattern` for a `Float` literal pattern, at any depth, or `None` for
/// every other literal.
pub(crate) fn float_literal_pattern(value: &LiteralValue, span: Span) -> Option<CompileError> {
    let LiteralValue::Float(text) = value else {
        return None;
    };
    Some(
        CompileError::new(
            "bynk.types.float_literal_pattern",
            span,
            format!("a `Float` can't be matched against the literal `{text}`"),
        )
        .with_note(
            "floating-point equality is exact and `NaN` never equals itself, so a literal arm rarely means what it seems",
        )
        .with_note(
            "a `Float` is not a `match` scrutinee, so compare it with `<`, `>=` or a tolerance in an `if` instead (`if x < 0.5 { … }`)",
        ),
    )
}

/// A `Float` literal pattern, which `check_match` hands to the pattern's own check so that its
/// error says why, rather than naming the scrutinee.
pub(crate) fn is_float_literal(p: &Pattern) -> bool {
    matches!(
        p,
        Pattern::Literal {
            value: LiteralValue::Float(_),
            ..
        }
    )
}

/// Whether the duplicate-arm check leaves an alternative alone: an irrefutable one, a list or a
/// range (the usefulness check judges their overlap), and a `Decimal` literal, whose repeat by
/// value (`20d` after `20.0d`) is unreachable rather than a duplicate (#3973).
pub(crate) fn skips_duplicate_check(p: &Pattern) -> bool {
    p.is_irrefutable()
        || matches!(
            p,
            Pattern::List { .. }
                | Pattern::Range(_)
                | Pattern::Literal {
                    value: LiteralValue::Decimal { .. },
                    ..
                }
        )
}
