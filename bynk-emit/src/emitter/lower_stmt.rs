//! Statement emission — the half of lowering whose contract is "append
//! statements to `out` at `indent`": block and function bodies, statements,
//! tail positions, hoisting, `par` and statement-position `match`. Split out of
//! `lower.rs` (back-half paydown 1A.9, S0 of the statement-emitter design note
//! #3708); expression lowering stays there and is reached via `super::lower`.

use std::sync::Arc;

use bynk_check::checker::{NamedKind, Ty, TyId};
use bynk_syntax::ast::{
    BaseType, BinOp, Block, Expr, ExprKind, MatchArm, Pattern, PatternBindingKind, Statement,
    TypeBody,
};
use bynk_syntax::span::Span;

use super::ctx::TailSlot;
use super::lower::{Lowered, LoweredExpr, Pre, member_node, ternary_built};
use super::lower_ctx::LowerCtx;
use super::ts_types::{ts_ty, ts_type_ref, ts_type_ref_to_ts_type};
use super::*;

/// The statement emitter builds a [`Body`] — a list of [`Item`]s — instead of
/// appending text to a caller's buffer; [`render`] is the one adapter that
/// turns it back into text and records the source-map checkpoints (back-half
/// paydown 1A.10, S1 of the statement-emitter design note #3708). The producers
/// below nest with [`Item::Indent`] (always `+ INDENT_STEP`) rather than by
/// threading an `indent` through every call; the entry points keep their
/// `(out, …, indent)` signatures as build-then-render wrappers.
pub(super) struct Body(Vec<Item>);

pub(super) enum Item {
    /// Verbatim text, indented on its first line only: today's [`write_line`].
    /// A multi-line string keeps whatever continuation indentation it carries.
    Line(String),
    /// A typed statement, printed at depth 0 with every line indented.
    // Boxed, like `LoweredStmt::Node`: a `TsStmt` dwarfs the other variants.
    Node(Box<bynk_ts::TsStmt>),
    /// A source-map checkpoint at the offset where the next item starts.
    Mark(Span),
    /// A nested body, rendered at `indent + INDENT_STEP`.
    Indent(Body),
}

impl Body {
    pub(super) fn new() -> Self {
        Body(Vec::new())
    }

    pub(super) fn push(&mut self, item: Item) {
        self.0.push(item);
    }

    pub(super) fn extend(&mut self, items: impl IntoIterator<Item = Item>) {
        self.0.extend(items);
    }

    /// The body as statements, if every item is a [`Item::Node`] or an
    /// [`Item::Mark`] (1A.11, Decision C of #3712); `None` if any item is text
    /// or a nested [`Item::Indent`], or a mark has no node to attach to.
    ///
    /// A `Mark`'s span goes on the **next** node's [`bynk_ts::TsStmt::span`], and
    /// on that node only: a statement's hoisted lines followed by its own give
    /// the span to the first, which is the nearest-enclosing rule the one
    /// checkpoint implements today. Two marks in a row sit at one offset, where
    /// the later replaces the earlier, as `SourceMapBuilder::record` does.
    /// Borrowing rather than consuming lets the caller fall back to the same
    /// items as text when this says no.
    pub(super) fn to_nodes(&self) -> Option<Vec<bynk_ts::TsStmt>> {
        let mut stmts = Vec::with_capacity(self.0.len());
        let mut pending: Option<Span> = None;
        for item in &self.0 {
            match item {
                Item::Mark(span) => pending = Some(*span),
                Item::Node(stmt) => {
                    let mut stmt = (**stmt).clone();
                    stmt.span = stmt.span.or(pending.take());
                    stmts.push(stmt);
                }
                Item::Line(_) | Item::Indent(_) => return None,
            }
        }
        pending.is_none().then_some(stmts)
    }
}

#[cfg(test)]
thread_local! {
    /// Test-only: how many `Item::Line`s have been rendered (1A.11's "converted
    /// to nodes" tests read it; never compiled into the emitter).
    static LINES_RENDERED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Render `body` into `out` at `indent`, recording each [`Item::Mark`] with
/// `cx.record_span` at the offset where it is reached.
pub(super) fn render(body: &Body, out: &mut String, indent: usize, cx: &LowerCtx) {
    for item in &body.0 {
        match item {
            Item::Line(text) => {
                #[cfg(test)]
                LINES_RENDERED.with(|c| c.set(c.get() + 1));
                write_line(out, indent, text);
            }
            Item::Node(stmt) => {
                // A node can carry checkpoints of its own, one per nested
                // statement that has a span (Decision C of #3712). The printer
                // reports each at a depth-0 offset; this adapter then writes
                // `indent` columns before every line, so a checkpoint lands
                // `indent` further on for each line before it, and one more
                // when it is mid-line (an inline branch), after its own line's
                // indentation.
                let (text, spans) = bynk_ts::print_stmt_with_spans(stmt, 0);
                let base = out.len();
                for (offset, span) in spans {
                    let before = &text[..offset];
                    let mid_line = !before.is_empty() && !before.ends_with('\n');
                    let lines = before.matches('\n').count() + usize::from(mid_line);
                    cx.record_span(base + offset + lines * indent, span);
                }
                for line in text.lines() {
                    write_line(out, indent, line);
                }
            }
            Item::Mark(span) => cx.record_span(out.len(), *span),
            Item::Indent(inner) => render(inner, out, indent + INDENT_STEP, cx),
        }
    }
}

pub(crate) fn emit_block_as_function_body(
    out: &mut String,
    block: &Block,
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    // v0.154 (ADR 0178): **preserve** the enclosing `return_ty` rather than
    // resetting it. Nested control-flow blocks (an `if`/`else` or `match` arm
    // *within* a function body) reach here, and a `return` from them returns
    // from the whole function — so an embedding `?` inside them must still see
    // the function's return type. Only a genuine function/handler boundary
    // (`_with_return`) rebinds it. A top-level body with no embedding context
    // (a mock/test that never called `_with_return`) simply inherits the
    // ambient `None`.
    let mut body = Body::new();
    emit_block_inner(&mut body, block, cx, async_tail);
    render(&body, out, indent, cx);
}

/// As [`emit_block_as_function_body`], but records the enclosing return type on
/// the `LowerCtx` for the duration of the body so the `?` lowering can apply a
/// declared error embedding (v0.154, ADR 0178). The return type is set and then
/// restored, so a *sibling* body (a lambda, a mock) does not inherit the wrong
/// one; nested control-flow blocks reached via the shim above keep it.
pub(crate) fn emit_block_as_function_body_with_return(
    out: &mut String,
    block: &Block,
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
    return_type: Option<&TypeRef>,
) {
    let tys = cx.commons().tys();
    let prev = cx.return_ty.take();
    cx.return_ty = return_type
        .and_then(|rt| bynk_check::checker::resolve_type_ref(rt, &cx.commons().types, tys));
    // #1750: a function body's tails `return`, whatever sink encloses it.
    let prev_slot = cx.tail_slot.take();
    let mut body = Body::new();
    emit_block_inner(&mut body, block, cx, async_tail);
    cx.tail_slot = prev_slot;
    cx.return_ty = prev;
    // W30 (#2840): a service handler that claimed settles its claims on exit.
    // The wrapper's text goes around the body as `Line` items, so the body's
    // checkpoints land at the right offsets without being shifted.
    if cx.claim_scope_used && cx.wraps_claims_in_body() {
        body = claim_scoped(body, indent);
    }
    render(&body, out, indent, cx);
}

/// `body` between [`claim_scope_wrapper`]'s `before` and `after` text. The
/// wrapper's text carries its own `indent` padding on every line, and
/// [`Item::Line`] supplies the first line's, so that one is stripped here.
fn claim_scoped(body: Body, indent: usize) -> Body {
    let (before, after) = claim_scope_wrapper(indent);
    let pad = " ".repeat(indent);
    let as_line = |text: &str| {
        let text = text.strip_prefix(pad.as_str()).unwrap_or(text);
        Item::Line(text.strip_suffix('\n').unwrap_or(text).to_string())
    };
    let mut wrapped = Body::new();
    wrapped.push(as_line(&before));
    wrapped.extend(body.0);
    wrapped.push(as_line(&after));
    wrapped
}

fn emit_block_inner(body: &mut Body, block: &Block, cx: &mut LowerCtx, async_tail: bool) {
    // #908: every nested block (if/else branch, match arm, lambda body) as
    // well as a function/handler's own body reaches here, so this is the one
    // place that needs to push/pop a shadow-rename frame to match the
    // language's actual block-scoping — a re-`let` frame must not survive
    // past the block it was declared in.
    cx.shadow_scopes.push(HashMap::new());
    for stmt in &block.statements {
        statement(body, stmt, cx);
    }
    // Tail position: match → inline switch, if → inline if, otherwise return expr.
    // Anchor the tail's generated lines to the tail expression's span (slice 1).
    body.push(Item::Mark(block.tail.span));
    match &block.tail.kind {
        ExprKind::Match { discriminant, arms } => {
            match_tail(body, discriminant, arms, cx, async_tail);
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } if ternary_shaped(cond, then_block, else_block, cx)
            && !cx.tco.as_ref().is_some_and(|p| p.has_call_in(&block.tail)) =>
        {
            // T2.1 (R6.2): the shape *could* be a ternary. Whether it actually
            // is depends on whether either branch hoists — which is now
            // answered by lowering it, not by predicting it. Both outcomes are
            // built from the same `Lowered` values, so nothing is lowered twice
            // and no temp number is burned on a discarded attempt.
            let mut pre = Pre::new();
            let cond_expr = pre.lower_node(cond, cx);
            let then_tail = lower_tail_expr(&then_block.tail, cx, async_tail);
            let else_tail = lower_tail_expr(&else_block.tail, cx, async_tail);
            body.extend(pre.items());
            if then_tail.pre.is_empty() && else_tail.pre.is_empty() {
                // `finish_built` is the one place a `Built` becomes a
                // `LoweredExpr`; its hoists are empty here.
                let ternary = Pre::new()
                    .finish_built(ternary_built(cond_expr, then_tail.expr, else_tail.expr))
                    .expr;
                body.push(tail_item(cx, pre.all_nodes(), ternary));
            } else {
                // A branch hoisted after all — the case `simple_expr` used to
                // mispredict (#945, and the `ListLit`/`InterpStr` fallthrough
                // this slice closes). A statement `if` is where a hoisted `?`'s
                // early return belongs, so emit that instead of a ternary that
                // has nowhere to put it. Byte-identical to what `emit_if_tail`
                // produces for this shape (both branches are statement-free and
                // their tails lower as plain expressions), rebuilt from the
                // values already in hand.
                let then_body = pure_tail_branch(then_tail, cx);
                let else_body = pure_tail_branch(else_tail, cx);
                push_if(body, cond_expr, pre.all_nodes(), then_body, else_body);
            }
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            if_tail(body, cond, then_block, else_block, cx, async_tail);
        }
        // #3193: a self tail call is a `continue`, not a `return`.
        _ if super::tco::emit_continue(body, &block.tail, cx) => {}
        _ => {
            let tail = lower_tail_expr(&block.tail, cx, async_tail);
            body.extend(tail.pre_items());
            let pre_nodes = tail.pre_all_nodes();
            body.push(tail_item(cx, pre_nodes, tail.expr));
        }
    }
    cx.shadow_scopes.pop();
}

/// #1750: the statement that hands a block's tail value to its sink: `return
/// value;` in a function, lambda or arrow body, or, inside a statement-form
/// value expression ([`hoist_value_as_statement`]), the assignment to its slot
/// followed by a `break` out of its labelled block.
///
/// The text form, for the `match` arms that still build text (S4).
fn tail_stmt(cx: &LowerCtx, value: &str) -> String {
    match &cx.tail_slot {
        None => format!("return {value};"),
        Some(TailSlot { slot, label }) => format!("{slot} = {value}; break {label};"),
    }
}

/// A single-line expression, which prints the same wherever it sits. A multi-line
/// one (an arrow with a block body) keeps today's text, whose continuation lines
/// the adapter would otherwise indent where `Line` does not.
fn single_line(e: &bynk_ts::TsExpr) -> bool {
    !bynk_ts::print_expr(e).contains('\n')
}

fn stmt_item(stmt: bynk_ts::TsStmt) -> Item {
    Item::Node(Box::new(stmt))
}

/// [`tail_stmt`] as an item (1A.11): `return <value>;` is a `Return` node when
/// the value and every statement hoisted before it are nodes, otherwise today's
/// text. The [`TailSlot`] form puts two statements on one line, so it stays a
/// `Line` until the slot's labelled block is a node too (S3).
fn tail_item(cx: &LowerCtx, pre_nodes: bool, value: LoweredExpr) -> Item {
    match (&cx.tail_slot, value) {
        (None, LoweredExpr::Node(n)) if pre_nodes && single_line(&n) => {
            stmt_item(bynk_ts::TsStmt::return_stmt(Some(n), None))
        }
        // 1A: the `TailSlot` form and a text value are `Line`s.
        (_, value) => Item::Line(tail_stmt(cx, &value.into_text())),
    }
}

/// `if (cond) { then } else { else }` as items (1A.11): one `If` node when the
/// condition and the statements before it are nodes and both bodies convert
/// ([`Body::to_nodes`], checkpoints carried onto their statements); otherwise
/// today's lines around the same two bodies, which keep their marks.
fn push_if(body: &mut Body, cond: LoweredExpr, pre_nodes: bool, then_body: Body, else_body: Body) {
    if pre_nodes
        && let LoweredExpr::Node(c) = &cond
        && single_line(c)
        && let Some(then_stmts) = then_body.to_nodes()
        && let Some(else_stmts) = else_body.to_nodes()
        && let LoweredExpr::Node(cond) = cond
    {
        body.push(stmt_item(bynk_ts::TsStmt::if_else_same_line_stmt(
            cond,
            bynk_ts::TsStmt::block(then_stmts, None),
            bynk_ts::TsStmt::block(else_stmts, None),
            None,
        )));
        return;
    }
    // 1A: the text form, for a condition, body or hoisted statement that is
    // still text, or an `is`-binding line.
    body.push(Item::Line(format!("if ({}) {{", cond.into_text())));
    body.push(Item::Indent(then_body));
    body.push(Item::Line("} else {".to_string()));
    body.push(Item::Indent(else_body));
    body.push(Item::Line("}".to_string()));
}

/// `await <value>;` as an item: an `ExprStmt(Await)` node over a node value, else
/// today's text.
fn do_item(pre_nodes: bool, value: LoweredExpr) -> Item {
    match value {
        LoweredExpr::Node(n) if pre_nodes && single_line(&n) => stmt_item(
            bynk_ts::TsStmt::expr_stmt(bynk_ts::TsExpr::Await(Box::new(n)), None),
        ),
        // 1A: a text value or hoisted statement keeps the text.
        value => Item::Line(format!("await {};", value.into_text())),
    }
}

/// `<target> = <value>;` as an item: an `Assign` node over a node value, else
/// today's text. `target` is a genuine `Ident` or `Member`.
fn assign_item(pre_nodes: bool, target: bynk_ts::TsExpr, value: LoweredExpr) -> Item {
    match value {
        LoweredExpr::Node(n) if pre_nodes && single_line(&n) => {
            stmt_item(bynk_ts::TsStmt::assign(target, n, None))
        }
        // 1A: a text value or hoisted statement keeps the text.
        value => Item::Line(format!(
            "{} = {};",
            bynk_ts::print_expr(&target),
            value.into_text()
        )),
    }
}

/// `const <name>[: <ty>] = [await ]<value>;` as an item (1A.11): a `Const` node
/// when the value and every hoisted statement are nodes, and any annotation
/// prints byte-identically through `ts_type_ref_to_ts_type`; otherwise today's
/// text.
fn const_item(
    pre_nodes: bool,
    name: &str,
    annot: Option<&TypeRef>,
    value: LoweredExpr,
    awaited: bool,
) -> Item {
    let ty = match annot {
        None => Some(None),
        Some(a) => {
            let ty = ts_type_ref_to_ts_type(a, None);
            (bynk_ts::print_type(&ty) == ts_type_ref(a)).then_some(Some(ty))
        }
    };
    match (value, ty) {
        (LoweredExpr::Node(n), Some(ty)) if pre_nodes && single_line(&n) => {
            let init = if awaited {
                bynk_ts::TsExpr::Await(Box::new(n))
            } else {
                n
            };
            stmt_item(bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident(name.to_string()),
                ty,
                init,
                None,
            ))
        }
        // 1A: a text value, a text hoisted statement, or an annotation that
        // does not print the same through the type node.
        (value, _) => {
            let value = value.into_text();
            let await_ = if awaited { "await " } else { "" };
            Item::Line(match annot {
                Some(a) => format!("const {name}: {ty} = {await_}{value};", ty = ts_type_ref(a)),
                None => format!("const {name} = {await_}{value};"),
            })
        }
    }
}

/// One branch of the statement `if` above: the tail's hoisted statements
/// followed by its `return` (or, #1750, its sink's [`tail_stmt`]). Exactly the
/// body [`emit_block_inner`]'s own `_` arm builds for a statement-free block,
/// which is what `if_tail` would have recursed into.
fn pure_tail_branch(tail: Lowered, cx: &LowerCtx) -> Body {
    let mut body = Body::new();
    body.extend(tail.pre_items());
    let pre_nodes = tail.pre_all_nodes();
    body.push(tail_item(cx, pre_nodes, tail.expr));
    body
}

/// Lower an expression that's in the tail position of a returning context.
///
/// In async-tail position (v0.7.1), an `async function` wraps its return value
/// as a Promise automatically, so `Effect.pure(...)` is redundant and should
/// emit as a bare value. Recurse through control-flow forms whose result is
/// the surrounding function's return value:
/// - `Effect.pure(x)` → lower `x` directly.
/// - A ternary-form `if`/`else` (simple branches) where each branch's tail is
///   itself an async-tail position.
/// - A pure-tail block (no statements) where the inner tail is the actual
///   returned expression.
/// - Parens (transparent).
///
/// In non-async-tail position, defer to [`lower_expr`] unchanged.
fn lower_tail_expr(e: &Expr, cx: &mut LowerCtx, async_tail: bool) -> Lowered {
    if !async_tail {
        return lower_expr(e, cx);
    }
    match &e.kind {
        ExprKind::EffectPure(inner) => lower_expr(inner, cx),
        ExprKind::Paren(inner) => lower_tail_expr(inner, cx, true),
        ExprKind::Block(b) if b.statements.is_empty() => lower_tail_expr(&b.tail, cx, true),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } if ternary_shaped(cond, then_block, else_block, cx) => {
            let mut pre = Pre::new();
            let cond_expr = pre.lower(cond, cx);
            let then_tail = lower_tail_expr(&then_block.tail, cx, true);
            let else_tail = lower_tail_expr(&else_block.tail, cx, true);
            if then_tail.pre.is_empty() && else_tail.pre.is_empty() {
                return pre.finish_text(format!(
                    "({cond_expr} ? {t} : {e})",
                    t = then_tail.expr.into_text(),
                    e = else_tail.expr.into_text()
                ));
            }
            let slot_ty = async_tail_slot_ty(e, cx);
            let value =
                hoist_if_as_statement(&mut pre, cond_expr, then_tail, else_tail, slot_ty, cx);
            pre.finish_text(value)
        }
        _ => lower_expr(e, cx),
    }
}

/// The slot annotation for a hoisted `if` in **async-tail** position, where
/// `checked_ty_ts` alone is wrong.
///
/// #1029 review: this function exists because an `async function` wraps its
/// return value, so `Effect.pure(x)` in tail position emits a bare `x` rather
/// than a promise. A branch therefore assigns `T` where the `if`'s recorded type
/// is `Effect[T]` — and `ts_ty` would render that `Promise<T>`, which `tsc
/// --strict` rejects (`Type 'boolean' is not assignable to type
/// 'Promise<boolean>'`). The ternary form never had to care: `return (c ? x : p)`
/// in an `async function` legally accepts either.
///
/// Both shapes are genuinely reachable — an `Effect.pure(x)` branch yields `T`
/// (the `EffectPure` arm above unwraps it) while a branch that is merely
/// `Effect`-typed falls through to `lower_expr` and yields `Promise<T>` — so the
/// annotation admits both rather than picking one.
fn async_tail_slot_ty(e: &Expr, cx: &LowerCtx) -> Option<String> {
    let tys = cx.commons().tys();
    match cx.commons().expr_ty(e.id).as_deref() {
        Some(Ty::Effect(inner)) => {
            let inner_ts = match &*tys.get(*inner) {
                Ty::Unit => "void".to_string(),
                _ => ts_ty(*inner, tys),
            };
            Some(format!("{inner_ts} | globalThis.Promise<{inner_ts}>"))
        }
        _ => checked_ty_ts(e, cx),
    }
}

/// T2.1: a branch of a would-be ternary hoisted, so the ternary has nowhere to
/// put its statements. Hoist the whole `if` as a real statement assigning to a
/// fresh slot, and hand back the slot as the expression.
///
/// The reason this matters is the one R6.6 names: an arrow wrapper
/// (`(() => { … })()`, the only shape available before `pre` could travel with
/// the expression) makes a hoisted `?`'s `return` exit the *arrow* rather than
/// the enclosing function — the `lower_match_as_iife` miscompile (#946) in a
/// different costume, which `tsc --strict` rejects outright because the arrow's
/// type becomes `T | { tag: "Err" }`. Hoisting keeps the `return` where the
/// source put it.
///
/// Decision B of #1017 asked whether threading closes this for free. For the
/// `if`-expression it does, and this is where. It does **not** close
/// `lower_bin_op`'s short-circuit `rhs`, whose hoist must be *skipped* when the
/// operator doesn't reach it — a statement cannot be conditionally skipped, so
/// the arrow there is load-bearing rather than incidental. That one stays open.
pub(super) fn hoist_if_as_statement(
    pre: &mut Pre,
    cond_expr: String,
    then_tail: Lowered,
    else_tail: Lowered,
    slot_ty: Option<String>,
    cx: &mut LowerCtx,
) -> String {
    let slot = cx.fresh();
    // The slot needs its type spelled out. TypeScript's "evolving `let`"
    // inference gives up once a branch also `return`s (TS7034/TS7005 under
    // `--strict`), which is exactly the shape a hoisted `?` produces — so take
    // the type the checker already recorded for the `if` expression.
    match &slot_ty {
        Some(ty) => pre.push_text(format!("let {slot}: {ty};")),
        None => pre.push_text(format!("let {slot};")),
    }
    pre.push_text(format!("if ({cond_expr}) {{"));
    pre.extend_text(assign_branch(then_tail, &slot));
    pre.push_text("} else {".to_string());
    pre.extend_text(assign_branch(else_tail, &slot));
    pre.push_text("}".to_string());
    slot
}

/// #1750: emit a value-position `if`, `match` or block (`e`) as a real
/// statement, for when its branches hold a `?`. The arrow (IIFE) these forms
/// otherwise lower to would capture the `?`'s `return`, so its `Err` became the
/// expression's value instead of leaving the function: the same miscompile
/// [`hoist_if_as_statement`] closes for a ternary-shaped `if`, which has only
/// branch tails to place. Here `emit` writes the form through its ordinary
/// statement-position emitter (`emit_if_tail`, `emit_match_tail`,
/// `emit_block_as_function_body`) with [`LowerCtx::tail_slot`] set, so every
/// tail, however deeply nested, assigns the slot and breaks out of the
/// labelled block:
///
/// ```text
/// let __r3: T;
/// __r3_out: {
///   if (cond) { …; __r3 = a; break __r3_out; } else { __r3 = b; break __r3_out; }
/// }
/// ```
///
/// The label is what makes the `break` reach past a `switch` (whose cases
/// would otherwise fall through to its trailing `throw`) and past nested
/// statement forms. `return_ty` is deliberately left set: the `?` now returns
/// from the enclosing function, so ADR 0178's embedding conversion applies
/// (#1750 corrects that ADR's "an IIFE clears it", which described the
/// miscompile). An `await` inside lands in the enclosing function's own body,
/// which is `async` whenever its body can `await`.
pub(super) fn hoist_value_as_statement(
    e: &Expr,
    cx: &mut LowerCtx,
    emit: impl FnOnce(&mut String, &mut LowerCtx),
) -> Lowered {
    let slot = cx.fresh();
    let label = slot.clone() + "_out";
    let mut pre = Pre::new();
    // Typed for the same reason as `hoist_if_as_statement`'s slot: inference
    // gives up on a `let` whose assignments sit beside a `return`.
    let annotation = checked_ty_ts(e, cx)
        .map(|ty| ": ".to_string() + &ty)
        .unwrap_or_default();
    pre.push_text(format!("let {slot}{annotation};"));
    pre.push_text(label.clone() + ": {");
    let saved = cx.tail_slot.replace(TailSlot {
        slot: slot.clone(),
        label,
    });
    // The body is a local buffer spliced into `pre` — see `without_source_map`.
    let mut body = String::new();
    cx.without_source_map(|cx| emit(&mut body, cx));
    cx.tail_slot = saved;
    pre.extend_text(body.lines().map(str::to_string));
    pre.push_text("}".to_string());
    pre.finish_text(slot)
}

/// #1750: whether a `?` in `b` would lower to a `return` inside `b`'s own
/// emitted body. Structural, like [`ternary_shaped`], so the route is chosen
/// before anything is lowered. A lambda is its own return scope, so a `?`
/// inside one doesn't count.
pub(super) fn block_contains_question(b: &Block) -> bool {
    let mut stmt_exprs = Vec::new();
    for s in &b.statements {
        bynk_syntax::ast::statement_exprs(s, &mut stmt_exprs);
    }
    stmt_exprs.into_iter().any(expr_contains_question) || expr_contains_question(&b.tail)
}

/// [`block_contains_question`] over a `match`'s arms, guards included: a guard
/// is evaluated inside the arrow too.
pub(super) fn arms_contain_question(arms: &[MatchArm]) -> bool {
    arms.iter().any(|a| {
        a.guard.as_ref().is_some_and(expr_contains_question)
            || match &a.body {
                MatchBody::Expr(e) => expr_contains_question(e),
                MatchBody::Block(b) => block_contains_question(b),
            }
    })
}

fn expr_contains_question(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Question(_) => true,
        ExprKind::Lambda(_) => false,
        _ => bynk_syntax::ast::expr_children(e)
            .into_iter()
            .any(expr_contains_question),
    }
}

/// One branch of the hoisted statement `if` above: the branch's own hoisted
/// statements, then the assignment of its value to the shared slot. Each line
/// carries its own indent — a hoisted statement is written by whichever caller
/// owns the statement position, at one flat indent, so nesting has to travel
/// inside the string.
fn assign_branch(tail: Lowered, slot: &str) -> Vec<String> {
    let pad = " ".repeat(INDENT_STEP);
    let mut out: Vec<String> = tail
        .pre_texts()
        .iter()
        .map(|s| format!("{pad}{s}"))
        .collect();
    out.push(format!("{pad}{slot} = {};", tail.expr.into_text()));
    out
}

/// T2.1 (R6.2): the ternary form's **structural** precondition — everything
/// about the shape that must hold before any lowering is attempted. Both
/// branches must be pure tails (a ternary has nowhere to host a statement)
/// whose own tails lower as plain expressions, and the condition must introduce
/// no `is` bindings (likewise nowhere to declare them).
///
/// The question this deliberately does *not* ask is the one `simple_expr` used
/// to guess: "will lowering this branch hoist a statement?" That is now
/// answered after the fact by [`Lowered::pre`], which cannot be wrong. The
/// predictive classifier — and the `debug_assert!(stmts.is_empty())` pair that
/// stood in for the guarantee it could not make — is deleted; the five defects
/// it produced (#945–#948 plus the `ListLit`/`InterpStr` fallthrough this slice
/// found live) are unrepresentable rather than merely fixed.
///
/// What remains here is only *routing*: which emitter shape a branch would take
/// if it were emitted as a block. That is a structural property of the AST, not
/// a prediction about lowering, and it never guessed wrong — every one of the
/// defects was hoisting, none was routing.
pub(super) fn ternary_shaped(
    cond: &Expr,
    then_block: &Block,
    else_block: &Block,
    cx: &LowerCtx,
) -> bool {
    then_block.statements.is_empty()
        && else_block.statements.is_empty()
        && tail_lowers_as_expression(&then_block.tail, cx)
        && tail_lowers_as_expression(&else_block.tail, cx)
        && !cond_has_is_bindings(cond, cx)
}

/// True when [`emit_block_inner`] would lower this tail through its plain-
/// expression arm rather than routing it to a statement shape of its own. A
/// ternary branch — and the branch rebuild both hoisting fallbacks perform —
/// can host an expression and its hoisted statements, but not an emitted
/// `switch` or `if`/`else`.
fn tail_lowers_as_expression(e: &Expr, cx: &LowerCtx) -> bool {
    match &e.kind {
        ExprKind::Match { .. } => false,
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => ternary_shaped(cond, then_block, else_block, cx),
        _ => true,
    }
}

/// Lower the awaited expression of a `let x <- e`, with the call-site
/// `by <Actor>(<identity>)` clause it carries folded into the addressed call.
fn lower_effect_let_value(
    l: &bynk_syntax::ast::LetStmt,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> LoweredExpr {
    // v0.182 (#664): a call-site `by <Actor>(<identity>)` supplies the
    // identity the addressed handler reads as `deps.identity`. Lower it
    // (a test-only brand cast, like an agent key) and stash it for the
    // address-call lowering to fold into that call's deps.
    let saved_identity = cx.call_site_identity.take();
    let saved_claims = cx.call_site_claims.take();
    let saved_claim_lines = cx.call_site_claim_lines.take();
    let saved_actor = cx.call_site_actor.take();
    let saved_no_credential = cx.call_site_no_credential;
    cx.call_site_no_credential = false;
    let saved_bad_signature = cx.call_site_bad_signature.take();
    if let Some(principal) = &l.principal {
        // #706: `by Nobody` drives the route with no credential (the 401
        // path); it names no identity. Any other principal supplies its
        // identity — the *raw* lowered value (`"bob"`), which unit deps
        // folds with an `as any` brand cast and the system driver passes
        // as the JWT `sub` verbatim.
        if principal.actor.name == "Nobody" {
            cx.call_site_no_credential = true;
        } else if let Some(variant) = bad_signature_driver(&principal.actor.name) {
            // #2135: `by Forged` / `by Stale` name no identity either.
            cx.call_site_bad_signature = Some(variant);
        } else {
            cx.call_site_identity = principal.identity.as_ref().map(|id| pre.lower(id, cx));
            // #3052: a `system` call mints its `Oidc` token's claims from the
            // lines of a literal `TokenClaims.of(lines)` (the checker admits
            // no other form there, `bynk.test.system_claims_need_literal`),
            // and never builds the in-process `TokenClaims` value, so the
            // lines are lowered once and in place of it.
            let literal_lines = match principal.claims.as_deref().map(|c| &c.kind) {
                Some(ExprKind::MethodCall {
                    receiver,
                    method,
                    args,
                    ..
                }) if cx.is_system_oidc_body()
                    && method.name == "of"
                    && args.len() == 1
                    && matches!(&receiver.kind, ExprKind::Ident(id) if id.name == "TokenClaims") =>
                {
                    Some(&args[0])
                }
                _ => None,
            };
            if let Some(lines) = literal_lines {
                cx.call_site_claim_lines = Some(pre.lower(lines, cx));
            } else {
                cx.call_site_claims = principal.claims.as_ref().map(|c| pre.lower(c, cx));
            }
            cx.call_site_actor = Some(principal.actor.name.clone());
        }
    }
    let value = pre.lower_node(&l.value, cx);
    cx.call_site_identity = saved_identity;
    cx.call_site_claims = saved_claims;
    cx.call_site_claim_lines = saved_claim_lines;
    cx.call_site_actor = saved_actor;
    cx.call_site_no_credential = saved_no_credential;
    cx.call_site_bad_signature = saved_bad_signature;
    value
}

/// #2640: `par { let a <- e1  let b <- e2 }` →
/// `const [a, b] = await __bynkSettleAll([() => e1, () => e2]);`.
///
/// Each binding is a thunk and the join calls them in source order, so each
/// effect is issued before the next binding is evaluated and before any is
/// awaited. A thunk that throws synchronously (a refinement or bounds check, a
/// provider that throws before it returns a promise) becomes a rejected entry
/// rather than escaping with an earlier effect left unawaited (ADR 0426), and the
/// first rejection in source order still wins (see `settle.ts`). A binding's
/// hoisted statements sit inside its thunk, so they run at its turn. A `do`
/// binding takes a hole in the pattern. A binding with a type annotation types
/// its thunk's result (`(): Promise<T> => …`), so the destructured name has the
/// type `const x: T = await e` gives it. Every effect is still awaited, so
/// ADR 0426's guarantee holds.
fn emit_par(body: &mut Body, p: &bynk_syntax::ast::ParStmt, cx: &mut LowerCtx) {
    // `None` for a `do`, which binds nothing; the name (or `_`) for a `let`.
    let mut names: Vec<Option<&bynk_syntax::ast::Ident>> = Vec::new();
    // Each binding's hoisted statements, its value, and its annotation.
    let mut parts: Vec<(Pre, String, Option<String>)> = Vec::new();
    for st in &p.statements {
        let mut pre = Pre::new();
        match st {
            Statement::EffectLet(l) => {
                let value = lower_effect_let_value(l, &mut pre, cx).into_text();
                names.push(Some(&l.name));
                parts.push((pre, value, l.type_annot.as_ref().map(ts_type_ref)));
            }
            Statement::Do(d) => {
                let value = pre.lower(&d.value, cx);
                names.push(None);
                parts.push((pre, value, None));
            }
            // The checker refuses every other statement in a `par` block.
            _ => {}
        }
    }
    // The names are bound only after every value is lowered, so a binding that
    // reads an outer name its own `let` re-binds still sees the outer one.
    let mut pattern: Vec<String> = names
        .iter()
        .map(|n| n.map(|id| cx.bind_local_name(&id.name)).unwrap_or_default())
        .collect();
    while pattern.last().is_some_and(String::is_empty) {
        pattern.pop();
    }
    cx.note_settle_all();
    // T2.2 (R6.4): see the matching note at `EffectLet`.
    cx.emitted_await = true;
    let hoisted = parts.iter().any(|(pre, ..)| !pre.stmts().is_empty());
    let thunk = |pre: &Pre, value: &str, annot: &Option<String>| -> (String, Vec<String>) {
        let hoists = pre.stmts();
        let asynchronous = hoists.iter().any(|s| s.contains("await "));
        let head = format!(
            "{}(){} =>",
            if asynchronous { "async " } else { "" },
            annot
                .as_ref()
                .map(|t| format!(": globalThis.Promise<{t}>"))
                .unwrap_or_default()
        );
        if hoists.is_empty() {
            (format!("{head} {value}"), Vec::new())
        } else {
            let mut lines = hoists.to_vec();
            lines.push(format!("return {value};"));
            (format!("{head} {{"), lines)
        }
    };
    let open = if pattern.is_empty() {
        String::new()
    } else {
        format!("const [{}] = ", pattern.join(", "))
    };
    if !hoisted {
        let items: Vec<String> = parts
            .iter()
            .map(|(pre, value, annot)| thunk(pre, value, annot).0)
            .collect();
        body.push(Item::Line(format!(
            "{open}await __bynkSettleAll([{}]);",
            items.join(", ")
        )));
        return;
    }
    body.push(Item::Line(format!("{open}await __bynkSettleAll([")));
    for (pre, value, annot) in &parts {
        // The thunks sit one column in and their bodies two, not a whole
        // `INDENT_STEP` (an odd offset the goldens pin), so the extra columns
        // are part of the line's text rather than an `Indent`.
        let (head, lines) = thunk(pre, value, annot);
        if lines.is_empty() {
            body.push(Item::Line(format!(" {head},")));
        } else {
            body.push(Item::Line([" ", head.as_str()].concat()));
            for line in lines {
                body.push(Item::Line(["  ", line.as_str()].concat()));
            }
            body.push(Item::Line(" },".to_string()));
        }
    }
    body.push(Item::Line("]);".to_string()));
}

pub(super) fn emit_statement(out: &mut String, stmt: &Statement, cx: &mut LowerCtx, indent: usize) {
    let mut body = Body::new();
    statement(&mut body, stmt, cx);
    render(&body, out, indent, cx);
}

fn statement(body: &mut Body, stmt: &Statement, cx: &mut LowerCtx) {
    // Slice 1 (ADR 0103 D2): every generated line this statement emits — including
    // the multi-line `?` expansion (temp / Err-guard / unwrap) — anchors to the
    // statement's source span, so a source-map-aware stepper coalesces them into
    // one source step (the slice-0 spike confirmed this).
    body.push(Item::Mark(stmt.span()));
    match stmt {
        Statement::Let(l) => {
            // Track `let x = AgentName(key)` so subsequent `x.method(args)`
            // calls can dispatch through the agent class.
            if l.name.name != "_"
                && let ExprKind::Call {
                    name,
                    args: ctor_args,
                    ..
                } = &l.value.kind
                && cx.local_agents.contains(&name.name)
                && ctor_args.len() == 1
            {
                cx.local_agent_vars
                    .insert(l.name.name.clone(), name.name.clone());
            }
            let mut pre = Pre::new();
            let value = pre.lower_node(&l.value, cx);
            body.extend(pre.items());
            // #908: also covers `_` (a fresh throwaway local; TS allows
            // `const _ = ...` only once per scope) and a re-`let` of a name
            // already bound in this or an enclosing block (which otherwise
            // either collides with the first `const` of that name — TS2451 —
            // or, one block deeper, shadows it into a TDZ read).
            let bind_name = cx.bind_local_name(&l.name.name);
            body.push(const_item(
                pre.all_nodes(),
                &bind_name,
                l.type_annot.as_ref(),
                value,
                false,
            ));
        }
        Statement::EffectLet(l) => {
            // `let x <- expr` → `const x = await expr;`
            let mut pre = Pre::new();
            let value = lower_effect_let_value(l, &mut pre, cx);
            body.extend(pre.items());
            let bind_name = cx.bind_local_name(&l.name.name);
            // T2.2 (R6.4): the literal `await` below is one of the two real
            // sources of effectfulness a synchronous-looking IIFE further out
            // needs to know about — see `emitted_await`.
            cx.emitted_await = true;
            body.push(const_item(
                pre.all_nodes(),
                &bind_name,
                l.type_annot.as_ref(),
                value,
                true,
            ));
        }
        Statement::Expect(a) if a.snapshot.is_some() => {
            // #2842: `expect <value> matches snapshot "<name>"` compares the
            // value's `Json.encode` text with the snapshot the module inlined
            // (`__bynkSnapshotCheck`, `test_runtime/snapshot.ts`).
            let clause = a.snapshot.as_ref().expect("guarded by is_some");
            let span_start = a.span.start;
            let span_end = a.span.end;
            let location = expect_location(cx, span_start);
            let src = expect_source_text(cx, a.value.span);
            let mut pre = Pre::new();
            let json = json_encode_text(&mut pre, &a.value, cx, false);
            body.extend(pre.items());
            let name = bynk_project::json_string(&clause.name);
            let shown = crate::emitter::escape_ts_string(&clause.name);
            match json {
                Some(json) => body.push(Item::Line(format!(
                        "{{ const __d = __bynkSnapshotCheck({name}, {json}); if (__d !== undefined) {{ throw __bynkExpectFailure(\"{location}\", {span_start}, {span_end}, \"expect {src} matches snapshot \\\"{shown}\\\"\\n  \" + __d); }} }}",
                    ))),
                // The checker typed the value; a missing type is drift, said at run time.
                None => body.push(Item::Line(format!(
                        "throw __bynkExpectFailure(\"{location}\", {span_start}, {span_end}, \"expect {src} matches snapshot \\\"{shown}\\\"\\n  the value's type is not known to the snapshot\");",
                    ))),
            }
        }
        Statement::Expect(a) => {
            // Inside a test case body, `expect <pred>` lowers to a runtime check
            // that throws an ExpectationError so the surrounding test-case runner
            // catches it and records the failure (v0.112, renamed from `assert`).
            let span_start = a.value.span.start;
            let span_end = a.value.span.end;
            let location = expect_location(cx, span_start);
            let src = expect_source_text(cx, a.value.span);
            let mut pre = Pre::new();
            let cond = pre.lower(&a.value, cx);
            // Structural expected-vs-actual for a top-level comparison. The
            // predicate is pure (ADR 0144), so re-evaluating the operands for the
            // failure message is observationally identical to the condition.
            let detail = if let ExprKind::BinOp(op, l, r) = &a.value.kind
                && let Some(sym) = comparison_op_symbol(*op)
            {
                let lv = pre.lower(l, cx);
                let rv = pre.lower(r, cx);
                // #2622: an operand that holds a `Decimal` prints through the
                // decimal-aware show, since `JSON.stringify` throws on a `bigint`.
                let decimal = cx
                    .commons()
                    .expr_types
                    .get(&l.id)
                    .is_some_and(|te| mentions_decimal(te.ty, cx));
                let show = if decimal {
                    cx.note_decimal("__bynkDecimalShow");
                    "__bynkDecimalShow"
                } else {
                    "__bynkShow"
                };
                format!(
                    "\"expect {src}\\n  expected: {src}\\n  actual:   \" + {show}(({lv})) + \" {sym} \" + {show}(({rv}))"
                )
            } else if matches!(a.value.kind, ExprKind::Faults(_)) {
                // #1706: the claim's only failure is a call that returned.
                format!("\"expect {src}\\n  the call returned without faulting\"")
            } else {
                format!("\"expect {src}\"")
            };
            body.extend(pre.items());
            body.push(Item::Line(format!(
                    "if (!({cond})) {{ throw __bynkExpectFailure(\"{location}\", {span_start}, {span_end}, {detail}); }}",
                )));
        }
        Statement::Advance(a) => {
            // #2137: move the case's harness clock (`project/test_runtime/clock.ts`),
            // which every participant's `Clock` reads at `system` and the
            // `Clock` double reads below it. A negative amount faults there.
            let mut pre = Pre::new();
            let amount = pre.lower(&a.amount, cx);
            body.extend(pre.items());
            body.push(Item::Line(format!("__bynkAdvanceClock({amount});")));
        }
        Statement::Do(d) => {
            // v0.146 (ADR 0170): `do expr` → `await expr;`. The binder-free
            // `let _ <- expr` — the effect runs and joins the handler, its
            // result discarded (no `const`).
            let mut pre = Pre::new();
            let value = pre.lower_node(&d.value, cx);
            body.extend(pre.items());
            // T2.2 (R6.4): see the matching note at `EffectLet`.
            cx.emitted_await = true;
            body.push(do_item(pre.all_nodes(), value));
        }
        Statement::Par(p) => emit_par(body, p, cx),
        Statement::Assign(a) => {
            // v0.81 (storage track, ADR 0109): `cell := expr` writes the mutable
            // working state in place (`__state.cell = <expr>`). It is staged in
            // memory — read-your-writes within the handler — and flushed once at
            // handler end via `commitState` (which runs the invariant gate before
            // the durable write). A fault before that flush persists nothing.
            let mut pre = Pre::new();
            let value = pre.lower_node(&a.value, cx);
            body.extend(pre.items());
            // Defensive: the checker resolves `:=` to a store cell, so a write
            // outside a store-agent handler (the `None` arm) does not reach
            // emission.
            let target = match cx.agent_store_cells() {
                Some((var, _)) => member_node(var, &a.target.name),
                None => bynk_ts::TsExpr::Ident(a.target.name.clone()),
            };
            body.push(assign_item(pre.all_nodes(), target, value));
        }
    }
}

/// v0.112: the source operator symbol for a comparison `BinOp`, or `None` for
/// non-comparison operators (which get the source-text-only failure report).
fn comparison_op_symbol(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Eq => Some("=="),
        BinOp::NotEq => Some("!="),
        BinOp::Lt => Some("<"),
        BinOp::LtEq => Some("<="),
        BinOp::Gt => Some(">"),
        BinOp::GtEq => Some(">="),
        _ => None,
    }
}

pub(crate) fn write_line(out: &mut String, indent: usize, line: &str) {
    for _ in 0..indent {
        out.push(' ');
    }
    out.push_str(line);
    out.push('\n');
}

/// True if the expression contains an `is` test with at least one
/// non-wildcard binding. Walks through `&&`, `||`, and parens.
fn cond_has_is_bindings(e: &Expr, cx: &LowerCtx) -> bool {
    // #1654: either branch may need a place for declarations — the then-branch
    // for the tests the condition proves when true, the else-branch for those
    // it proves when false (`if !(o is Some(v)) { … } else { v }`).
    [true, false].into_iter().any(|when_true| {
        bynk_check::narrowing::matched_is_tests(e, when_true)
            .into_iter()
            .any(|t| match &t.kind {
                ExprKind::Is { value, pattern } => pattern_is_has_bindings(pattern, value, cx),
                _ => false,
            })
    })
}

/// The per-pattern-kind check `cond_has_is_bindings` delegates to (#474: split
/// out so an or-pattern can recurse into its alternatives).
fn pattern_is_has_bindings(pattern: &Pattern, value: &Expr, cx: &LowerCtx) -> bool {
    match pattern {
        Pattern::Variant {
            variant, bindings, ..
        } => {
            bindings.iter().any(|b| !b.is_wildcard())
                // v0.13: a refined `is`-narrowing introduces a (shadow) binding
                // even though the pattern carries none.
                || (bindings.is_empty() && cx.is_refined_is_check(value, &variant.name))
        }
        Pattern::Record { fields, .. } => fields.iter().any(|(_, sub)| {
            sub.as_ref()
                .is_none_or(|p| matches!(p, Pattern::Binding(_)))
        }),
        Pattern::List { elems, rest, .. } => {
            elems.iter().any(|p| matches!(p, Pattern::Binding(_)))
                || rest.as_ref().is_some_and(|r| r.binder.is_some())
        }
        // #474: Rule 1 guarantees every alternative binds the same names, so
        // any one alternative having bindings means the whole pattern does.
        Pattern::Or(alts, _) => alts
            .iter()
            .any(|alt| pattern_is_has_bindings(alt, value, cx)),
        _ => false,
    }
}

pub(super) fn emit_if_tail(
    out: &mut String,
    cond: &Expr,
    then_block: &Block,
    else_block: &Block,
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    let mut body = Body::new();
    if_tail(&mut body, cond, then_block, else_block, cx, async_tail);
    render(&body, out, indent, cx);
}

fn if_tail(
    body: &mut Body,
    cond: &Expr,
    then_block: &Block,
    else_block: &Block,
    cx: &mut LowerCtx,
    async_tail: bool,
) {
    let mut pre = Pre::new();
    let cond_expr = pre.lower_node(cond, cx);
    body.extend(pre.items());
    // is-binding declarations on the truthy path. #908: scoped to the
    // then-branch only (see the IIFE sibling above for the same reasoning).
    cx.shadow_scopes.push(HashMap::new());
    let mut is_bindings = Vec::new();
    let mut found = false;
    gather_is_bindings_for_emit(cond, true, cx, &mut is_bindings, &mut found);
    let mut then_body = Body::new();
    for b in &is_bindings {
        then_body.push(Item::Line(b.to_string()));
    }
    emit_block_inner(&mut then_body, then_block, cx, async_tail);
    cx.shadow_scopes.pop();
    // #1654: the else-branch's bindings, those the condition's falsity proves.
    cx.shadow_scopes.push(HashMap::new());
    let mut else_bindings = Vec::new();
    gather_is_bindings_for_emit(cond, false, cx, &mut else_bindings, &mut found);
    let mut else_body = Body::new();
    for b in &else_bindings {
        else_body.push(Item::Line(b.to_string()));
    }
    emit_block_inner(&mut else_body, else_block, cx, async_tail);
    cx.shadow_scopes.pop();
    push_if(body, cond_expr, pre.all_nodes(), then_body, else_body);
}

/// v0.130: whether a `match` is literal-kind — its scrutinee is a primitive
/// `Int`/`String`/`Bool` (or a refinement over one), so arms are literal
/// patterns and the emitter switches on the *value* itself, not on a `.tag`
/// discriminant. Mirrors the checker's `literal_base_of` classification.
pub(super) fn is_literal_match(disc_ty: &Option<TyId>, tys: &Arc<Types>) -> bool {
    match disc_ty.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Base(b)) => matches!(b, BaseType::Int | BaseType::String | BaseType::Bool),
        Some(Ty::Named {
            kind: NamedKind::Refined(b),
            ..
        }) => matches!(b, BaseType::Int | BaseType::String | BaseType::Bool),
        _ => false,
    }
}

/// Whether an expression lowers to a stable reference TypeScript can narrow
/// across a `switch` (a variable or a property path rooted in one). Calls and
/// other computed expressions are not narrowable and must be bound to a temp.
fn is_narrowable_path(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ident(_) => true,
        ExprKind::FieldAccess { receiver, .. } => is_narrowable_path(receiver),
        ExprKind::Paren(inner) => is_narrowable_path(inner),
        _ => false,
    }
}

pub(super) fn emit_match_tail(
    out: &mut String,
    discriminant: &Expr,
    arms: &[MatchArm],
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    let mut body = Body::new();
    match_tail(&mut body, discriminant, arms, cx, async_tail);
    render(&body, out, indent, cx);
}

fn match_tail(
    body: &mut Body,
    discriminant: &Expr,
    arms: &[MatchArm],
    cx: &mut LowerCtx,
    async_tail: bool,
) {
    let tys = cx.commons().tys();
    // Anchor the discriminant lowering + `switch (…) {` header to the match's
    // scrutinee span (slice 1); each arm re-anchors to its own span below.
    body.push(Item::Mark(discriminant.span));
    let mut pre = Pre::new();
    let mut disc = pre.lower(discriminant, cx);
    let disc_ty = cx
        .commons()
        .expr_types
        .get(&discriminant.id)
        .map(|te| te.ty);
    body.extend(pre.items());
    // v0.9.2: a statement-position `switch` narrows the scrutinee only when it
    // is a stable reference (a variable or property path). A call discriminant
    // such as `ShortCode.of(raw)` is re-evaluated per arm and TypeScript cannot
    // narrow it (and re-evaluation could repeat side effects), so bind it to a
    // fresh temp once and switch on that. #2617: a path rooted at a constant
    // lowers to a call (`Fallback()`), so it is bound to a temp too.
    if !is_narrowable_path(discriminant) || cx.roots_at_const(discriminant) {
        let tmp = cx.fresh();
        body.push(Item::Line(format!("const {tmp} = {disc};")));
        disc = tmp;
    }
    // ADR 0169: nested/guarded matches lower to an if-chain; flat/unguarded
    // matches keep the `switch`.
    if bynk_model::match_needs_if_chain(arms) {
        match_if_chain(body, &disc, &disc_ty, arms, cx, async_tail);
        return;
    }
    // v0.130: literal-kind matches switch on the value; variant-kind on `.tag`.
    let scrutinee = if is_literal_match(&disc_ty, tys) {
        disc.clone()
    } else {
        format!("{disc}.tag")
    };
    body.push(Item::Line(format!("switch ({scrutinee}) {{")));
    let mut cases = Body::new();
    for arm in arms {
        match_case(&mut cases, &disc, &disc_ty, arm, cx, async_tail);
    }
    body.push(Item::Indent(cases));
    body.push(Item::Line("}".to_string()));
    body.push(Item::Line(
        "throw new globalThis.Error(\"non-exhaustive match\");".to_string(),
    ));
}

pub(super) fn emit_match_case(
    out: &mut String,
    disc_var: &str,
    disc_ty: &Option<TyId>,
    arm: &MatchArm,
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    let mut body = Body::new();
    match_case(&mut body, disc_var, disc_ty, arm, cx, async_tail);
    render(&body, out, indent, cx);
}

fn match_case(
    body: &mut Body,
    disc_var: &str,
    disc_ty: &Option<TyId>,
    arm: &MatchArm,
    cx: &mut LowerCtx,
    async_tail: bool,
) {
    let tys = cx.commons().tys();
    // Anchor this arm's `case`/binding/`return` lines to the arm's source span
    // (slice 1, ADR 0103 D2) — so stepping a `match` walks arm-to-arm.
    body.push(Item::Mark(arm.span));
    // #908: this arm's payload/catch-all bindings live in the `case`'s own JS
    // block, so a read inside the body must resolve to them rather than an
    // outer `let` rename — push a frame and register each below, popped once
    // the whole arm (bindings + body) is emitted.
    cx.shadow_scopes.push(HashMap::new());
    match &arm.pattern {
        Pattern::Wildcard(_) => {
            body.push(Item::Line("default: {".to_string()));
            let mut inner = Body::new();
            match_body(&mut inner, &arm.body, cx, async_tail);
            body.push(Item::Indent(inner));
            body.push(Item::Line("}".to_string()));
        }
        // A bare name binding is a catch-all that binds the scrutinee (ADR 0169).
        Pattern::Binding(id) => {
            body.push(Item::Line("default: {".to_string()));
            let local = cx.bind_local_name(&id.name);
            let mut inner = Body::new();
            inner.push(Item::Line(format!("const {local} = {disc_var};")));
            match_body(&mut inner, &arm.body, cx, async_tail);
            body.push(Item::Indent(inner));
            body.push(Item::Line("}".to_string()));
        }
        // v0.130: a literal pattern lowers to `case <literal>:`. JS `switch`
        // compares with `===`, matching the value semantics we want for
        // `Int`/`String`/`Bool`.
        Pattern::Literal { value, .. } => {
            body.push(Item::Line(format!(
                "case {}: {{",
                literal_case_label(value)
            )));
            let mut inner = Body::new();
            match_body(&mut inner, &arm.body, cx, async_tail);
            body.push(Item::Indent(inner));
            body.push(Item::Line("}".to_string()));
        }
        // #472: a refined arm always trips `match_needs_if_chain`, so the
        // switch path (this function) never sees one in practice — a
        // predicate has no `case`-label lowering, unlike a literal or a tag.
        Pattern::Refined { .. } => {
            unreachable!("a Pattern::Refined arm always routes through emit_match_if_chain")
        }
        // #2641: a record has no tag to switch on, so a record pattern always
        // trips `match_needs_if_chain`.
        Pattern::Record { .. } => {
            unreachable!("a Pattern::Record arm always routes through emit_match_if_chain")
        }
        Pattern::List { .. } => {
            unreachable!("a Pattern::List arm always routes through emit_match_if_chain")
        }
        Pattern::Range(_) => {
            unreachable!("a Pattern::Range arm always routes through emit_match_if_chain")
        }
        Pattern::Variant {
            variant, bindings, ..
        } => {
            body.push(Item::Line(format!(
                "case \"{tag}\": {{",
                tag = variant.name
            )));
            // The switch path only handles flat, unguarded arms (nested/guarded
            // matches take the if-chain path), so each payload binding is an
            // irrefutable name or `_`.
            let mut inner = Body::new();
            for (i, b) in bindings.iter().enumerate() {
                let Pattern::Binding(name) = b.pattern() else {
                    continue;
                };
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(*disc_ty, &variant.name, i, tys)
                    }
                };
                let local = cx.bind_local_name(&name.name);
                inner.push(Item::Line(format!("const {local} = {disc_var}.{field};")));
            }
            match_body(&mut inner, &arm.body, cx, async_tail);
            body.push(Item::Indent(inner));
            body.push(Item::Line("}".to_string()));
        }
        // #474: a bindingless or-pattern (guaranteed by `match_needs_if_chain`
        // routing — a bound/guarded one takes the if-chain path instead) lowers
        // to one fall-through `case` label per alternative, sharing one body —
        // *unless* an alternative is `_`/a bare binding (#825 review): that
        // alone already matches everything (`Pending | _` is exactly as
        // irrefutable as a bare `_`, per `Pattern::is_irrefutable`), and JS
        // `switch` has no `case _:` form, only `default:`. Emitting case
        // labels for the other (now-redundant) alternatives and dropping the
        // wildcard produced malformed, `tsc`-rejected output.
        Pattern::Or(alts, _) if arm.pattern.is_irrefutable() => {
            body.push(Item::Line("default: {".to_string()));
            let mut inner = Body::new();
            match_body(&mut inner, &arm.body, cx, async_tail);
            body.push(Item::Indent(inner));
            body.push(Item::Line("}".to_string()));
        }
        Pattern::Or(alts, _) => {
            let last = alts.len() - 1;
            for (i, alt) in alts.iter().enumerate() {
                let label = match alt {
                    Pattern::Literal { value, .. } => literal_case_label(value),
                    Pattern::Variant { variant, .. } => format!("\"{}\"", variant.name),
                    Pattern::Wildcard(_) | Pattern::Binding(_) => unreachable!(
                        "pattern.is_irrefutable() is false, so no alternative is a wildcard or binding"
                    ),
                    Pattern::Or(..) => unreachable!("an Or's alternatives are always leaves"),
                    Pattern::Record { .. } => {
                        unreachable!("a record alternative routes through emit_match_if_chain")
                    }
                    Pattern::List { .. } => {
                        unreachable!("a list alternative routes through emit_match_if_chain")
                    }
                    Pattern::Range(_) => {
                        unreachable!("a range alternative routes through emit_match_if_chain")
                    }
                    // #472/#474: the parser only ever wraps a *whole*,
                    // already-folded `|`-chain in `Refined` (never the other
                    // way — see `parse_pattern_where_suffix`), so an `Or`'s
                    // alternatives (built from `parse_pattern_base`) can never
                    // themselves be `Refined`.
                    Pattern::Refined { .. } => {
                        unreachable!("an Or's alternatives are never a Refined pattern")
                    }
                };
                if i == last {
                    body.push(Item::Line(format!("case {label}: {{")));
                } else {
                    body.push(Item::Line(format!("case {label}:")));
                }
            }
            let mut inner = Body::new();
            match_body(&mut inner, &arm.body, cx, async_tail);
            body.push(Item::Indent(inner));
            body.push(Item::Line("}".to_string()));
        }
    }
    cx.shadow_scopes.pop();
}

fn match_body(body: &mut Body, arm_body: &MatchBody, cx: &mut LowerCtx, async_tail: bool) {
    match arm_body {
        MatchBody::Expr(e) => {
            if super::tco::emit_continue(body, e, cx) {
                return;
            }
            // #3193: a bare `match` or `if` arm body holding a tail self call is
            // written as statements, as a block tail would be, so its
            // `continue` is not inside an arrow.
            if cx.tail_slot.is_none() && cx.tco.as_ref().is_some_and(|p| p.has_call_in(e)) {
                match &e.kind {
                    ExprKind::Match { discriminant, arms } => {
                        match_tail(body, discriminant, arms, cx, async_tail);
                        return;
                    }
                    ExprKind::If {
                        cond,
                        then_block,
                        else_block,
                    } => {
                        if_tail(body, cond, then_block, else_block, cx, async_tail);
                        return;
                    }
                    _ => {}
                }
            }
            let mut pre = Pre::new();
            let v = pre.absorb(lower_tail_expr(e, cx, async_tail));
            body.extend(pre.items());
            body.push(Item::Line(tail_stmt(cx, &v)));
        }
        MatchBody::Block(b) => emit_block_inner(body, b, cx, async_tail),
    }
}

/// Lower a nested/guarded match to a sequence of independent `if` blocks
/// (ADR 0169, DECISION E). Each arm tests its structural pattern; on a match it
/// binds names, then (if guarded) tests the guard, then runs its body — whose
/// tail `return` short-circuits the remaining arms. A guard failing falls
/// through to the next arm, which a `switch` cannot express. Per-arm span
/// anchoring is preserved (ADR 0103).
pub(super) fn emit_match_if_chain(
    out: &mut String,
    disc_var: &str,
    disc_ty: &Option<TyId>,
    arms: &[MatchArm],
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    let mut body = Body::new();
    match_if_chain(&mut body, disc_var, disc_ty, arms, cx, async_tail);
    render(&body, out, indent, cx);
}

fn match_if_chain(
    body: &mut Body,
    disc_var: &str,
    disc_ty: &Option<TyId>,
    arms: &[MatchArm],
    cx: &mut LowerCtx,
    async_tail: bool,
) {
    // An unguarded irrefutable arm is a catch-all: its body always returns, so
    // the trailing non-exhaustive `throw` would be unreachable (tsc rejects it).
    let has_catchall = arms
        .iter()
        .any(|a| a.guard.is_none() && a.pattern.is_irrefutable());
    let mut declared_here: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for arm in arms {
        body.push(Item::Mark(arm.span));
        // #908: this arm's bindings must resolve within its own body rather
        // than falling through to an outer `let` rename — see the identical
        // reasoning in `emit_match_case`. Pushed/popped once per arm
        // regardless of `has_tests` below, since the concern is which
        // binding a read resolves to, not whether the arm also happens to
        // emit a wrapping JS block.
        cx.shadow_scopes.push(HashMap::new());
        let mut tests = Vec::new();
        pattern_match_tests(disc_var, *disc_ty, &arm.pattern, cx, &mut tests);
        let has_tests = !tests.is_empty();
        // #2641: an arm with no tests declares its bindings at this level, so
        // a second such arm (guarded ones fall through) binding the same name
        // would redeclare it; wrap that one in its own block. The first, and
        // every arm that emitted valid code before, is untouched.
        let names: Vec<&str> = arm
            .pattern
            .bound_names()
            .iter()
            .map(|id| id.name.as_str())
            .collect();
        let needs_block = !has_tests && names.iter().any(|n| declared_here.contains(n));
        if !has_tests && !needs_block {
            declared_here.extend(names.iter().copied());
        }
        if has_tests {
            body.push(Item::Line(format!("if ({}) {{", tests.join(" && "))));
        } else if needs_block {
            body.push(Item::Line("{".to_string()));
        }
        let has_tests = has_tests || needs_block;
        // The arm's own lines sit one level in when it opened a block (`if (…) {`
        // or `{`), and at this level otherwise.
        let mut nested = Body::new();
        {
            let arm_body: &mut Body = if has_tests { &mut nested } else { &mut *body };
            emit_pattern_bindings(arm_body, disc_var, *disc_ty, &arm.pattern, cx);
            if let Some(guard) = &arm.guard {
                let guard_lowered = lower_expr(guard, cx);
                arm_body.extend(guard_lowered.pre_items());
                let gv = guard_lowered.expr.into_text();
                arm_body.push(Item::Line(format!("if ({gv}) {{")));
                let mut guarded = Body::new();
                match_body(&mut guarded, &arm.body, cx, async_tail);
                arm_body.push(Item::Indent(guarded));
                arm_body.push(Item::Line("}".to_string()));
            } else {
                match_body(arm_body, &arm.body, cx, async_tail);
            }
        }
        if has_tests {
            body.push(Item::Indent(nested));
            body.push(Item::Line("}".to_string()));
        }
        cx.shadow_scopes.pop();
    }
    if !has_catchall {
        body.push(Item::Line(
            "throw new globalThis.Error(\"non-exhaustive match\");".to_string(),
        ));
    }
}

/// #2622: whether a value of type `ty` can hold a `Decimal` anywhere: it is one
/// (or a refined or opaque type over one), or a container, record or sum that
/// mentions one. A name that does not resolve counts as no.
fn mentions_decimal(ty: TyId, cx: &LowerCtx) -> bool {
    use std::collections::HashSet;
    fn by_name(name: &str, cx: &LowerCtx, seen: &mut HashSet<String>) -> bool {
        if !seen.insert(name.to_string()) {
            return false;
        }
        let Some(decl) = cx.commons().types.get(name) else {
            return false;
        };
        match &decl.body {
            TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } => {
                *base == BaseType::Decimal
            }
            TypeBody::Record(r) => r.fields.iter().any(|f| by_ref(&f.type_ref, cx, seen)),
            TypeBody::Sum(s) => s
                .variants
                .iter()
                .any(|v| v.payload.iter().any(|f| by_ref(&f.type_ref, cx, seen))),
        }
    }
    fn by_ref(t: &TypeRef, cx: &LowerCtx, seen: &mut HashSet<String>) -> bool {
        match t {
            TypeRef::Base(b, _) => *b == BaseType::Decimal,
            TypeRef::Named(id) => by_name(&id.name, cx, seen),
            TypeRef::Option(a, _) | TypeRef::List(a, _) | TypeRef::Set(a, _) => by_ref(a, cx, seen),
            TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
                by_ref(a, cx, seen) || by_ref(b, cx, seen)
            }
            TypeRef::App { name, args, .. } => {
                by_name(&name.name, cx, seen) || args.iter().any(|a| by_ref(a, cx, seen))
            }
            _ => false,
        }
    }
    fn by_ty(ty: TyId, cx: &LowerCtx, seen: &mut HashSet<String>) -> bool {
        let tys = cx.commons().tys();
        match &*tys.get(ty) {
            Ty::Base(b) => *b == BaseType::Decimal,
            Ty::Option(t) | Ty::List(t) | Ty::Set(t) => by_ty(*t, cx, seen),
            Ty::Map(k, v) | Ty::Result(k, v) => by_ty(*k, cx, seen) || by_ty(*v, cx, seen),
            Ty::Named { name, args, .. } => {
                by_name(name, cx, seen) || args.iter().any(|a| by_ty(*a, cx, seen))
            }
            _ => false,
        }
    }
    by_ty(ty, cx, &mut HashSet::new())
}

#[cfg(test)]
mod body_tests {
    use super::*;
    use crate::emitter::ctx::{BodyMode, ModuleCtx};
    use bynk_check::checker::TypedCommons;
    use bynk_ts::SourceMapBuilder;
    use std::cell::RefCell;

    /// Run `f` with a `LowerCtx` that records into `smb`.
    fn with_cx<R>(smb: &RefCell<SourceMapBuilder>, f: impl FnOnce(&LowerCtx) -> R) -> R {
        let commons = TypedCommons::empty();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(&commons, &cross_context, &runtime_use),
            BodyMode::Method,
        )
        .with_source_map(Some(smb));
        f(&cx)
    }

    fn line(text: &str) -> Item {
        Item::Line(text.to_string())
    }

    fn rendered(body: &Body, indent: usize) -> (String, SourceMapBuilder) {
        let smb = RefCell::new(SourceMapBuilder::new());
        smb.borrow_mut().add_source("t.bynk", "x".repeat(200));
        let mut out = String::new();
        with_cx(&smb, |cx| render(body, &mut out, indent, cx));
        (out, smb.into_inner())
    }

    fn if_node() -> bynk_ts::TsStmt {
        bynk_ts::TsStmt::if_stmt(
            bynk_ts::TsExpr::Ident("flag".to_string()),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::expr_stmt(
                    bynk_ts::TsExpr::Ident("done".to_string()),
                    None,
                )],
                None,
            ),
            None,
        )
    }

    /// A `Line` is `write_line`: only its first line takes the indentation, so
    /// continuation lines keep whatever they carry (#3711, Decision B).
    #[test]
    fn a_line_indents_its_first_line_only() {
        let mut body = Body::new();
        body.push(line("a;\n  b;"));
        let (out, _) = rendered(&body, 4);
        assert_eq!(out, "    a;\n  b;\n");
        let mut expected = String::new();
        write_line(&mut expected, 4, "a;\n  b;");
        assert_eq!(out, expected);
    }

    /// A `Node` is printed at depth 0 with every line indented.
    #[test]
    fn a_node_indents_every_line() {
        let mut body = Body::new();
        body.push(Item::Node(Box::new(if_node())));
        let (out, _) = rendered(&body, 2);
        assert!(out.lines().count() > 1, "{out}");
        assert!(out.lines().all(|l| l.starts_with("  ")), "{out}");
    }

    /// `Indent` nests one `INDENT_STEP` per level, and an empty one is nothing.
    #[test]
    fn indent_nests_one_step_per_level() {
        let mut inner = Body::new();
        inner.push(line("b;"));
        let mut mid = Body::new();
        mid.push(line("a {"));
        mid.push(Item::Indent(inner));
        mid.push(line("}"));
        let mut body = Body::new();
        body.push(Item::Indent(mid));
        body.push(Item::Indent(Body::new()));
        let (out, _) = rendered(&body, 2);
        assert_eq!(out, "    a {\n      b;\n    }\n");
    }

    /// A `Mark` records the offset where the next item starts, including inside
    /// a nested `Indent`.
    #[test]
    fn a_mark_records_the_offset_where_it_is_reached() {
        let mut inner = Body::new();
        inner.push(Item::Mark(Span::new(30, 40)));
        inner.push(line("c;"));
        let mut body = Body::new();
        body.push(Item::Mark(Span::new(0, 10)));
        body.push(line("a;"));
        body.push(Item::Mark(Span::new(10, 20)));
        body.push(line("b {"));
        body.push(Item::Indent(inner));
        body.push(line("}"));
        let (out, got) = rendered(&body, 0);
        let mut want = SourceMapBuilder::new();
        want.add_source("t.bynk", "x".repeat(200));
        want.record(0, Span::new(0, 10));
        want.record("a;\n".len(), Span::new(10, 20));
        want.record("a;\nb {\n".len(), Span::new(30, 40));
        assert_eq!(out, "a;\nb {\n  c;\n}\n");
        assert_eq!(got.to_v3(&out, "t.js"), want.to_v3(&out, "t.js"));
    }

    /// The claim-scope wrapper (#3711, Decision D) as `Line` items around the
    /// body gives the text and the map the old insert-and-shift did.
    #[test]
    fn the_claim_scope_wrapper_matches_insert_and_shift() {
        let build = || {
            let mut inner = Body::new();
            inner.push(line("if (x) {"));
            let mut nested = Body::new();
            nested.push(Item::Mark(Span::new(40, 50)));
            nested.push(line("return 1;"));
            inner.push(Item::Indent(nested));
            inner.push(line("}"));
            let mut body = Body::new();
            body.push(Item::Mark(Span::new(0, 10)));
            body.push(line("const a = 1;"));
            body.push(Item::Mark(Span::new(10, 30)));
            body.push(Item::Node(Box::new(if_node())));
            body.push(Item::Mark(Span::new(30, 60)));
            body.push(Item::Indent(inner));
            body
        };
        for indent in [0, 4] {
            // The old way: render, insert the wrapper's text, shift the map.
            let (plain, mut shifted) = rendered(&build(), indent);
            let (before, after) = claim_scope_wrapper(indent);
            let old_text = format!("{before}{plain}{after}");
            shifted.shift_checkpoints(before.len());
            // The new way: the wrapper is part of the body.
            let (new_text, new_map) = rendered(&claim_scoped(build(), indent), indent);
            assert_eq!(new_text, old_text, "indent {indent}");
            assert_eq!(
                new_map.to_v3(&new_text, "t.js"),
                shifted.to_v3(&old_text, "t.js"),
                "indent {indent}"
            );
        }
    }

    fn ident(name: &str) -> bynk_ts::TsExpr {
        bynk_ts::TsExpr::Ident(name.to_string())
    }

    fn node(stmt: bynk_ts::TsStmt) -> Item {
        Item::Node(Box::new(stmt))
    }

    fn ret(name: &str) -> Item {
        node(bynk_ts::TsStmt::return_stmt(Some(ident(name)), None))
    }

    fn konst(name: &str, init: &str) -> Item {
        node(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident(name.to_string()),
            None,
            ident(init),
            None,
        ))
    }

    /// `to_nodes` hands each mark to the next node only, and refuses text, a
    /// nested `Indent` and a mark with nothing after it.
    #[test]
    fn to_nodes_attaches_a_mark_to_the_next_node_only() {
        let mut body = Body::new();
        body.push(Item::Mark(Span::new(1, 2)));
        body.push(konst("a", "x"));
        body.push(konst("b", "y"));
        body.push(Item::Mark(Span::new(3, 4)));
        body.push(Item::Mark(Span::new(5, 6)));
        body.push(ret("a"));
        let stmts = body.to_nodes().expect("all nodes and marks");
        let spans: Vec<Option<Span>> = stmts.iter().map(|s| s.span).collect();
        // Two marks in a row sit at one offset: the later one wins, as
        // `SourceMapBuilder::record` has it.
        assert_eq!(spans, [Some(Span::new(1, 2)), None, Some(Span::new(5, 6))]);

        let mut text = Body::new();
        text.push(konst("a", "x"));
        text.push(line("b;"));
        assert!(text.to_nodes().is_none());
        let mut nested = Body::new();
        nested.push(Item::Indent(Body::new()));
        assert!(nested.to_nodes().is_none());
        let mut trailing = Body::new();
        trailing.push(konst("a", "x"));
        trailing.push(Item::Mark(Span::new(1, 2)));
        assert!(trailing.to_nodes().is_none());
    }

    /// An `if` built from bodies that carry marks renders, with its map, exactly
    /// as the `Line` form around the same bodies does, at every indent.
    #[test]
    fn an_if_node_matches_the_line_form_with_its_checkpoints() {
        let then_body = || {
            let mut b = Body::new();
            b.push(Item::Mark(Span::new(10, 20)));
            b.push(konst("a", "x"));
            b.push(konst("b", "y"));
            b.push(Item::Mark(Span::new(20, 30)));
            b.push(ret("a"));
            b
        };
        let else_body = || {
            let mut b = Body::new();
            b.push(Item::Mark(Span::new(40, 50)));
            b.push(ret("c"));
            b
        };
        for indent in [0, 2, 6] {
            let mut as_node = Body::new();
            as_node.push(Item::Mark(Span::new(0, 10)));
            push_if(
                &mut as_node,
                LoweredExpr::Node(ident("k")),
                true,
                then_body(),
                else_body(),
            );
            assert!(matches!(as_node.0.last(), Some(Item::Node(_))));
            let mut as_lines = Body::new();
            as_lines.push(Item::Mark(Span::new(0, 10)));
            as_lines.push(line("if (k) {"));
            as_lines.push(Item::Indent(then_body()));
            as_lines.push(line("} else {"));
            as_lines.push(Item::Indent(else_body()));
            as_lines.push(line("}"));
            let (node_text, node_map) = rendered(&as_node, indent);
            let (line_text, line_map) = rendered(&as_lines, indent);
            assert_eq!(node_text, line_text, "indent {indent}");
            assert_eq!(
                node_map.to_v3(&node_text, "t.js"),
                line_map.to_v3(&line_text, "t.js"),
                "indent {indent}"
            );
        }
    }

    /// An `if` stays text when its condition is text, a body holds a line, or a
    /// hoisted statement before it is text, and the bodies keep their marks.
    #[test]
    fn an_if_stays_lines_when_any_part_is_text() {
        let nodes = || {
            let mut b = Body::new();
            b.push(Item::Mark(Span::new(10, 20)));
            b.push(ret("a"));
            b
        };
        let texty = || {
            let mut b = Body::new();
            b.push(line("is_binding;"));
            b.push(ret("a"));
            b
        };
        let cases = [
            (LoweredExpr::Text("k".into()), true, nodes(), nodes()),
            (LoweredExpr::Node(ident("k")), false, nodes(), nodes()),
            (LoweredExpr::Node(ident("k")), true, texty(), nodes()),
            (LoweredExpr::Node(ident("k")), true, nodes(), texty()),
        ];
        for (cond, pre_nodes, then_body, else_body) in cases {
            let mut body = Body::new();
            push_if(&mut body, cond, pre_nodes, then_body, else_body);
            assert!(
                matches!(body.0.first(), Some(Item::Line(l)) if l == "if (k) {"),
                "stays text"
            );
            assert_eq!(body.0.len(), 5);
        }
    }

    /// `do` and assignment: a node over a node value and clean hoists, text
    /// otherwise (they are reached from an agent handler, which the lib's own
    /// unit tests cannot compile in-process).
    #[test]
    fn do_and_assignment_are_nodes_only_over_node_values() {
        let printed = |item: &Item| match item {
            Item::Node(stmt) => ("node", bynk_ts::print_stmt(stmt, 0)),
            Item::Line(text) => ("line", format!("{text}\n")),
            _ => unreachable!("a statement item"),
        };
        let call = || {
            LoweredExpr::Node(bynk_ts::TsExpr::Call {
                callee: Box::new(ident("f")),
                args: vec![ident("x")],
            })
        };
        let text = || LoweredExpr::Text("a && b".into());
        assert_eq!(
            printed(&do_item(true, call())),
            ("node", "await f(x);\n".into())
        );
        assert_eq!(
            printed(&do_item(false, call())),
            ("line", "await f(x);\n".into())
        );
        assert_eq!(
            printed(&do_item(true, text())),
            ("line", "await a && b;\n".into())
        );
        let member = || member_node("__state", "count");
        assert_eq!(
            printed(&assign_item(true, member(), call())),
            ("node", "__state.count = f(x);\n".into())
        );
        assert_eq!(
            printed(&assign_item(false, member(), call())),
            ("line", "__state.count = f(x);\n".into())
        );
        assert_eq!(
            printed(&assign_item(true, ident("n"), text())),
            ("line", "n = a && b;\n".into())
        );
    }

    /// A nested checkpoint inside a node lands `indent` further on for every
    /// line before it, and one more when it is mid-line (an inline branch).
    #[test]
    fn a_nested_checkpoint_is_adjusted_for_the_indent_the_adapter_adds() {
        let inline_if = bynk_ts::TsStmt::if_stmt(
            ident("c"),
            bynk_ts::TsStmt::return_stmt(Some(ident("r")), Some(Span::new(7, 8))),
            None,
        );
        let mut body = Body::new();
        body.push(node(inline_if));
        let (out, got) = rendered(&body, 4);
        assert_eq!(out, "    if (c) return r;\n");
        let mut want = SourceMapBuilder::new();
        want.add_source("t.bynk", "x".repeat(200));
        // After `    if (c) `, the inline branch's own start.
        want.record("    if (c) ".len(), Span::new(7, 8));
        assert_eq!(got.to_v3(&out, "t.js"), want.to_v3(&out, "t.js"));
    }
}

/// 1A.11 (#3712): the statement producers build nodes when every part is a
/// node, and today's text otherwise. `LINES_RENDERED` counts the `Item::Line`s
/// the emitter rendered, so "0" means every statement was a node.
#[cfg(test)]
mod stmt_node_tests {
    use super::*;

    /// `(Item::Line items rendered, emitted module)` for a single commons.
    fn emitted(src: &str) -> (usize, String) {
        LINES_RENDERED.with(|c| c.set(0));
        let ts = crate::testkit::emit_source(src);
        (LINES_RENDERED.with(|c| c.get()), ts)
    }

    /// The body of `export [async ]function <name>` — its lines, up to the
    /// closing brace at column 0.
    fn body_of(ts: &str, name: &str) -> String {
        let head = format!("function {name}(");
        let at = ts.find(&head).unwrap_or_else(|| panic!("no {name}:\n{ts}"));
        let rest = &ts[at..];
        let open = rest.find('\n').expect("a body") + 1;
        let end = rest.find("\n}\n").expect("a closing brace");
        rest[open..end + 1].to_string()
    }

    const EFFECTS: &str = "fn bump(n: Int) -> Effect[Int] {\n  Effect.pure(n)\n}\n\n";

    #[test]
    fn let_let_effect_do_and_return_over_nodes_are_nodes() {
        let src = format!(
            "commons stmts\n\n{EFFECTS}fn pure(x: Int) -> Int {{\n  let y = x + 1\n  y * 2\n}}\n\nfn eff(x: Int) -> Effect[Int] {{\n  let a <- bump(x)\n  do bump(a)\n  Effect.pure(a)\n}}\n"
        );
        let (lines, ts) = emitted(&src);
        assert_eq!(
            body_of(&ts, "pure"),
            "  const y = x + 1;\n  return y * 2;\n"
        );
        assert_eq!(
            body_of(&ts, "eff"),
            "  const a = await bump(x);\n  await bump(a);\n  return a;\n"
        );
        assert_eq!(lines, 0, "{ts}");
    }

    #[test]
    fn an_annotated_let_is_a_node_when_the_type_prints_identically() {
        // One per `TypeRef` shape; each must print through `ts_type_ref_to_ts_type`
        // exactly as `ts_type_ref` does, or the form would stay a `Line`.
        let src = "commons annot\n\n\
            type Id = String where NonEmpty\n\n\
            fn f(x: Int, id: Id, o: Option[Int], r: Result[Int, String], l: List[Int], m: Map[String, Int], s: Set[Int]) -> Int {\n  \
            let a: Int = x\n  \
            let b: String = \"s\"\n  \
            let c: Bool = true\n  \
            let d: Id = id\n  \
            let e: Option[Int] = o\n  \
            let g: Result[Int, String] = r\n  \
            let h: List[Int] = l\n  \
            let i: Map[String, Int] = m\n  \
            let j: Set[Int] = s\n  \
            let k: List[Option[Id]] = []\n  \
            a\n}\n";
        let (lines, ts) = emitted(src);
        assert_eq!(lines, 0, "{ts}");
        let body = body_of(&ts, "f");
        for want in [
            "const a: number = x;",
            "const b: string = \"s\";",
            "const c: boolean = true;",
            "const d: Id = id;",
            "const e: Option<number> = o;",
            "const g: Result<number, string> = r;",
            "const h: readonly number[] = l;",
        ] {
            assert!(body.contains(want), "{want}\n{body}");
        }
    }

    #[test]
    fn text_valued_statements_keep_their_text_byte_for_byte() {
        // `&&` and `Effect.pure(…)` still lower to text, so the statement that
        // holds one is a `Line` with today's bytes.
        let src = "commons texty\n\n\
            fn f(a: Bool, b: Bool) -> Bool {\n  let v = a && b\n  let w: Bool = a && b\n  a && b\n}\n\n\
            fn g(x: Int) -> Effect[Int] {\n  let r <- Effect.pure(x)\n  let s: Int <- Effect.pure(r)\n  do Effect.pure(s)\n  Effect.pure(s)\n}\n";
        let (lines, ts) = emitted(src);
        assert_eq!(
            body_of(&ts, "f"),
            "  const v = a && b;\n  const w: boolean = a && b;\n  return a && b;\n"
        );
        assert_eq!(
            body_of(&ts, "g"),
            "  const r = await globalThis.Promise.resolve(x);\n  const s: number = await globalThis.Promise.resolve(r);\n  await globalThis.Promise.resolve(s);\n  return s;\n"
        );
        // Five text statements and the tail `return undefined`-style ones.
        assert!(lines >= 6, "{lines}\n{ts}");
    }

    #[test]
    fn a_ternary_tail_is_a_return_of_a_parenthesised_conditional() {
        let src = "commons tern\n\n\
            fn f(c: Bool, x: Int, y: Int) -> Int {\n  if c { x } else { y }\n}\n";
        let (lines, ts) = emitted(src);
        assert_eq!(lines, 0, "{ts}");
        assert_eq!(body_of(&ts, "f"), "  return (c ? x : y);\n");
    }

    #[test]
    fn a_ternary_whose_branch_is_a_ternary_or_a_lower_precedence_binary_keeps_its_bytes() {
        // 1A.8 review nit: the `Paren(Conditional)` keeps today's text when a
        // branch is itself a ternary (a nested `Paren`) or a binary that binds
        // more loosely than `?:`'s operands do.
        let src = "commons tern2\n\n\
            fn f(c: Bool, d: Bool, x: Int, y: Int, z: Int) -> Int {\n  if c { if d { x } else { y } } else { z }\n}\n\n\
            fn g(c: Bool, x: Int, y: Int) -> Bool {\n  if c { x < y } else { x == y }\n}\n\n\
            fn h(c: Bool, x: Int, y: Int) -> Int {\n  if c { x + y } else { x - y }\n}\n";
        let (lines, ts) = emitted(src);
        assert_eq!(lines, 0, "{ts}");
        assert_eq!(body_of(&ts, "f"), "  return (c ? (d ? x : y) : z);\n");
        assert_eq!(body_of(&ts, "g"), "  return (c ? x < y : x === y);\n");
        assert_eq!(body_of(&ts, "h"), "  return (c ? x + y : x - y);\n");
    }

    #[test]
    fn a_hoisted_branch_ternary_becomes_an_if_node() {
        // A branch that hoists (`?`) cannot be a ternary: it is the statement
        // `if`, both bodies converted.
        let src = "commons hoist\n\n\
            fn f(c: Bool, r: Result[Int, String]) -> Result[Int, String] {\n  if c { Ok(r?) } else { Ok(0) }\n}\n";
        let (lines, ts) = emitted(src);
        assert_eq!(lines, 0, "{ts}");
        assert_eq!(
            body_of(&ts, "f"),
            "  if (c) {\n    const __r0 = r;\n    if (__r0.tag === \"Err\") return __r0;\n    return Ok(__r0.value);\n  } else {\n    return Ok(0);\n  }\n"
        );
    }

    #[test]
    fn an_if_tail_nested_two_deep_with_let_and_question_in_each_branch_is_all_nodes() {
        let src = "commons deep\n\n\
            fn f(a: Bool, b: Bool, r: Result[Int, String]) -> Result[Int, String] {\n  \
            if a {\n    let x = r?\n    if b {\n      let y = r?\n      Ok(x + y)\n    } else {\n      let z = r?\n      Ok(x + z)\n    }\n  } else {\n    let w = r?\n    Ok(w)\n  }\n}\n";
        let (lines, ts) = emitted(src);
        assert_eq!(lines, 0, "{ts}");
        assert_eq!(
            body_of(&ts, "f"),
            "  if (a) {\n    const __r0 = r;\n    if (__r0.tag === \"Err\") return __r0;\n    const x = __r0.value;\n    if (b) {\n      const __r1 = r;\n      if (__r1.tag === \"Err\") return __r1;\n      const y = __r1.value;\n      return Ok(x + y);\n    } else {\n      const __r2 = r;\n      if (__r2.tag === \"Err\") return __r2;\n      const z = __r2.value;\n      return Ok(x + z);\n    }\n  } else {\n    const __r3 = r;\n    if (__r3.tag === \"Err\") return __r3;\n    const w = __r3.value;\n    return Ok(w);\n  }\n"
        );
    }

    #[test]
    fn an_if_tail_with_an_is_binding_keeps_its_text_lines() {
        // The `is`-binding declaration is a text line, so the `if` stays text
        // around bodies that are still nodes inside it.
        let src = "commons isb\n\n\
            fn f(o: Option[Int]) -> Int {\n  if o is Some(v) {\n    let k = v + 1\n    k\n  } else {\n    0\n  }\n}\n";
        let (lines, ts) = emitted(src);
        assert!(lines > 0, "{ts}");
        assert_eq!(
            body_of(&ts, "f"),
            "  if (((__c: boolean) => __c)(o.tag === \"Some\")) {\n    const v = (o as Extract<typeof o, { tag: \"Some\" }>).value;\n    const k = v + 1;\n    return k;\n  } else {\n    return 0;\n  }\n"
        );
    }

    #[test]
    fn the_tail_slot_form_stays_a_line() {
        // #1750: `slot = value; break label;` is two statements on one line, so
        // it is text until the labelled block is a node (S3).
        let src = "commons slot\n\n\
            fn f(a: Option[Int], r: Result[Int, String]) -> Result[Int, String] {\n  \
            let n = if a is Some(v) { r? + v } else { 0 }\n  Ok(n)\n}\n";
        let (lines, ts) = emitted(src);
        assert!(lines >= 2, "{ts}");
        let body = body_of(&ts, "f");
        assert!(
            body.contains("      __r0 = __r1.value + v; break __r0_out;\n"),
            "{body}"
        );
        assert!(body.contains("      __r0 = 0; break __r0_out;\n"), "{body}");
    }
}
