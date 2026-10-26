//! #2476: pure `inlineValue` computation — a variable lookup for each local
//! declared in the visible range that is still in scope where the debugger is
//! paused, under the name the emitter gives it (`bynk_ide::locals_nav::
//! inline_value_sites` picks them; this lowers the spans to LSP positions
//! against the analysed snapshot, the v0.24 rule).

use bynk_check::locals::LocalBinding;
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::*;

use crate::position::{PositionMap, position_to_offset};

/// The inline-value lookups for `visible`, with the debugger paused at
/// `stopped`. A position outside the snapshot yields nothing.
pub fn inline_values(
    text: &str,
    locals: &[LocalBinding],
    visible: Range,
    stopped: Position,
) -> Vec<InlineValue> {
    let (Some(start), Some(end), Some(stop)) = (
        position_to_offset(text, visible.start),
        position_to_offset(text, visible.end),
        position_to_offset(text, stopped),
    ) else {
        return Vec::new();
    };
    let positions = PositionMap::new(text);
    crate::locals_nav::inline_value_sites(locals, Span::new(start, end), Span::new(stop, stop))
        .into_iter()
        .map(|site| {
            InlineValue::VariableLookup(InlineValueVariableLookup {
                range: Range::new(
                    positions.position(site.range.start),
                    positions.position(site.range.end),
                ),
                variable_name: Some(site.variable),
                case_sensitive_lookup: true,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_check::locals::LocalKind;

    // Line 0: `fn f(class: Int) -> Int {`, line 1: `  let y = class`, line 2: `  y`.
    const TEXT: &str = "fn f(class: Int) -> Int {\n  let y = class\n  y\n}\n";

    fn locals() -> Vec<LocalBinding> {
        let span = |needle: &str, nth: usize| {
            let s = TEXT.match_indices(needle).nth(nth).unwrap().0;
            Span::new(s, s + needle.len())
        };
        vec![
            LocalBinding {
                name: "class".into(),
                def_span: span("class", 0),
                kind: LocalKind::Param,
                ty: "Int".into(),
                scope: Span::new(24, TEXT.len()),
            },
            LocalBinding {
                name: "y".into(),
                def_span: span("y", 0),
                kind: LocalKind::Let,
                ty: "Int".into(),
                scope: Span::new(TEXT.find("  y\n").unwrap(), TEXT.len()),
            },
        ]
    }

    fn lookups(values: Vec<InlineValue>) -> Vec<(Range, String)> {
        values
            .into_iter()
            .map(|v| match v {
                InlineValue::VariableLookup(l) => (l.range, l.variable_name.unwrap()),
                other => panic!("expected a variable lookup, got {other:?}"),
            })
            .collect()
    }

    fn whole() -> Range {
        Range::new(Position::new(0, 0), Position::new(4, 0))
    }

    #[test]
    fn lookups_carry_the_emitted_name_and_the_declaration_range() {
        let got = lookups(inline_values(TEXT, &locals(), whole(), Position::new(2, 2)));
        assert_eq!(
            got,
            vec![
                (
                    Range::new(Position::new(0, 5), Position::new(0, 10)),
                    "__id_class".to_string()
                ),
                (
                    Range::new(Position::new(1, 6), Position::new(1, 7)),
                    "y".to_string()
                ),
            ]
        );
    }

    #[test]
    fn only_the_visible_range_and_live_bindings_are_returned() {
        // Visible: line 1 only.
        let visible = Range::new(Position::new(1, 0), Position::new(2, 0));
        let got = lookups(inline_values(TEXT, &locals(), visible, Position::new(2, 2)));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, "y");
        // Paused on line 1: `y` is not yet bound.
        let got = lookups(inline_values(TEXT, &locals(), whole(), Position::new(1, 2)));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].1, "__id_class");
    }

    #[test]
    fn a_position_outside_the_snapshot_yields_nothing() {
        let off = Position::new(99, 0);
        assert!(inline_values(TEXT, &locals(), whole(), off).is_empty());
    }
}
