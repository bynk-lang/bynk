//! #3972: a refined pattern on an `Int` that is a range pattern.
//!
//! `_ where InRange(200, 299)` predates range patterns (#2699), and on an `Int`
//! it is strictly weaker than `200..299`: a refined pattern is a guard and never
//! counts towards exhaustiveness, while a range does and reports the intervals a
//! match is missing. The warning names the range the pattern denotes and offers
//! it as a machine-applicable fix. A refined pattern stays where a range cannot
//! go (`Float`, `Decimal`, `String`), and a pattern whose predicates are not one
//! interval is left alone.

use bynk_syntax::ast::{IntRange, Pattern, PredKind, Refinement};
use bynk_syntax::error::{Applicability, CompileError};
use bynk_syntax::span::Span;

/// The inclusive interval the predicates of an `Int` refinement admit together, as
/// `(low, high)` bounds with `None` for an open end. `None` when a predicate is not
/// an interval (anything but `InRange`, `Positive` and `NonNegative`), or when the
/// intersection is empty, since neither is a single range pattern.
fn interval(predicate: &Refinement) -> Option<(Option<i64>, Option<i64>)> {
    let (mut lo, mut hi): (Option<i64>, Option<i64>) = (None, None);
    let raise = |lo: &mut Option<i64>, to: i64| *lo = Some(lo.map_or(to, |l| l.max(to)));
    let lower = |hi: &mut Option<i64>, to: i64| *hi = Some(hi.map_or(to, |h| h.min(to)));
    for p in &predicate.predicates {
        match &p.kind {
            PredKind::InRange(a, b) => {
                raise(&mut lo, a.value);
                lower(&mut hi, b.value);
            }
            PredKind::Positive => raise(&mut lo, 1),
            PredKind::NonNegative => raise(&mut lo, 0),
            _ => return None,
        }
    }
    match (lo, hi) {
        (Some(l), Some(h)) if l > h => None,
        (None, None) => None,
        bounds => Some(bounds),
    }
}

/// `bynk.match.refined_pattern_is_range` for `_ where …` over an `Int` scrutinee
/// whose predicates are a single interval, or `None` for any other pattern. Only a
/// wildcard inner pattern is reported: a name binding would be lost by a range.
pub(crate) fn refined_pattern_is_range(
    inner: &Pattern,
    predicate: &Refinement,
    span: Span,
) -> Option<CompileError> {
    if !matches!(inner, Pattern::Wildcard(_)) {
        return None;
    }
    let (lo, hi) = interval(predicate)?;
    let range = IntRange {
        lo,
        hi,
        lo_text: None,
        hi_text: None,
        span,
    }
    .describe();
    Some(
        CompileError::new(
            "bynk.match.refined_pattern_is_range",
            span,
            format!("this refined pattern is the range pattern `{range}`"),
        )
        .with_note(
            "a range pattern counts towards exhaustiveness; a refined pattern never does, so \
             after the rewrite a later arm that the range now covers is unreachable and must \
             be deleted",
        )
        .with_suggestion(
            format!("replace with `{range}`"),
            vec![(span, range)],
            Applicability::MaybeIncorrect,
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::interval;
    use bynk_syntax::ast::{IntBound, PredKind, Refinement, RefinementPred};
    use bynk_syntax::span::Span;

    fn sp() -> Span {
        Span::new_in(Default::default(), 0, 0)
    }

    fn bound(v: i64) -> IntBound {
        IntBound {
            value: v,
            lexeme: v.to_string(),
            span: sp(),
        }
    }

    fn refinement(kinds: Vec<PredKind>) -> Refinement {
        Refinement {
            predicates: kinds
                .into_iter()
                .map(|kind| RefinementPred {
                    kind,
                    arg_text: None,
                    span: sp(),
                })
                .collect(),
            annotations: Vec::new(),
            span: sp(),
        }
    }

    #[test]
    fn each_predicate_is_its_interval() {
        let r = |k| interval(&refinement(vec![k]));
        assert_eq!(
            r(PredKind::InRange(bound(200), bound(299))),
            Some((Some(200), Some(299)))
        );
        assert_eq!(r(PredKind::Positive), Some((Some(1), None)));
        assert_eq!(r(PredKind::NonNegative), Some((Some(0), None)));
    }

    #[test]
    fn a_combined_predicate_is_the_intersection_when_it_is_one_interval() {
        let both = refinement(vec![
            PredKind::Positive,
            PredKind::InRange(bound(-5), bound(10)),
        ]);
        assert_eq!(interval(&both), Some((Some(1), Some(10))));
    }

    #[test]
    fn an_empty_intersection_or_a_non_interval_predicate_is_left_alone() {
        let empty = refinement(vec![
            PredKind::InRange(bound(1), bound(3)),
            PredKind::InRange(bound(5), bound(9)),
        ]);
        assert_eq!(interval(&empty), None);
        let other = refinement(vec![PredKind::Positive, PredKind::NonEmpty]);
        assert_eq!(interval(&other), None);
    }
}
