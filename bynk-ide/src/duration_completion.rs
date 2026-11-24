//! #3947: completion after an integer literal's `.` — the `Duration` units.
//!
//! An integer literal has no fields, so `<int>.<cursor>` can only be a
//! `Duration` literal. The unit offered agrees with the number: the singular
//! units after `1`, the plural ones after any other integer.

use crate::completion::{Completion, CompletionKind};
use bynk_syntax::ast::DurationUnit;

/// The `Duration` units to offer when the text before the cursor ends in
/// `<int>.<partial>`, or `None` when it does not.
pub(crate) fn duration_unit_candidates(line: &str) -> Option<Vec<Completion>> {
    let head = line
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .strip_suffix('.')?;
    let start = head
        .char_indices()
        .rfind(|&(_, c)| !(c.is_ascii_digit() || c == '_'))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let digits = &head[start..];
    // A number, not part of an identifier (`x1.`), a float (`1.5.`) or a path.
    if digits.is_empty() || !digits.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let before = &head[..start];
    if before.ends_with(|c: char| c.is_alphanumeric() || c == '_' || c == '.') {
        return None;
    }
    let value: i64 = digits.replace('_', "").parse().ok()?;
    Some(
        DurationUnit::ALL
            .iter()
            .map(|u| {
                Completion::item(
                    u.name_for(value),
                    CompletionKind::Member,
                    Some("`Duration` unit".to_string()),
                )
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(line: &str) -> Option<Vec<String>> {
        duration_unit_candidates(line).map(|v| v.into_iter().map(|c| c.label).collect())
    }

    #[test]
    fn one_offers_the_singular_units() {
        assert_eq!(
            labels("  let d = 1.").unwrap(),
            ["millisecond", "second", "minute", "hour", "day"]
        );
        assert_eq!(labels("  let d = 1.ho").unwrap()[3], "hour");
    }

    #[test]
    fn any_other_integer_offers_the_plural_units() {
        assert_eq!(
            labels("  within(5.").unwrap(),
            ["milliseconds", "seconds", "minutes", "hours", "days"]
        );
        assert_eq!(labels("  0.").unwrap()[1], "seconds");
        assert_eq!(labels("  1_000.").unwrap()[0], "milliseconds");
    }

    #[test]
    fn only_after_an_integer_literal() {
        assert!(labels("  x.").is_none());
        assert!(labels("  x1.").is_none());
        assert!(labels("  1.5.").is_none());
        assert!(labels("  a.1.").is_none());
        assert!(labels("  Color.").is_none());
    }
}
