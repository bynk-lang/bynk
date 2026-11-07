//! v0.27 (ADR 0056): pure `inlayHint` computation — inferred-type hints
//! from a cached analysis round's harvested hint set.
//!
//! The hints arrive pre-curated from `bynk` (annotation-absent `let` /
//! `let <-` bindings and lambda parameters, labels pre-rendered via
//! `Ty::display()`); this module only filters to the requested visible
//! range and converts positions against the analysed snapshot (the v0.24
//! rule). v0.39 (ADR 0072): the harvested set now carries a [`HintKind`] —
//! **`Type`** hints anchor at the span's **end** (the label's leading `: ` /
//! `[` reads as source: `x: Int`, `identity[Int]`, no padding); **`Parameter`**
//! hints anchor at the argument span's **start** with trailing padding, so the
//! label reads `count: 5`.

use bynk_check::hints::{Hint, HintKind};
use bynk_check::requirements::Requirement;
use bynk_syntax::span::Span;
use std::collections::HashSet;
use tower_lsp::lsp_types::*;

use crate::position::PositionMap;

/// The hints whose anchor falls inside the requested range. `text` is the
/// analysed snapshot the spans are offsets into; `hints` is one file's
/// harvested [`Hint`] set.
pub fn inlay_hints(text: &str, hints: &[Hint], requested: Span) -> Vec<InlayHint> {
    let positions = PositionMap::new(text);
    hints
        .iter()
        .filter_map(|h| {
            let anchor = match h.kind {
                HintKind::Type => h.span.end,
                HintKind::Parameter => h.span.start,
            };
            (requested.start <= anchor && anchor <= requested.end).then(|| {
                let (kind, padding_right) = match h.kind {
                    HintKind::Type => (InlayHintKind::TYPE, None),
                    HintKind::Parameter => (InlayHintKind::PARAMETER, Some(true)),
                };
                InlayHint {
                    position: positions.position(anchor),
                    label: InlayHintLabel::String(h.label.clone()),
                    kind: Some(kind),
                    text_edits: None,
                    tooltip: None,
                    padding_left: None,
                    padding_right,
                    data: None,
                }
            })
        })
        .collect()
}

/// v0.99 (DECISION E): the materializable ghost `given` inlay hints for a file.
///
/// Each **uncovered** capability requirement (its enclosing handler does not
/// declare the capability) renders a ghost clause at the handler's declaration
/// site — `… -> Effect[()]` `«given Clock»` — whose `text_edits` write the real
/// clause via the same `given_insertion_edit` the undeclared-capability
/// quick-fix uses. Deduplicated per `(insertion point, capability)` so one
/// handler that consumes a capability at several sites offers a single ghost.
/// The ghost is positioned where the clause would be inserted, so accepting it
/// reads naturally (`given Clock`, or `, Clock` after an existing clause).
pub fn given_hints(text: &str, requirements: &[Requirement], requested: Span) -> Vec<InlayHint> {
    let positions = PositionMap::new(text);
    let mut seen: HashSet<(usize, String)> = HashSet::new();
    let mut out = Vec::new();
    for req in requirements {
        // Covered requirements carry no materialization — they explain hover,
        // not the ghost clause.
        let Some(m) = &req.materialize else {
            continue;
        };
        let anchor = m.edit_span.start;
        if anchor < requested.start || anchor > requested.end {
            continue;
        }
        if !seen.insert((anchor, req.capability.clone())) {
            continue;
        }
        // The label mirrors the exact insertion (` given Clock` → `given Clock`,
        // `, Clock` → `, Clock`); `padding_left` restores the leading space.
        let label = m.edit_text.trim_start().to_string();
        out.push(InlayHint {
            position: positions.position(anchor),
            label: InlayHintLabel::String(label),
            kind: Some(InlayHintKind::TYPE),
            text_edits: Some(vec![TextEdit {
                range: positions.range(m.edit_span),
                new_text: m.edit_text.clone(),
            }]),
            tooltip: Some(InlayHintTooltip::String(format!(
                "{} — {}",
                req.capability,
                req.source.reason(&req.capability)
            ))),
            padding_left: Some(true),
            padding_right: None,
            data: None,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_check::requirements::{Materialize, RequirementSource, StoreKind};

    fn label_of(h: &InlayHint) -> &str {
        match &h.label {
            InlayHintLabel::String(s) => s,
            other => panic!("expected a plain string label, got {other:?}"),
        }
    }

    fn type_hint(start: usize, end: usize, label: &str) -> Hint {
        Hint {
            span: Span::new(start, end),
            label: label.to_string(),
            kind: HintKind::Type,
        }
    }

    #[test]
    fn type_hints_anchor_after_the_name_with_no_padding() {
        //                0123456789
        let text = "let x = 1\nlet y = 2\n";
        let hints = vec![type_hint(4, 5, ": Int"), type_hint(14, 15, ": Int")];
        let got = inlay_hints(text, &hints, Span::new(0, text.len()));
        assert_eq!(got.len(), 2);
        // Anchored at the end of `x` (line 0, col 5) and `y` (line 1, col 5).
        assert_eq!(got[0].position, Position::new(0, 5));
        assert_eq!(label_of(&got[0]), ": Int");
        assert_eq!(got[0].kind, Some(InlayHintKind::TYPE));
        assert_eq!(got[0].padding_right, None);
        assert_eq!(got[1].position, Position::new(1, 5));
    }

    #[test]
    fn parameter_hints_anchor_before_the_argument_with_trailing_padding() {
        // text: `f(5)` — the argument `5` is at offset 2..3.
        let text = "f(5)\n";
        let hints = vec![Hint {
            span: Span::new(2, 3),
            label: "count:".to_string(),
            kind: HintKind::Parameter,
        }];
        let got = inlay_hints(text, &hints, Span::new(0, text.len()));
        assert_eq!(got.len(), 1);
        // Anchored at the *start* of the argument (col 2), with trailing space.
        assert_eq!(got[0].position, Position::new(0, 2));
        assert_eq!(label_of(&got[0]), "count:");
        assert_eq!(got[0].kind, Some(InlayHintKind::PARAMETER));
        assert_eq!(got[0].padding_right, Some(true));
    }

    #[test]
    fn out_of_range_hints_are_filtered() {
        let text = "let x = 1\nlet y = 2\n";
        let hints = vec![type_hint(4, 5, ": Int"), type_hint(14, 15, ": Int")];
        // Only the first line is visible.
        let got = inlay_hints(text, &hints, Span::new(0, 9));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].position, Position::new(0, 5));
    }

    #[test]
    fn empty_hint_set_returns_empty() {
        assert!(inlay_hints("let x = 1\n", &[], Span::new(0, 9)).is_empty());
    }

    fn uncovered(cap: &str, site: usize, anchor: usize, edit_text: &str) -> Requirement {
        Requirement {
            capability: cap.to_string(),
            site: Span::new(site, site + 1),
            source: RequirementSource::StoreOp {
                kind: StoreKind::Cache,
                op: "put".to_string(),
            },
            covered: false,
            materialize: Some(Materialize {
                anchor: Span::new(anchor, anchor),
                edit_span: Span::new(anchor, anchor),
                edit_text: edit_text.to_string(),
            }),
        }
    }

    #[test]
    fn ghost_given_renders_at_the_insertion_point_with_a_materialization_edit() {
        // `-> Effect[()]` ends at offset 13; the ghost renders there.
        let text = "on call f() -> Effect[()] {\n}\n";
        let reqs = vec![uncovered("Clock", 20, 25, " given Clock")];
        let got = given_hints(text, &reqs, Span::new(0, text.len()));
        assert_eq!(got.len(), 1);
        assert_eq!(label_of(&got[0]), "given Clock");
        assert_eq!(got[0].padding_left, Some(true));
        let edits = got[0].text_edits.as_ref().expect("materialization edit");
        assert_eq!(edits.len(), 1);
        assert_eq!(edits[0].new_text, " given Clock");
    }

    #[test]
    fn ghost_given_dedups_per_handler_and_capability() {
        // Two uncovered Clock sites in one handler → one ghost (same anchor).
        let text = "on call f() -> Effect[()] {\n}\n";
        let reqs = vec![
            uncovered("Clock", 20, 25, " given Clock"),
            uncovered("Clock", 30, 25, " given Clock"),
        ];
        let got = given_hints(text, &reqs, Span::new(0, text.len()));
        assert_eq!(got.len(), 1, "deduped to a single ghost");
    }

    #[test]
    fn covered_requirements_render_no_ghost() {
        let text = "on call f() -> Effect[()] given Clock {\n}\n";
        let covered = Requirement {
            capability: "Clock".to_string(),
            site: Span::new(20, 21),
            source: RequirementSource::StoreOp {
                kind: StoreKind::Cache,
                op: "put".to_string(),
            },
            covered: true,
            materialize: None,
        };
        assert!(given_hints(text, &[covered], Span::new(0, text.len())).is_empty());
    }
}
