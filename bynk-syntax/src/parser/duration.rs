//! #3947: the unit of a `<int>.<unit>` `Duration` literal. A literal uses the
//! unit that agrees with its number — `1.hour`, `5.hours` — and an integer
//! literal followed by `.<identifier>` that names no unit is its own error,
//! `bynk.duration.unknown_unit`, rather than a field access on an `Int`.
//!
//! Shared by the two parser sites: an expression literal and a scheme config
//! literal (`ttl = 12.hours`).

use crate::ast::{DurationUnit, UnitForm};
use crate::error::{Applicability, CompileError};
use crate::span::Span;

/// The unit a literal `value.name` denotes, or the diagnostic for a wrong form
/// (`bynk.duration.unit_agreement`) or an unknown name
/// (`bynk.duration.unknown_unit`). `name_span` covers the word after the `.`;
/// both diagnostics carry a fix-it over it.
pub(super) fn resolve_duration_unit(
    value: i64,
    name: &str,
    name_span: Span,
) -> Result<DurationUnit, CompileError> {
    let want = if value == 1 {
        UnitForm::Singular
    } else {
        UnitForm::Plural
    };
    let Some((unit, form)) = DurationUnit::from_name(name) else {
        return Err(unknown_unit(value, name, name_span));
    };
    if form == want {
        return Ok(unit);
    }
    let right = unit.name_for(value);
    let (rule, why) = match want {
        UnitForm::Singular => ("singular", "`1` takes the singular unit"),
        UnitForm::Plural => ("plural", "a number other than `1` takes the plural unit"),
    };
    Err(CompileError::new(
        "bynk.duration.unit_agreement",
        name_span,
        format!("{why}: `{value}.{right}`, not `{value}.{name}`"),
    )
    .with_note(format!(
        "a `Duration` literal uses the {rule} unit with this number"
    ))
    .with_suggestion(
        format!("write `{value}.{right}`"),
        vec![(name_span, right.to_string())],
        Applicability::MachineApplicable,
    ))
}

fn unknown_unit(value: i64, name: &str, name_span: Span) -> CompileError {
    let units = DurationUnit::ALL
        .iter()
        .map(|u| format!("`{}`/`{}`", u.singular_name(), u.name()))
        .collect::<Vec<_>>()
        .join(", ");
    let mut err = CompileError::new(
        "bynk.duration.unknown_unit",
        name_span,
        format!("`{name}` is not a `Duration` unit"),
    )
    .with_note(format!(
        "an integer literal has no fields, so `{value}.{name}` is read as a `Duration` literal; the units are {units}"
    ));
    if let Some(unit) = nearest_unit(name) {
        let right = unit.name_for(value);
        err = err.with_suggestion(
            format!("did you mean `{value}.{right}`?"),
            vec![(name_span, right.to_string())],
            Applicability::MaybeIncorrect,
        );
    }
    err
}

/// The unit `name` most plausibly meant: a close spelling (edit distance of at
/// most 2 from either form), or an abbreviation whose letters appear in order
/// in the unit and start with the same letter (`hr` for `hours`).
fn nearest_unit(name: &str) -> Option<DurationUnit> {
    let name = name.to_ascii_lowercase();
    let mut best: Option<(usize, DurationUnit)> = None;
    for u in DurationUnit::ALL {
        let d = edit_distance(&name, u.name()).min(edit_distance(&name, u.singular_name()));
        let abbreviation = name.len() >= 2 && is_subsequence(&name, u.name());
        let score = if d <= 2 {
            d
        } else if u.name().starts_with(&name) {
            2
        } else if abbreviation {
            3
        } else {
            continue;
        };
        if best.is_none_or(|(b, _)| score < b) {
            best = Some((score, u));
        }
    }
    best.map(|(_, u)| u)
}

/// Whether `short`'s characters occur in `long` in order, starting with the
/// same first character.
fn is_subsequence(short: &str, long: &str) -> bool {
    if short.chars().next() != long.chars().next() {
        return false;
    }
    let mut rest = long.chars();
    short.chars().all(|c| rest.any(|l| l == c))
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = (prev + usize::from(ca != *cb)).min(row[j] + 1).min(cur + 1);
            prev = cur;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> Span {
        Span::new(0, 1)
    }

    #[test]
    fn singular_with_one_and_plural_otherwise() {
        assert_eq!(
            resolve_duration_unit(1, "hour", at()).unwrap(),
            DurationUnit::Hours
        );
        assert_eq!(
            resolve_duration_unit(5, "hours", at()).unwrap(),
            DurationUnit::Hours
        );
        assert_eq!(
            resolve_duration_unit(0, "seconds", at()).unwrap(),
            DurationUnit::Seconds
        );
    }

    #[test]
    fn the_wrong_form_is_a_unit_agreement_error() {
        let e = resolve_duration_unit(1, "hours", at()).unwrap_err();
        assert_eq!(e.category, "bynk.duration.unit_agreement");
        assert_eq!(e.suggestions[0].edits[0].1, "hour");
        let e = resolve_duration_unit(5, "hour", at()).unwrap_err();
        assert_eq!(e.suggestions[0].edits[0].1, "hours");
        let e = resolve_duration_unit(0, "second", at()).unwrap_err();
        assert_eq!(e.suggestions[0].edits[0].1, "seconds");
    }

    #[test]
    fn an_unknown_word_is_unknown_unit_with_the_nearest_suggestion() {
        let e = resolve_duration_unit(5, "hr", at()).unwrap_err();
        assert_eq!(e.category, "bynk.duration.unknown_unit");
        assert_eq!(e.suggestions[0].edits[0].1, "hours");
        let e = resolve_duration_unit(1, "min", at()).unwrap_err();
        assert_eq!(e.suggestions[0].edits[0].1, "minute");
        let e = resolve_duration_unit(1, "week", at()).unwrap_err();
        assert_eq!(e.category, "bynk.duration.unknown_unit");
        assert!(e.suggestions.is_empty());
    }
}
