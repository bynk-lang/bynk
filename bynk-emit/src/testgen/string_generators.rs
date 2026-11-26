//! #4121: the generator spec of a `String` refined by `Matches` or `IsoDate`.
//!
//! The spec is one JSON string the test runtime reads (`rng.matches`, `__bynkMatchBoundary`,
//! `__bynkShrinkMatch`): the pattern's walk tree, the patterns every drawn value must also
//! match, whether it must be a calendar date, and the length bounds. The runtime re-checks each
//! draw against the patterns, so a walk that disagrees with the engine fails loudly.

use bynk_syntax::ast::{PredKind, Refinement};

/// `text` as a JSON string literal.
fn json_str(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str("\\u00");
                out.push(char::from_digit((c as u32) >> 4, 16).unwrap_or('0'));
                out.push(char::from_digit((c as u32) & 0xF, 16).unwrap_or('0'));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `["<source>","<flags>"]` for a `Matches` argument (its inline flag group split off).
fn pattern_pair(literal: &str) -> String {
    let (src, flags) = bynk_check::checker::split_regex_flags(literal);
    ["[", &json_str(&src), ",", &json_str(&flags), "]"].concat()
}

/// The spec for a refinement with a `Matches` or an `IsoDate` predicate, or `None` for any other
/// string. A `Matches` the walk cannot honour never reaches here: the checker pins it.
pub(super) fn string_gen_spec(refinement: Option<&Refinement>) -> Option<String> {
    let r = refinement?;
    let mut patterns: Vec<&String> = Vec::new();
    let mut iso = false;
    let (mut min, mut max) = (0i64, -1i64);
    for p in &r.predicates {
        match &p.kind {
            PredKind::Matches(pat) => patterns.push(pat),
            PredKind::IsoDate => iso = true,
            PredKind::NonEmpty => min = min.max(1),
            PredKind::MinLength(k) => min = min.max(*k),
            PredKind::Length(k) => {
                min = min.max(*k);
                max = if max < 0 { *k } else { max.min(*k) };
            }
            PredKind::MaxLength(k) => max = if max < 0 { *k } else { max.min(*k) },
            PredKind::InRange(..)
            | PredKind::InRangeF(..)
            | PredKind::InRangeD(..)
            | PredKind::InRangeDur(..)
            | PredKind::AtLeast(_)
            | PredKind::AtMost(_)
            | PredKind::NonNegative
            | PredKind::Positive
            | PredKind::Cron => {}
        }
    }
    if patterns.is_empty() && !iso {
        return None;
    }
    // The first pattern is walked; the others, and the date rule, filter what it draws.
    let walked = patterns
        .first()
        .and_then(|lit| bynk_check::regex_walk::parse_walkable(lit).ok());
    let (tree, own, others): (String, String, Vec<&String>) =
        match (&walked, patterns.split_first()) {
            (Some(walk), Some((first, rest))) => (
                bynk_check::regex_walk::walk_json(walk),
                pattern_pair(first),
                rest.to_vec(),
            ),
            _ if iso => ("null".to_string(), "null".to_string(), patterns),
            _ => return None,
        };
    let others: Vec<String> = others.iter().map(|p| pattern_pair(p)).collect();
    Some(
        [
            "{\"t\":",
            &tree,
            ",\"p\":",
            &own,
            ",\"o\":[",
            &others.join(","),
            "],\"iso\":",
            if iso { "true" } else { "false" },
            ",\"min\":",
            &min.to_string(),
            ",\"max\":",
            &max.to_string(),
            "}",
        ]
        .concat(),
    )
}
