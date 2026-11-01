//! v0.37 (ADR 0070): structural ranges — `textDocument/foldingRange` and
//! `textDocument/selectionRange`.
//!
//! Both read the per-file **recovered AST** (the document-symbols parse path)
//! and share one span visitor ([`collect`]): every node contributes its
//! `(span, foldable)` pair. **Folding** keeps the multi-line block-like nodes
//! (`foldable`); **selection** keeps every span containing the cursor and
//! nests them. Neither touches the binding index or the analysis round — they
//! parse the live document, so they answer even when the project doesn't check.

use std::collections::HashSet;

use bynk_syntax::ast::*;
use bynk_syntax::lexer::{TokenKind, tokenize};
use bynk_syntax::parser::parse_unit_with_recovery;
use bynk_syntax::span::Span;
use tower_lsp::lsp_types::{FoldingRange, FoldingRangeKind, Position, Range, SelectionRange};

use crate::position::{PositionMap, position_to_offset};

/// Every AST node's span paired with whether it is a folding candidate (a
/// multi-line block-like construct). Non-candidate spans are still collected —
/// selection chains need the fine-grained leaves. Empty when the file has no
/// recognisable header (recovery returned nothing).
fn collect(source: &str) -> Vec<(Span, bool)> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    let (Some(unit), _errs) = parse_unit_with_recovery(&tokens, source) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    walk_unit(&unit, &mut out);
    out
}

fn walk_unit(unit: &SourceUnit, out: &mut Vec<(Span, bool)>) {
    match unit {
        SourceUnit::Commons(c) => {
            out.push((c.span, true));
            c.items.iter().for_each(|i| walk_item(i, out));
        }
        SourceUnit::Context(c) => {
            out.push((c.span, true));
            c.items.iter().for_each(|i| walk_item(i, out));
        }
        SourceUnit::Adapter(a) => {
            out.push((a.span, true));
            a.items.iter().for_each(|i| walk_item(i, out));
        }
        SourceUnit::Suite(t) => {
            out.push((t.span, true));
            for case in &t.cases {
                out.push((case.span, true));
                walk_block(&case.body, out);
            }
        }
    }
}

fn walk_item(item: &CommonsItem, out: &mut Vec<(Span, bool)>) {
    match item {
        CommonsItem::Type(t) => {
            out.push((t.span, true));
            match &t.body {
                TypeBody::Record(r) => out.push((r.span, true)),
                TypeBody::Sum(s) => out.push((s.span, true)),
                TypeBody::Opaque { .. } | TypeBody::Refined { .. } => {}
            }
        }
        CommonsItem::Fn(f) => {
            out.push((f.span, true));
            walk_block(&f.body, out);
        }
        CommonsItem::Capability(c) => out.push((c.span, true)),
        CommonsItem::Provider(p) => {
            out.push((p.span, true));
            for op in &p.ops {
                out.push((op.span, true));
                walk_block(&op.body, out);
            }
        }
        CommonsItem::Service(s) => {
            out.push((s.span, true));
            for h in &s.handlers {
                out.push((h.span, true));
                walk_block(&h.body, out);
            }
        }
        CommonsItem::Agent(a) => {
            out.push((a.span, true));
            for h in &a.handlers {
                out.push((h.span, true));
                walk_block(&h.body, out);
            }
        }
        CommonsItem::Actor(a) => {
            out.push((a.span, true));
        }
        CommonsItem::Messages(m) => {
            out.push((m.span, true));
        }
        // Events track, slice 0 (spine #936): an `event` folds exactly like
        // a `type` with a record body — it always has one (no sum/refined/
        // opaque event forms in slice 0).
        CommonsItem::Event(e) => {
            out.push((e.span, true));
            out.push((e.body.span, true));
        }
        // #1883, slice 1a: a one-line declaration, nothing to fold.
        CommonsItem::Asset(_) => {}
        // #2387, slice 1: likewise.
        CommonsItem::Embed(_) => {}
        // #2617: a declaration, usually on one line; nothing to fold.
        CommonsItem::Const(_) => {}
    }
}

fn walk_block(b: &Block, out: &mut Vec<(Span, bool)>) {
    out.push((b.span, true));
    walk_statements(b, out);
}

/// A block's statements (each a selection level, then its expressions,
/// principal identity included) and its tail.
fn walk_statements(b: &Block, out: &mut Vec<(Span, bool)>) {
    for s in &b.statements {
        out.push((s.span(), false));
        let mut exprs = Vec::new();
        statement_exprs(s, &mut exprs);
        for e in exprs {
            walk_expr(e, out);
        }
    }
    walk_expr(&b.tail, out);
}

/// Every expression is a selection level; the multi-line constructs also fold.
/// `Block`, `If` and `Match` are walked by hand because they add spans of
/// their own (statements, branch blocks, arms). Every other kind recurses
/// through [`expr_children`], so no child slot is skipped (#1850: match-arm
/// guards and observation predicates once were).
fn walk_expr(e: &Expr, out: &mut Vec<(Span, bool)>) {
    let foldable = matches!(
        e.kind,
        ExprKind::Block(_)
            | ExprKind::If { .. }
            | ExprKind::Match { .. }
            | ExprKind::RecordConstruction { .. }
            | ExprKind::RecordSpread { .. }
            | ExprKind::ListLit(_)
            | ExprKind::Lambda(_)
    );
    out.push((e.span, foldable));
    match &e.kind {
        ExprKind::Block(b) => walk_statements(b, out),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            walk_expr(cond, out);
            walk_block(then_block, out);
            walk_block(else_block, out);
        }
        ExprKind::Match { discriminant, arms } => {
            walk_expr(discriminant, out);
            for arm in arms {
                out.push((arm.span, true));
                if let Some(guard) = &arm.guard {
                    walk_expr(guard, out);
                }
                match &arm.body {
                    MatchBody::Expr(ex) => walk_expr(ex, out),
                    MatchBody::Block(bl) => walk_block(bl, out),
                }
            }
        }
        _ => expr_children(e).into_iter().for_each(|c| walk_expr(c, out)),
    }
}

/// `textDocument/foldingRange` — the structural multi-line constructs plus
/// multi-line comment runs. A range is emitted only when it spans more than
/// one line (LSP folds ≥2 lines); duplicate `(start, end)` line pairs (a decl
/// and its body sharing both lines) collapse to one.
pub fn folding_ranges(source: &str) -> Vec<FoldingRange> {
    let positions = PositionMap::new(source);
    let mut out = Vec::new();
    let mut seen: HashSet<(u32, u32)> = HashSet::new();
    for (span, foldable) in collect(source) {
        if !foldable {
            continue;
        }
        let start = positions.position(span.start).line;
        let end = positions.position(span.end).line;
        if end > start && seen.insert((start, end)) {
            out.push(fold(start, end, None));
        }
    }
    out.extend(comment_folds(source, &positions));
    out
}

/// Multi-line runs of consecutive `--` line comments → `Comment` folds. Spans
/// come from the lexer's `Comment` tokens (the trivia table keeps only bodies),
/// grouped while each comment sits on the line immediately after the previous.
fn comment_folds(source: &str, positions: &PositionMap) -> Vec<FoldingRange> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    // #1888: a run of `--|` doc lines folds the same way, as its own run (a
    // doc line's span runs through its newline, so it is trimmed to the line).
    let comments: Vec<(TokenKind, Span)> = tokens
        .iter()
        .filter(|t| matches!(t.kind, TokenKind::Comment | TokenKind::DocLine))
        .map(|t| {
            let line = source[t.span.range()].trim_end_matches(['\n', '\r']);
            (t.kind, Span::new(t.span.start, t.span.start + line.len()))
        })
        .collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < comments.len() {
        let (kind, first) = comments[i];
        let start = positions.position(first.start).line;
        let mut end = positions.position(first.end).line;
        let mut j = i;
        while j + 1 < comments.len() && comments[j + 1].0 == kind {
            let next = positions.position(comments[j + 1].1.start).line;
            if next == end + 1 {
                j += 1;
                end = positions.position(comments[j].1.end).line;
            } else {
                break;
            }
        }
        if end > start {
            out.push(fold(start, end, Some(FoldingRangeKind::Comment)));
        }
        i = j + 1;
    }
    out
}

fn fold(start_line: u32, end_line: u32, kind: Option<FoldingRangeKind>) -> FoldingRange {
    FoldingRange {
        start_line,
        start_character: None,
        end_line,
        end_character: None,
        kind,
        collapsed_text: None,
    }
}

/// `textDocument/selectionRange` — for each position, the chain of enclosing
/// AST node ranges, innermost first (each `.parent` widens outward to the
/// file). Falls back to an empty range at the cursor when no node contains it
/// (e.g. trailing whitespace) or the file doesn't parse.
pub fn selection_ranges(source: &str, positions: &[Position]) -> Vec<SelectionRange> {
    let nodes = collect(source);
    let map = PositionMap::new(source);
    positions
        .iter()
        .map(|pos| selection_at(source, &map, &nodes, *pos))
        .collect()
}

fn selection_at(
    source: &str,
    map: &PositionMap,
    nodes: &[(Span, bool)],
    pos: Position,
) -> SelectionRange {
    let empty = SelectionRange {
        range: Range::new(pos, pos),
        parent: None,
    };
    let Some(offset) = position_to_offset(source, pos) else {
        return empty;
    };
    // Spans containing the offset, de-duplicated, smallest first.
    let mut spans: Vec<Span> = nodes
        .iter()
        .map(|(s, _)| *s)
        .filter(|s| s.start <= offset && offset <= s.end)
        .collect();
    spans.sort_by_key(|s| (s.start, s.end));
    spans.dedup();
    spans.sort_by_key(|s| s.end - s.start);
    // Build outermost → innermost so each node's `parent` is the next-larger.
    let mut chain: Option<Box<SelectionRange>> = None;
    for span in spans.into_iter().rev() {
        chain = Some(Box::new(SelectionRange {
            range: map.range(span),
            parent: chain,
        }));
    }
    chain.map(|b| *b).unwrap_or(empty)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::position::offset_to_position;

    const SRC: &str = concat!(
        "context shop\n",
        "\n",
        "// a comment\n",
        "// second line\n",
        "\n",
        "type Money = {\n",
        "  cents: Int,\n",
        "  currency: String,\n",
        "}\n",
        "\n",
        "fn total(m: Money) -> Int {\n",
        "  if m.cents > 0 {\n",
        "    m.cents\n",
        "  } else {\n",
        "    0\n",
        "  }\n",
        "}\n",
    );

    /// The 0-based line a substring first appears on.
    fn line_of(needle: &str) -> u32 {
        let off = SRC.find(needle).expect("substring present");
        offset_to_position(SRC, off).line
    }

    /// #1888: a run of `--|` doc lines folds as a comment, apart from a `--`
    /// run beside it, and ends on its own last line.
    #[test]
    fn a_doc_line_run_folds_apart_from_comments() {
        let src = "commons m\n\n// one\n// two\n--- Doc one.\n--- Doc two.\ntype T = Int\n";
        let folds: Vec<(u32, u32)> = folding_ranges(src)
            .iter()
            .filter(|f| f.kind == Some(FoldingRangeKind::Comment))
            .map(|f| (f.start_line, f.end_line))
            .collect();
        assert_eq!(folds, [(2, 3), (4, 5)]);
    }

    #[test]
    fn folds_structural_constructs_and_comment_runs_omitting_single_lines() {
        let folds = folding_ranges(SRC);

        // Every fold spans more than one line.
        assert!(folds.iter().all(|f| f.end_line > f.start_line));

        // The two-line comment run folds as a Comment.
        let comment = folds
            .iter()
            .find(|f| f.kind == Some(FoldingRangeKind::Comment))
            .expect("comment run folds");
        assert_eq!((comment.start_line, comment.end_line), (2, 3));

        // The record body folds (start at the `type` line, end at its `}`).
        assert!(
            folds
                .iter()
                .any(|f| f.start_line == line_of("type Money") && f.end_line == line_of("}\n\nfn")),
            "record body folds"
        );

        // The `if` folds (a structural Region); the single-line then/else tail
        // expression (`m.cents` alone, line 12) does not start a fold.
        assert!(
            folds
                .iter()
                .any(|f| f.start_line == line_of("if m.cents") && f.kind.is_none()),
            "if folds"
        );
        let tail_line = line_of("    m.cents"); // the indented tail, line 12
        assert!(
            !folds.iter().any(|f| f.start_line == tail_line),
            "single-line tail expression is not folded"
        );
    }

    #[test]
    fn selection_chain_widens_from_the_cursor_to_the_file() {
        // Cursor on `cents` of the then-block tail `m.cents` (line 12).
        let off = SRC.find("    m.cents").unwrap() + 6; // onto `cents`
        let pos = offset_to_position(SRC, off);
        let ranges = selection_ranges(SRC, &[pos]);
        assert_eq!(ranges.len(), 1);

        // Walk the parent chain; ranges must strictly widen and stay nested.
        let mut levels = 0;
        let mut cur = Some(&ranges[0]);
        let mut prev: Option<&Range> = None;
        let mut outermost = ranges[0].range;
        while let Some(node) = cur {
            if let Some(p) = prev {
                // Each parent contains the previous (child) range.
                assert!(node.range.start <= p.start && node.range.end >= p.end);
                assert!(node.range != *p, "ranges strictly widen");
            }
            outermost = node.range;
            prev = Some(&node.range);
            levels += 1;
            cur = node.parent.as_deref();
        }
        assert!(levels >= 4, "cursor → … → context is several levels");
        // Outermost is the whole context (starts on line 0).
        assert_eq!(outermost.start.line, 0);
    }

    #[test]
    fn partial_parse_still_folds_what_parsed() {
        // A malformed trailing item must not panic and must still fold the
        // valid context + type above it.
        let src = "context shop\n\ntype Money = {\n  cents: Int,\n}\n\nfn broken(";
        let folds = folding_ranges(src);
        assert!(
            folds.iter().any(|f| f.start_line == 2),
            "the type still folds"
        );
        // Selection at the top of the file is well-formed too.
        let sel = selection_ranges(src, &[Position::new(3, 4)]);
        assert_eq!(sel.len(), 1);
    }

    #[test]
    fn selection_inside_a_match_guard_includes_the_guard() {
        let src = "commons d\n\nfn f(n: Int, lim: Int) -> Int {\n  match n {\n    k if k + 1 > lim => 1\n    _ => 0\n  }\n}\n";
        let guard = "k + 1 > lim";
        let start = src.find(guard).unwrap();
        let (gs, ge) = (
            offset_to_position(src, start),
            offset_to_position(src, start + guard.len()),
        );
        let ranges = selection_ranges(src, &[offset_to_position(src, start + 4)]);
        let mut cur = Some(&ranges[0]);
        let mut found = false;
        while let Some(node) = cur {
            found |= node.range.start == gs && node.range.end == ge;
            cur = node.parent.as_deref();
        }
        assert!(found, "the guard `{guard}` is a selection level");
    }
}
