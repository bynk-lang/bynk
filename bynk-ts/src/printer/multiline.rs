use super::compact::render_compact_stmts;
use super::decl::render_params;
use super::expr::{render_array_item, render_expr};
use super::fragment::contains_raw;
use super::stmt::{render_block_stmts, render_doc_comment};
use super::types::render_type;
use super::{MergeTarget, indent, note_span_at};
use crate::program::{TsExpr, TsObjectEntry};
use bynk_syntax::span::Span;

/// Render `expr` as a statement or declaration's own top-level expression
/// (a `const`/`let` initialiser, an assignment's own value, …) — one of the
/// two places `depth` is available to render a `multiline: true`
/// [`TsExpr::Object`]/[`TsExpr::Array`] correctly (see `Object`'s own doc);
/// the other is [`render_multiline_object_entry`]'s own `Prop` arm (#1355 —
/// a `Prop`'s own value can itself be a nested multiline object/array,
/// `emit_messages_bundle`'s own real doubly-nested table). Every other
/// shape defers to the ordinary, depth-unaware [`render_expr`] unchanged;
/// this exists only to intercept the one shape that needs `depth` before it
/// gets there.
pub(super) fn render_stmt_level_expr(
    out: &mut String,
    expr: &TsExpr,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    if let TsExpr::Object {
        entries,
        multiline: true,
        end_span,
    } = expr
    {
        render_multiline_object(out, entries, *end_span, depth, m);
    } else if let TsExpr::Array {
        items,
        multiline: true,
    } = expr
    {
        render_multiline_array(out, items, depth, m);
    } else if let TsExpr::AsConst(inner) = expr
        && is_multiline_literal(inner)
    {
        render_stmt_level_expr(out, inner, depth, m);
        out.push_str(" as const");
    } else if let TsExpr::As { expr: inner, ty } = expr
        && is_multiline_literal(inner)
    {
        render_stmt_level_expr(out, inner, depth, m);
        out.push_str(" as ");
        render_type(out, ty);
    } else {
        render_expr(out, expr, depth, m);
    }
}

/// Whether `expr` is a `multiline: true` object or array literal.
fn is_multiline_literal(expr: &TsExpr) -> bool {
    matches!(
        expr,
        TsExpr::Object {
            multiline: true,
            ..
        } | TsExpr::Array {
            multiline: true,
            ..
        }
    )
}

/// `{ <newline> ("  "*depth+1)<key>: <value>,<newline> ... ("  "*depth)}` —
/// one entry per line, each with its own trailing comma (including the
/// last), closing brace back at `depth`'s own indent. `events_fanout.rs`'s
/// own `__eventRoutes` table is the real, grounded shape this exists for
/// (#1317) — TypeScript's ordinary multi-line object-literal convention.
/// #1321 (`workers.rs`): entries may now be [`TsObjectEntry`], not just
/// `Prop` — the `compose`-returned surface object is one shorthand async
/// `Method` entry per wrapper, one per line.
pub(super) fn render_multiline_object(
    out: &mut String,
    entries: &[TsObjectEntry],
    end_span: Option<Span>,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    // Deliberately no empty-entries shortcut (review of #1317/#1318, finding
    // 1): `events_fanout.rs`'s own `__eventRoutes` table is reachable with
    // zero entries (a context can `ctx_uses_emit` while publishing only
    // events nobody subscribes to — `own_event_routes` filters down to
    // exactly that empty case, `bynk-emit/src/project.rs`'s own
    // `own_event_routes` computation), and the pre-conversion `writeln!`
    // code always wrote the open-brace line and the closing `};` line
    // unconditionally, regardless of how many times its own `for` loop
    // iterated — an empty table printed `{\n};`, not the tight `{}` a
    // single-line object's own empty case uses. Matching that byte-for-byte
    // means never taking the single-line shortcut here, not even for zero
    // entries.
    out.push_str("{\n");
    for entry in entries {
        render_multiline_object_entry(out, entry, depth, m.as_mut().map(|t| t.reborrow()));
    }
    // 1A.32a: what follows the last entry belongs to the object's declaration, not
    // to that entry's final body line.
    if let Some(span) = end_span {
        note_span_at(out.len(), span, m.as_mut());
    }
    out.push_str(&indent(depth));
    out.push('}');
}

/// One [`TsObjectEntry`], as [`render_multiline_object`]'s own per-entry
/// loop body — factored out so [`print_object_entry`](super::fragment::print_object_entry) can render exactly
/// one entry without a whole object's own opening/closing braces, sharing
/// this one real per-kind dispatch rather than a second copy (the same
/// "one document-fragment entry point, no duplicated rendering" posture
/// [`print_stmt`](super::fragment::print_stmt)/[`print_type`](super::fragment::print_type) already established).
/// `map`: see [`render_block_stmts`]'s own doc (#1477) — reaches a `Method`
/// entry's own body unchanged; every other entry kind ignores it (no body
/// of its own).
pub(super) fn render_multiline_object_entry(
    out: &mut String,
    entry: &TsObjectEntry,
    depth: usize,
    mut map: Option<MergeTarget<'_>>,
) {
    // Review of #1340, finding 1: a `Raw`-bodied `Method` entry's own text
    // is captured pre-indented at a fixed absolute depth by its caller (see
    // `print_object_entry`'s own doc) — correct only when `depth` is `0`.
    // Originally guarded only in `print_object_entry`, the one entry point
    // #1337 had in mind; #1339's `emit_refined_type` reaches this function
    // directly (via `TsExpr::multiline_object_entries`/`render_expr`, never
    // through `print_object_entry`), which the original guard's placement
    // silently missed. Asserted here instead, where both callers converge,
    // so neither path can bypass it again.
    if let TsObjectEntry::Method { body, .. } = entry {
        debug_assert!(
            depth == 0 || !contains_raw(body),
            "render_multiline_object_entry: a Raw-bodied Method entry's own baked-in indent only matches depth 0"
        );
    }
    match entry {
        TsObjectEntry::Prop {
            key: k,
            value: v,
            doc,
        } => {
            if let Some(text) = doc {
                render_doc_comment(out, text, depth + 1);
            }
            out.push_str(&indent(depth + 1));
            out.push_str(k);
            out.push_str(": ");
            // #1355: a `Prop`'s own value sits one level deeper than the
            // entry itself (matching this function's own `indent(depth +
            // 1)` above) — the same depth `render_stmt_level_expr`'s own
            // doc names as "the only place `depth` is available to render a
            // `multiline: true` `TsExpr::Object`/`Array` correctly."
            // `emit_messages_bundle`'s own real, doubly-nested `{ locale: {
            // code: expr, ... }, ... }` table is this gap's real grounding
            // — a nested multiline object value previously fell back to
            // single-line silently (the plain, depth-unaware `render_expr`
            // recursion ignores `multiline` entirely).
            render_stmt_level_expr(out, v, depth + 1, &mut map);
            out.push_str(",\n");
        }
        TsObjectEntry::Shorthand(name) => {
            out.push_str(&indent(depth + 1));
            out.push_str(name);
            out.push_str(",\n");
        }
        TsObjectEntry::Spread(e) => {
            out.push_str(&indent(depth + 1));
            out.push_str("...");
            render_expr(out, e, depth + 1, &mut map);
            out.push_str(",\n");
        }
        // `workers.rs`'s own dominant real shape (Decision A, gap 1):
        // `async {name}({params}) { <body> },`, body indented one
        // level deeper still — the same shape a class method's own
        // body gets (`render_decl_body`'s `TsDecl::Class` arm), here
        // for one object-literal entry instead.
        TsObjectEntry::Method {
            name,
            is_async,
            generics,
            params,
            return_type,
            doc,
            inline,
            body,
            span,
        } => {
            if let Some(span) = span {
                note_span_at(out.len(), *span, map.as_mut());
            }
            if let Some(text) = doc {
                render_doc_comment(out, text, depth + 1);
            }
            out.push_str(&indent(depth + 1));
            if *is_async {
                out.push_str("async ");
            }
            out.push_str(name);
            render_bare_generics(out, generics);
            out.push('(');
            render_params(out, params);
            out.push(')');
            if let Some(rt) = return_type {
                out.push_str(": ");
                render_type(out, rt);
            }
            if *inline {
                out.push_str(" { ");
                render_compact_stmts(out, body, depth);
                out.push_str(" }");
            } else {
                render_block_stmts(out, body, depth + 1, map);
            }
            out.push_str(",\n");
        }
    }
}

/// `<{names.join(", ")}>`, or nothing at all when `names` is empty — bare
/// names only, matching `bynk-emit`'s own `ts_type_params` rendering
/// exactly. Originally `TsObjectEntry::Method`'s own generic parameter list
/// (#1337); generalised by #1339 into the one shared renderer for every
/// bare-name generics/type-params list this crate builds
/// (`TsDecl::Interface.type_params`, `TsDecl::TypeAlias.type_params`,
/// `TsExpr::Arrow.generics`) rather than four near-identical copies.
pub(super) fn render_bare_generics(out: &mut String, names: &[String]) {
    if names.is_empty() {
        return;
    }
    out.push('<');
    for (i, g) in names.iter().enumerate() {
        if i > 0 {
            out.push_str(", ");
        }
        out.push_str(g);
    }
    out.push('>');
}

/// `[ <newline> ("  "*depth+1)<item>,<newline> ... ("  "*depth)]` — one
/// item per line, each with its own trailing comma, closing bracket back at
/// `depth`'s own indent. [`render_multiline_object`]'s own sibling for array
/// literals — #1325's own real, grounded shape: `emit_test_main`'s own
/// `modules` array (one `{ name, run }` entry per test). Same "no
/// empty-items shortcut" discipline as `render_multiline_object` — nothing
/// in `emit_test_main`'s own real content reaches this with zero tests, but
/// matching the pre-conversion `writeln!` code's own unconditional
/// open/close-bracket-on-separate-lines shape (rather than assuming an
/// untested empty case) is the same precedent review of #1317/#1318 set.
pub(super) fn render_multiline_array(
    out: &mut String,
    items: &[TsExpr],
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    out.push_str("[\n");
    for item in items {
        out.push_str(&indent(depth + 1));
        render_array_item(out, item, depth + 1, m);
        out.push_str(",\n");
    }
    out.push_str(&indent(depth));
    out.push(']');
}

/// [`TsExpr::MultilineConditional`]: the test on the current line, then
/// `? consequent` and `: alternate` on lines of their own, one indent level (two
/// spaces) deeper than `depth`. The test, the consequent and the alternate take
/// the same parenthesisation as [`render_conditional`](super::expr::render_conditional)'s.
pub(super) fn render_multiline_conditional(
    out: &mut String,
    test: &TsExpr,
    consequent: &TsExpr,
    alternate: &TsExpr,
    depth: usize,
    m: &mut Option<MergeTarget<'_>>,
) {
    let test_needs_parens = matches!(
        test,
        TsExpr::Arrow { .. }
            | TsExpr::Conditional { .. }
            | TsExpr::MultilineConditional { .. }
            | TsExpr::Yield(_)
    );
    if test_needs_parens {
        out.push('(');
        render_expr(out, test, depth, m);
        out.push(')');
    } else {
        render_expr(out, test, depth, m);
    }
    let indent = "  ".repeat(depth + 1);
    out.push('\n');
    out.push_str(&indent);
    out.push_str("? ");
    render_expr(out, consequent, depth + 1, m);
    out.push('\n');
    out.push_str(&indent);
    out.push_str(": ");
    render_expr(out, alternate, depth + 1, m);
}
