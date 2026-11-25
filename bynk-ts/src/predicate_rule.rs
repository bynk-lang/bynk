//! #3952: the `ValidationRule` a refinement check names when it fails.
//!
//! [`pred_condition_and_message`](crate::pred_condition_and_message) is the one
//! `PredKind` → check mapping; this is its companion, the one `PredKind` → rule
//! mapping, so the owner-side `.of` check and the boundary-side decoder build the
//! same value by construction. The rule is a plain tagged object in the shape
//! the compiler gives a sum (`{ tag: "MaxLength", limit: 280 }`), which is what
//! `bynk.validation`'s `ValidationRule` compiles to, so a `match` on `e.rule`
//! reads it with no conversion and no import in the emitted module.

use crate::predicate::decimal_signed_coef;
use crate::program::{TsExpr, TsLit, TsObjectEntry};
use bynk_syntax::ast::{PredBound, PredKind};

/// `{ tag: "<name>" }`, then `entries` after it.
fn tagged(name: &str, entries: Vec<(&str, TsExpr)>) -> TsExpr {
    let mut all = vec![TsObjectEntry::prop(
        "tag",
        TsExpr::Lit(TsLit::Str(name.to_string())),
    )];
    all.extend(
        entries
            .into_iter()
            .map(|(key, value)| TsObjectEntry::prop(key, value)),
    );
    TsExpr::object_entries(all)
}

fn number(text: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Num(text.into()))
}

/// A bound as the `Bound` sum: `{ tag: "IntBound", value: 1 }`. A `Decimal` is
/// the runtime's own `{ coefficient, scale }` object and a `Duration` its
/// milliseconds, as those types erase everywhere else.
fn bound_expr(bound: &PredBound) -> TsExpr {
    match bound {
        PredBound::Int(b) => tagged("IntBound", vec![("value", number(b.value.to_string()))]),
        PredBound::Float(b) => tagged("FloatBound", vec![("value", number(b.lexeme.clone()))]),
        PredBound::Decimal(b) => tagged(
            "DecimalBound",
            vec![(
                "value",
                TsExpr::object_entries(vec![
                    TsObjectEntry::prop(
                        "coefficient",
                        TsExpr::bigint(decimal_signed_coef(&b.coef)),
                    ),
                    TsObjectEntry::prop("scale", number(b.scale.to_string())),
                ]),
            )],
        ),
        PredBound::Duration(b) => tagged(
            "DurationBound",
            vec![("value", number(b.millis.to_string()))],
        ),
    }
}

/// The rule `pred` enforces, as the expression a failing check puts in `rule`.
pub fn pred_rule_expr(pred: &PredKind) -> TsExpr {
    let int = |b: &bynk_syntax::ast::IntBound| bound_expr(&PredBound::Int(b.clone()));
    let float = |b: &bynk_syntax::ast::FloatBound| bound_expr(&PredBound::Float(b.clone()));
    let decimal = |b: &bynk_syntax::ast::DecimalBound| bound_expr(&PredBound::Decimal(b.clone()));
    let duration =
        |b: &bynk_syntax::ast::DurationBound| bound_expr(&PredBound::Duration(b.clone()));
    match pred {
        PredKind::NonNegative => tagged("NonNegative", vec![]),
        PredKind::Positive => tagged("Positive", vec![]),
        PredKind::NonEmpty => tagged("NonEmpty", vec![]),
        PredKind::IsoDate => tagged("IsoDate", vec![]),
        PredKind::Cron => tagged("Cron", vec![]),
        PredKind::InRange(a, b) => tagged("InRange", vec![("lo", int(a)), ("hi", int(b))]),
        PredKind::InRangeF(a, b) => tagged("InRange", vec![("lo", float(a)), ("hi", float(b))]),
        PredKind::InRangeD(a, b) => tagged("InRange", vec![("lo", decimal(a)), ("hi", decimal(b))]),
        PredKind::InRangeDur(a, b) => {
            tagged("InRange", vec![("lo", duration(a)), ("hi", duration(b))])
        }
        PredKind::AtLeast(b) => tagged("AtLeast", vec![("bound", bound_expr(b))]),
        PredKind::AtMost(b) => tagged("AtMost", vec![("bound", bound_expr(b))]),
        PredKind::MinLength(n) => tagged("MinLength", vec![("limit", number(n.to_string()))]),
        PredKind::MaxLength(n) => tagged("MaxLength", vec![("limit", number(n.to_string()))]),
        PredKind::Length(n) => tagged("Length", vec![("limit", number(n.to_string()))]),
        PredKind::Matches(pattern) => tagged(
            "Matches",
            vec![("pattern", TsExpr::Lit(TsLit::Str(pattern.clone())))],
        ),
    }
}

/// The rule of a value that is not a valid member of its base type at all
/// (`WellFormed`), before any predicate was tried.
pub fn well_formed_rule_expr() -> TsExpr {
    tagged("WellFormed", vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::print_expr;
    use bynk_syntax::ast::IntBound;
    use bynk_syntax::span::Span;

    fn int(v: i64) -> IntBound {
        IntBound {
            value: v,
            lexeme: v.to_string(),
            span: Span::new_in(Default::default(), 0, 0),
        }
    }

    #[test]
    fn a_length_rule_carries_its_limit() {
        assert_eq!(
            print_expr(&pred_rule_expr(&PredKind::MaxLength(280))),
            "{ tag: \"MaxLength\", limit: 280 }"
        );
    }

    #[test]
    fn a_range_rule_carries_typed_bounds() {
        assert_eq!(
            print_expr(&pred_rule_expr(&PredKind::InRange(int(1), int(100)))),
            "{ tag: \"InRange\", lo: { tag: \"IntBound\", value: 1 }, hi: { tag: \"IntBound\", value: 100 } }"
        );
    }

    #[test]
    fn a_rule_without_an_argument_is_just_its_tag() {
        assert_eq!(
            print_expr(&pred_rule_expr(&PredKind::NonEmpty)),
            "{ tag: \"NonEmpty\" }"
        );
    }
}
