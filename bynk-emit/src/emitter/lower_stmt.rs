//! Statement emission — the half of lowering whose contract is "append
//! statements to `out` at `indent`": block and function bodies, statements,
//! tail positions, hoisting, `par` and statement-position `match`. Split out of
//! `lower.rs` (back-half paydown 1A.9, S0 of the statement-emitter design note
//! #3708); expression lowering stays there and is reached via `super::lower`.

use std::sync::Arc;

use bynk_check::actors::CallSitePrincipal;
use bynk_check::checker::{Callee, NamedKind, Ty, TyId};
use bynk_check::kernel_op::{StaticOp, TokenClaimsStatic};
use bynk_syntax::ast::{
    BaseType, BinOp, Block, Branching, ChainLink, Expr, ExprKind, MatchArm, Pattern,
    PatternBindingKind, Statement, TypeBody,
};
use bynk_syntax::span::Span;

use super::ctx::TailSlot;
use super::lower::{
    Lowered, PatPath, Pre, and_join, build_value_bin_op, call_site_principal,
    claim_scope_declaration, emit_pattern_bindings, expect_location_raw, expect_source_text_raw,
    hoist_if_slot, literal_label_node, member_node, pattern_tests, ternary_built,
};
use super::lower_call::codec_statics::json_encode;
use super::lower_ctx::LowerCtx;
use super::ts_types::{ts_ty_to_ts_type, ts_type_ref_to_ts_type};
use super::*;
use crate::ts_build::{binary, ident, num_lit, str_lit};

/// The statement emitter builds a [`Body`] — a list of [`Item`]s — instead of
/// appending text to a caller's buffer; [`render`] is the one adapter that
/// turns it back into text and records the source-map checkpoints (back-half
/// paydown 1A.10, S1 of the statement-emitter design note #3708). A body holds
/// statement nodes and source-map marks only (1A.35b); the entry points keep their
/// `(out, …, indent)` signatures as build-then-render wrappers.
pub(super) struct Body(Vec<Item>);

pub(super) enum Item {
    /// A typed statement, printed at depth 0 with every line indented.
    // Boxed: a `TsStmt` dwarfs the other variant.
    Node(Box<bynk_ts::TsStmt>),
    /// A source-map checkpoint at the offset where the next item starts.
    Mark(Span),
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

    /// Append `other`'s items, at the same level.
    pub(super) fn append(&mut self, other: Body) {
        self.0.extend(other.0);
    }

    /// The body as the statements of a typed function or method body (1A.15, Decision A of
    /// #3716), or of any nested block: what the callers of the statement emitter hand to their
    /// `TsDecl::Function`, `TsObjectEntry::Method` or `TsStmt::block` instead of rendering the
    /// body to text.
    ///
    /// An [`Item::Node`] passes through, as itself. An [`Item::Mark`] goes on the **next**
    /// statement's [`bynk_ts::TsStmt::span`], and on that statement only: a statement's hoisted
    /// lines followed by its own give the span to the first, which is the nearest-enclosing rule
    /// the one checkpoint implements today. Two marks in a row sit at one offset, where the later
    /// replaces the earlier, as `SourceMapBuilder::record` does; the printer records the span
    /// where that statement starts.
    pub(super) fn into_stmts(self) -> Vec<bynk_ts::TsStmt> {
        let (stmts, pending) = self.into_stmts_pending();
        // A mark with no statement after it would be dropped silently. Every
        // producer pushes its content after its mark; this makes that explicit.
        debug_assert!(
            pending.is_none(),
            "into_stmts: a trailing Item::Mark has no statement to carry its span"
        );
        stmts
    }

    /// [`Body::into_stmts`], handing back a trailing [`Item::Mark`] that no statement follows, for
    /// a caller that attaches it to the statement it adds next (a runner's `return`).
    fn into_stmts_pending(self) -> (Vec<bynk_ts::TsStmt>, Option<Span>) {
        let mut stmts = Vec::with_capacity(self.0.len());
        let mut pending: Option<Span> = None;
        for item in self.0 {
            match item {
                Item::Mark(span) => pending = Some(span),
                Item::Node(stmt) => {
                    let mut stmt = *stmt;
                    stmt.span = stmt.span.or(pending.take());
                    stmts.push(stmt);
                }
            }
        }
        (stmts, pending)
    }
}

/// A case body built as statements, for the harness (1B.5b): the statements the lowering emits,
/// with the source-map marks they carry, and the tail's hoisted statements after them. The
/// statements print the text the case runner used to splice, with the same checkpoints.
pub(crate) struct StatementSink(Body);

impl StatementSink {
    pub(crate) fn new() -> Self {
        StatementSink(Body::new())
    }

    /// Lower `stmt` into the sink.
    pub(crate) fn statement(&mut self, stmt: &Statement, cx: &mut LowerCtx) {
        statement(&mut self.0, stmt, cx);
    }

    /// A source-map checkpoint for the statement added next.
    pub(crate) fn mark(&mut self, span: Span) {
        self.0.push(Item::Mark(span));
    }

    /// Statements already built, added as they are.
    pub(crate) fn nodes(&mut self, stmts: Vec<bynk_ts::TsStmt>) {
        for stmt in stmts {
            self.0.push(Item::Node(Box::new(stmt)));
        }
    }

    /// The statements, and a trailing mark no statement follows (to go on the next one).
    pub(crate) fn finish(self) -> (Vec<bynk_ts::TsStmt>, Option<Span>) {
        self.0.into_stmts_pending()
    }
}

/// Render `body` into `out` at `indent`, recording each [`Item::Mark`] with
/// `cx.record_span` at the offset where it is reached.
#[cfg(test)]
pub(super) fn render(body: &Body, out: &mut String, indent: usize, cx: &LowerCtx) {
    render_with(body, out, indent, Some(cx));
}

#[cfg(test)]
fn render_with(body: &Body, out: &mut String, indent: usize, cx: Option<&LowerCtx>) {
    for item in &body.0 {
        match item {
            Item::Node(stmt) => {
                // A node can carry checkpoints of its own, one per nested
                // statement that has a span (Decision C of #3712). The printer
                // reports each at a depth-0 offset; this adapter then writes
                // `indent` columns before every line, so a checkpoint lands
                // `indent` further on for each line before it, and one more
                // when it is mid-line (an inline branch), after its own line's
                // indentation.
                let (text, spans) = bynk_ts::print_stmt_with_spans(stmt, 0);
                if let Some(cx) = cx {
                    let base = out.len();
                    for (offset, span) in spans {
                        let before = &text[..offset];
                        let mid_line = !before.is_empty() && !before.ends_with('\n');
                        let lines = before.matches('\n').count() + usize::from(mid_line);
                        cx.record_span(base + offset + lines * indent, span);
                    }
                }
                for line in text.lines() {
                    write_line(out, indent, line);
                }
            }
            Item::Mark(span) => {
                if let Some(cx) = cx {
                    cx.record_span(out.len(), *span);
                }
            }
        }
    }
}

/// The items of a block lowered as a function body, not yet rendered, for a
/// caller that may turn it into nodes (1A.12) or render it. It preserves the
/// enclosing `return_ty`.
pub(super) fn block_body(block: &Block, cx: &mut LowerCtx, async_tail: bool) -> Body {
    let mut body = Body::new();
    block_body_into(&mut body, block, cx, async_tail);
    body
}

/// As [`block_body`], appending to `body`.
pub(super) fn block_body_into(body: &mut Body, block: &Block, cx: &mut LowerCtx, async_tail: bool) {
    // v0.154 (ADR 0178): **preserve** the enclosing `return_ty` rather than
    // resetting it. Nested control-flow blocks (an `if`/`else` or `match` arm
    // *within* a function body) reach here, and a `return` from them returns
    // from the whole function — so an embedding `?` inside them must still see
    // the function's return type. Only a genuine function/handler boundary
    // (`_with_return`) rebinds it. A top-level body with no embedding context
    // (a mock/test that never called `_with_return`) simply inherits the
    // ambient `None`.
    emit_block_inner(body, block, cx, async_tail);
}

/// [`emit_block_as_function_body_with_return`], as the statements of a typed
/// function or method body rather than text (1A.15, Decision A of #3716).
/// `indent` is the indentation the printer will give them.
pub(crate) fn function_body_stmts(
    block: &Block,
    cx: &mut LowerCtx,
    async_tail: bool,
    return_type: Option<&TypeRef>,
) -> Vec<bynk_ts::TsStmt> {
    block_body_with_return(block, cx, async_tail, return_type).into_stmts()
}

/// The items of a function body lowered with its enclosing return type bound,
/// not yet rendered: what both entry points above start from.
pub(super) fn block_body_with_return(
    block: &Block,
    cx: &mut LowerCtx,
    async_tail: bool,
    return_type: Option<&TypeRef>,
) -> Body {
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
        body = claim_scoped(body);
    }
    body
}

/// `body` inside the claim scope (W30, #2840), as nodes (1A.15, Decision C of
/// #3716): the `__claims` scope, `let __r;`, then a `try` whose block assigns the
/// body's result, run in an async closure, to `__r`, and whose `catch` releases
/// the claims and rethrows; after it the claims are confirmed and the result is
/// returned. `indent` is the column of the first statement.
///
/// The `__claims` initialiser is a node too ([`claim_scope_declaration`], 1A.35a part B). The
/// layout is the printer's own, which puts the body one `try` block and one closure deeper than
/// the text wrapper did.
pub(super) fn claim_scoped(body: Body) -> Body {
    use bynk_ts::{TsBindingName, TsExpr, TsLit, TsStmt};
    let settle = |result: TsExpr, threw: bool| {
        TsStmt::expr_stmt(
            TsExpr::Await(Box::new(TsExpr::Call {
                callee: Box::new(member_node("__claims", "settle")),
                args: vec![result, TsExpr::Lit(TsLit::Bool(threw))],
            })),
            None,
        )
    };
    // The body sits in the closure, a `try` block and an arrow body deeper.
    let inner = body.into_stmts();
    let closure = TsExpr::Arrow {
        params: Vec::new(),
        is_async: true,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Multiline(inner)),
    };
    let run = TsStmt::assign(
        ident("__r"),
        TsExpr::Await(Box::new(TsExpr::Call {
            callee: Box::new(TsExpr::Paren(Box::new(closure))),
            args: Vec::new(),
        })),
        None,
    );
    let on_throw = TsStmt::block(
        vec![
            settle(ident("undefined"), true),
            TsStmt::throw_stmt(ident("__e"), None),
        ],
        None,
    );
    let mut wrapped = Body::new();
    wrapped.push(stmt_item(claim_scope_declaration()));
    wrapped.push(stmt_item(TsStmt::let_stmt(
        TsBindingName::Ident("__r".to_string()),
        None,
        None,
        None,
    )));
    wrapped.push(stmt_item(TsStmt::try_catch(
        TsStmt::block(vec![run], None),
        Some("__e"),
        on_throw,
        None,
    )));
    wrapped.push(stmt_item(settle(TsExpr::Ident("__r".to_string()), false)));
    wrapped.push(stmt_item(TsStmt::return_stmt(Some(ident("__r")), None)));
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
    match block.tail.kind.branching() {
        Some(Branching::Match { discriminant, arms }) => {
            match_tail(body, discriminant, arms, cx, async_tail);
        }
        Some(Branching::If {
            cond,
            then_block,
            else_block,
        }) if ternary_shaped(cond, then_block, else_block, cx)
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
                // `finish_built` is the one place a `bynk_ts::TsExpr` becomes a
                // `bynk_ts::TsExpr`; its hoists are empty here.
                let ternary = Pre::new()
                    .finish(ternary_built(cond_expr, then_tail.expr, else_tail.expr))
                    .expr;
                push_tail(body, cx, ternary);
            } else {
                // A branch hoisted after all — the case `simple_expr` used to
                // mispredict (#945, and the `ListLit`/`InterpStr` fallthrough
                // this slice closes). A statement `if` is where a hoisted `?`'s
                // early return belongs, so emit that instead of a ternary that
                // has nowhere to put it. Byte-identical to what `if_tail`
                // produces for this shape (both branches are statement-free and
                // their tails lower as plain expressions), rebuilt from the
                // values already in hand.
                let then_body = pure_tail_branch(then_tail, cx);
                let else_body = pure_tail_branch(else_tail, cx);
                push_if(body, cond_expr, then_body, else_body);
            }
        }
        Some(Branching::If {
            cond,
            then_block,
            else_block,
        }) => {
            if_tail(body, cond, then_block, else_block, cx, async_tail);
        }
        // #3193: a self tail call is a `continue`, not a `return`.
        None if super::tco::emit_continue(body, &block.tail, cx) => {}
        None => {
            let tail = lower_tail_expr(&block.tail, cx, async_tail);
            body.extend(tail.pre_items());
            push_tail(body, cx, tail.expr);
        }
    }
    cx.shadow_scopes.pop();
}

fn stmt_item(stmt: bynk_ts::TsStmt) -> Item {
    Item::Node(Box::new(stmt))
}

/// #1750: the statement that hands a block's tail value to its sink: `return value;` in a
/// function, lambda or arrow body, or, inside a statement-form value expression
/// ([`hoist_value_as_statement`]), the assignment to its slot followed by a labelled `break`
/// out of its block. `site` is where a value that is not a node is reported.
fn push_tail(body: &mut Body, cx: &LowerCtx, value: bynk_ts::TsExpr) {
    let n = value;
    match &cx.tail_slot {
        None => body.push(stmt_item(bynk_ts::TsStmt::return_stmt(Some(n), None))),
        Some(TailSlot { slot, label }) => {
            body.push(stmt_item(bynk_ts::TsStmt::assign(
                bynk_ts::TsExpr::Ident(slot.clone()),
                n,
                None,
            )));
            body.push(stmt_item(bynk_ts::TsStmt::break_label_stmt(
                label.clone(),
                None,
            )));
        }
    }
}

/// `if (cond) { then } else { else }` as one `If` node: the condition is a node, and both bodies
/// are statements ([`Body::to_nodes`], checkpoints carried onto their statements). `site` is
/// where a condition or body that is not nodes is reported.
fn push_if(body: &mut Body, cond: bynk_ts::TsExpr, then_body: Body, else_body: Body) {
    let then_stmts = then_body.into_stmts();
    let else_stmts = else_body.into_stmts();
    body.push(stmt_item(bynk_ts::TsStmt::if_else_same_line_stmt(
        cond,
        bynk_ts::TsStmt::block(then_stmts, None),
        bynk_ts::TsStmt::block(else_stmts, None),
        None,
    )));
}

/// `await <value>;` as an item: an `ExprStmt(Await)` node.
fn do_item(value: bynk_ts::TsExpr) -> Item {
    stmt_item(bynk_ts::TsStmt::expr_stmt(
        bynk_ts::TsExpr::Await(Box::new(value)),
        None,
    ))
}

/// `<target> = <value>;` as an item: an `Assign` node. `target` is a genuine `Ident` or
/// `Member`.
pub(super) fn assign_item(target: bynk_ts::TsExpr, value: bynk_ts::TsExpr) -> Item {
    stmt_item(bynk_ts::TsStmt::assign(target, value, None))
}

/// `const <name>[: <ty>] = [await ]<value>;` as an item (1A.11): a `Const` node, the annotation
/// a type node.
pub(super) fn const_item(
    name: &str,
    annot: Option<&TypeRef>,
    value: bynk_ts::TsExpr,
    awaited: bool,
) -> Item {
    const_node_item(name, annot, value, awaited)
}

/// [`const_item`] over a value that is already a node.
pub(super) fn const_node_item(
    name: &str,
    annot: Option<&TypeRef>,
    value: bynk_ts::TsExpr,
    awaited: bool,
) -> Item {
    let init = if awaited {
        bynk_ts::TsExpr::Await(Box::new(value))
    } else {
        value
    };
    stmt_item(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(name.to_string()),
        annot.map(|a| ts_type_ref_to_ts_type(a, None)),
        init,
        None,
    ))
}

/// One branch of the statement `if` above: the tail's hoisted statements
/// followed by its `return` (or, #1750, its sink's assignment). Exactly the
/// body [`emit_block_inner`]'s own `_` arm builds for a statement-free block,
/// which is what `if_tail` would have recursed into.
fn pure_tail_branch(tail: Lowered, cx: &LowerCtx) -> Body {
    let mut body = Body::new();
    body.extend(tail.pre_items());
    push_tail(&mut body, cx, tail.expr);
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
            let cond_expr = pre.lower_node(cond, cx);
            let then_tail = lower_tail_expr(&then_block.tail, cx, true);
            let else_tail = lower_tail_expr(&else_block.tail, cx, true);
            if then_tail.pre.is_empty() && else_tail.pre.is_empty() {
                return pre.finish(ternary_built(cond_expr, then_tail.expr, else_tail.expr));
            }
            let slot_ty = async_tail_slot_ty(e, cx);
            let slot = hoist_if_slot(&mut pre, slot_ty, cond_expr, then_tail, else_tail, cx);
            pre.finish(bynk_ts::TsExpr::Ident(slot))
        }
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::DecimalLit { .. }
        | ExprKind::DurationLit { .. }
        | ExprKind::StrLit(..)
        | ExprKind::InterpStr(..)
        | ExprKind::BoolLit(..)
        | ExprKind::Ident(..)
        | ExprKind::Call { .. }
        | ExprKind::Lambda(..)
        | ExprKind::BinOp(..)
        | ExprKind::UnaryOp(..)
        | ExprKind::Block(..)
        | ExprKind::If { .. }
        | ExprKind::Ok(..)
        | ExprKind::Err(..)
        | ExprKind::Question(..)
        | ExprKind::RecordConstruction { .. }
        | ExprKind::FieldAccess { .. }
        | ExprKind::MethodCall { .. }
        | ExprKind::Match { .. }
        | ExprKind::Is { .. }
        | ExprKind::Some(..)
        | ExprKind::None
        | ExprKind::UnitLit
        | ExprKind::RecordSpread { .. }
        | ExprKind::Expect(..)
        | ExprKind::Val { .. }
        | ExprKind::Wire(..)
        | ExprKind::ListLit(..)
        | ExprKind::Observation(..)
        | ExprKind::Faults(..)
        | ExprKind::Trace { .. } => lower_expr(e, cx),
    }
}

/// The slot annotation for a hoisted `if` in **async-tail** position, where
/// `checked_ty_node` alone is wrong.
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
fn async_tail_slot_ty(e: &Expr, cx: &LowerCtx) -> Option<bynk_ts::TsType> {
    use bynk_ts::TsType;
    let tys = cx.ty_render();
    match cx
        .commons()
        .expr_ty(e.id)
        .as_deref()
        .and_then(Ty::as_effect)
    {
        Some(inner) => {
            let inner_ty = if matches!(&*tys.get(inner), Ty::Unit) {
                TsType::named("void")
            } else {
                ts_ty_to_ts_type(inner, tys)
            };
            Some(TsType::union(vec![
                inner_ty.clone(),
                TsType::named_with_args("globalThis.Promise", vec![inner_ty]),
            ]))
        }
        None => checked_ty_node(e, cx),
    }
}

/// #1750: emit a value-position `if`, `match` or block (`e`) as a real
/// statement, for when its branches hold a `?`. The arrow (IIFE) these forms
/// otherwise lower to would capture the `?`'s `return`, so its `Err` became the
/// expression's value instead of leaving the function: the same miscompile
/// [`hoist_if_slot`] closes for a ternary-shaped `if`, which has only
/// branch tails to place. Here `emit` writes the form through its ordinary
/// statement-position emitter (`if_tail`, `emit_match_tail`,
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
    emit: impl FnOnce(&mut Body, &mut LowerCtx),
) -> Lowered {
    let slot = cx.fresh();
    let label = slot.clone() + "_out";
    let mut pre = Pre::new();
    let saved = cx.tail_slot.replace(TailSlot {
        slot: slot.clone(),
        label: label.clone(),
    });
    let mut body = Body::new();
    emit(&mut body, cx);
    cx.tail_slot = saved;
    // Typed for the same reason as `hoist_if_slot`'s slot: inference
    // gives up on a `let` whose assignments sit beside a `return`.
    //
    // Statement nodes (1A.35a part B): `let slot: T;` then the labelled block.
    let stmts = body.into_stmts();
    pre.push_node(bynk_ts::TsStmt::let_stmt(
        bynk_ts::TsBindingName::Ident(slot.clone()),
        checked_ty_node(e, cx),
        None,
        None,
    ));
    pre.push_node(bynk_ts::TsStmt::labelled(
        label,
        bynk_ts::TsStmt::block(stmts, None),
        None,
    ));
    pre.finish(bynk_ts::TsExpr::Ident(slot))
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
    // A `?` is found, a lambda is not entered (its `?` returns from the lambda), and every
    // other expression is searched through its children: the generic walk, not a decision per
    // construct, so it is tested for the two shapes rather than listing the rest.
    if matches!(e.kind, ExprKind::Question(_)) {
        true
    } else if matches!(e.kind, ExprKind::Lambda(_)) {
        false
    } else {
        bynk_syntax::ast::expr_children(e)
            .into_iter()
            .any(expr_contains_question)
    }
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
    match e.kind.branching() {
        Some(Branching::Match { .. }) => false,
        Some(Branching::If {
            cond,
            then_block,
            else_block,
        }) => ternary_shaped(cond, then_block, else_block, cx),
        None => true,
    }
}

/// Lower the awaited expression of a `let x <- e`, with the call-site
/// `by <Actor>(<identity>)` clause it carries folded into the addressed call.
fn lower_effect_let_value(
    l: &bynk_syntax::ast::LetStmt,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
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
        let kind = call_site_principal(cx, principal);
        if kind == CallSitePrincipal::Nobody {
            cx.call_site_no_credential = true;
        } else if let CallSitePrincipal::BadCredential(bad) = kind {
            // #2135: `by Forged` / `by Stale` name no identity either.
            cx.call_site_bad_signature = Some(crate::emitter::lower_call::test_calls::BadDriver {
                driver: bad.driver(),
                arm: bynk_check::actors::bad_principal_arm(principal).map(str::to_string),
            });
        } else {
            cx.call_site_identity = principal.identity.as_ref().map(|id| pre.lower_node(id, cx));
            // #3052, #3937: a `system` call mints its `Oidc` or `Bearer` token's claims from the
            // lines of a literal `TokenClaims.of(lines)` (the checker admits
            // no other form there, `bynk.test.system_claims_need_literal`),
            // and never builds the in-process `TokenClaims` value, so the
            // lines are lowered once and in place of it.
            let literal_lines = match principal.claims.as_deref().map(|c| &c.kind) {
                Some(ExprKind::MethodCall { args, .. })
                    if cx.is_system_claims_body()
                        && args.len() == 1
                        && matches!(
                            principal
                                .claims
                                .as_deref()
                                .and_then(|c| cx.commons().callee(c.id)),
                            Some(Callee::Intrinsic {
                                op: StaticOp::TokenClaims(TokenClaimsStatic::Of),
                                ..
                            })
                        ) =>
                {
                    Some(&args[0])
                }
                _ => None,
            };
            if let Some(lines) = literal_lines {
                cx.call_site_claim_lines = Some(pre.lower_node(lines, cx));
            } else {
                cx.call_site_claims = principal.claims.as_ref().map(|c| pre.lower_node(c, cx));
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
    // Each binding's hoisted statements, its value, its annotation, and whether
    // lowering it awaited (1A.14, Decision D of #3715).
    let mut parts: Vec<ParPart> = Vec::new();
    for st in &p.statements {
        let mut pre = Pre::new();
        // Each part has its own flag, as 1A.12 gave each IIFE, so a thunk is
        // `async` because *its* lowering awaited, not because its hoisted text
        // happens to contain `"await "` (a string literal can).
        let saved_await = std::mem::take(&mut cx.emitted_await);
        let (value, annot) = match st {
            Statement::EffectLet(l) => {
                let value = lower_effect_let_value(l, &mut pre, cx);
                names.push(Some(&l.name));
                (value, l.type_annot.as_ref())
            }
            Statement::Do(d) => {
                let value = pre.lower_node(&d.value, cx);
                names.push(None);
                (value, None)
            }
            // The checker refuses every other statement in a `par` block.
            Statement::Let(..)
            | Statement::Expect(..)
            | Statement::Assign(..)
            | Statement::Advance(..)
            | Statement::Par(..) => {
                cx.emitted_await = saved_await;
                continue;
            }
        };
        let asynchronous = std::mem::replace(&mut cx.emitted_await, saved_await);
        parts.push(ParPart {
            pre,
            value,
            annot,
            asynchronous,
        });
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
    // T2.2 (R6.4): see the matching note at `EffectLet`.
    cx.emitted_await = true;
    let hoisted = parts.iter().any(|part| !part.pre.is_empty());
    body.push(stmt_item(par_node(parts, &pattern, hoisted)));
}

/// One `par` binding: its hoisted statements, value, annotation, and whether
/// lowering it awaited.
struct ParPart<'a> {
    pre: Pre,
    value: bynk_ts::TsExpr,
    annot: Option<&'a bynk_syntax::ast::TypeRef>,
    asynchronous: bool,
}

/// `par`'s join as a statement node: `const [a, b] = await __bynkSettleAll([…]);`
/// over thunk arrows (an `await …;` with no binding when the pattern is empty). The array is one
/// item per line when any part hoisted, in the printer's own layout (the old text indented items
/// by one column, an odd offset that is normalised here: 1A.14).
fn par_node(parts: Vec<ParPart>, pattern: &[String], hoisted: bool) -> bynk_ts::TsStmt {
    let mut items = Vec::with_capacity(parts.len());
    for part in parts {
        let value = part.value;
        let return_type = part.annot.map(|a| {
            bynk_ts::TsType::named_with_args(
                "globalThis.Promise",
                vec![ts_type_ref_to_ts_type(a, None)],
            )
        });
        let arrow_body = if part.pre.is_empty() {
            bynk_ts::TsArrowBody::Expr(Box::new(value))
        } else {
            let mut stmts = part.pre.ts_stmts();
            stmts.push(bynk_ts::TsStmt::return_stmt(Some(value), None));
            bynk_ts::TsArrowBody::Multiline(stmts)
        };
        items.push(bynk_ts::TsExpr::Arrow {
            params: Vec::new(),
            is_async: part.asynchronous,
            generics: Vec::new(),
            return_type,
            body: Box::new(arrow_body),
        });
    }
    let join = bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident("__bynkSettleAll".to_string())),
        args: vec![if hoisted {
            bynk_ts::TsExpr::LinesArray(items)
        } else {
            bynk_ts::TsExpr::array(items)
        }],
    }));
    if pattern.is_empty() {
        bynk_ts::TsStmt::expr_stmt(join, None)
    } else {
        bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::ArrayPattern(pattern.to_vec()),
            None,
            join,
            None,
        )
    }
}

fn statement(body: &mut Body, stmt: &Statement, cx: &mut LowerCtx) {
    // Slice 1 (ADR 0103 D2): every generated line this statement emits — including
    // the multi-line `?` expansion (temp / Err-guard / unwrap) — anchors to the
    // statement's source span, so a source-map-aware stepper coalesces them into
    // one source step (the slice-0 spike confirmed this).
    body.push(Item::Mark(stmt.span()));
    match stmt {
        Statement::Let(l) => {
            let mut pre = Pre::new();
            let value = pre.lower_node(&l.value, cx);
            body.extend(pre.items());
            // #908: also covers `_` (a fresh throwaway local; TS allows
            // `const _ = ...` only once per scope) and a re-`let` of a name
            // already bound in this or an enclosing block (which otherwise
            // either collides with the first `const` of that name — TS2451 —
            // or, one block deeper, shadows it into a TDZ read).
            let bind_name = cx.bind_local_name(&l.name.name);
            body.push(const_item(&bind_name, l.type_annot.as_ref(), value, false));
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
            body.push(const_item(&bind_name, l.type_annot.as_ref(), value, true));
        }
        Statement::Expect(a) if a.snapshot.is_some() => {
            // #2842: `expect <value> matches snapshot "<name>"` compares the
            // value's `Json.encode` text with the snapshot the module inlined
            // (`__bynkSnapshotCheck`, `test_runtime/snapshot.ts`).
            let clause = a.snapshot.as_ref().expect("guarded by is_some");
            let span_start = a.span.start;
            let span_end = a.span.end;
            let location = expect_location_raw(cx, span_start);
            let src = expect_source_text_raw(cx, a.value.span);
            let mut pre = Pre::new();
            let json = json_encode(&mut pre, &a.value, cx, false);
            body.extend(pre.items());
            match json {
                // The snapshot's name is a string-literal node, spelled as the printer
                // spells every string.
                Some(json) => body.push(stmt_item(snapshot_check(
                    str_lit(&clause.name),
                    json,
                    &location,
                    (span_start, span_end),
                    &src,
                    &clause.name,
                ))),
                // The checker typed the value; a missing type is drift, said at run time.
                None => {
                    let message = format!(
                        "expect {src} matches snapshot \"{}\"\n  the value's type is not known to the snapshot",
                        clause.name
                    );
                    body.push(stmt_item(bynk_ts::TsStmt::throw_stmt(
                        expect_failure_call(&location, span_start, span_end, str_lit(message)),
                        None,
                    )));
                }
            }
        }
        Statement::Expect(a) => {
            // Inside a test case body, `expect <pred>` lowers to a runtime check
            // that throws an ExpectationError so the surrounding test-case runner
            // catches it and records the failure (v0.112, renamed from `assert`).
            let span_start = a.value.span.start;
            let span_end = a.value.span.end;
            let location = expect_location_raw(cx, span_start);
            let src = expect_source_text_raw(cx, a.value.span);
            let mut pre = Pre::new();
            let mut was = None;
            // Structural expected-vs-actual for a top-level comparison. Each
            // operand is lowered once and both the condition and the failure
            // detail read it. When an operand hoists statements (a `?` guard),
            // both operands are bound to temporaries after the hoists, so
            // nothing in the operand runs twice: lowering the operands a second
            // time for the detail would hoist a second copy, which runs before
            // the `if` on every `expect`, pass or fail (#3768).
            let (cond, detail) = if let ExprKind::BinOp(op, l, r) = &a.value.kind
                && let Some(sym) = comparison_op_symbol(*op)
            {
                let before = pre.len();
                let mut lv = pre.lower_node(l, cx);
                let mut rv = pre.lower_node(r, cx);
                if pre.len() != before {
                    lv = pre.bind_temp(lv, cx);
                    rv = pre.bind_temp(rv, cx);
                }
                let cond = build_value_bin_op(*op, l, vec![lv.clone(), rv.clone()], cx);
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
                (
                    cond,
                    ExpectDetail::Compare {
                        show,
                        sym,
                        operands: Box::new((lv, rv)),
                    },
                )
            } else if matches!(a.value.kind, ExprKind::Faults(_)) {
                // #1706: the claim's only failure is a call that returned.
                (
                    pre.lower_node(&a.value, cx),
                    ExpectDetail::Message(format!(
                        "expect {src}\n  the call returned without faulting"
                    )),
                )
            } else if let Some(r) = super::lower::expect_was::subject_report(&a.value, cx) {
                // #3980: a failed `is` reports the subject's value.
                let head = ["expect ", src.as_str()].concat();
                let text = r.detail(&head);
                let c = pre.lower_node(&a.value, cx);
                was = Some((r, head));
                (c, ExpectDetail::Message(text))
            } else {
                (
                    pre.lower_node(&a.value, cx),
                    ExpectDetail::Message(format!("expect {src}")),
                )
            };
            body.extend(pre.items());
            match was {
                Some((r, head)) if !r.runtime_args().is_empty() => {
                    body.push(stmt_item(super::lower::expect_was::guard(
                        cond,
                        &location,
                        (span_start, span_end),
                        &head,
                        &r,
                    )));
                }
                _ => body.push(expect_item(
                    cond,
                    detail,
                    &location,
                    &src,
                    (span_start, span_end),
                )),
            }
            // #3981: the names a statement-form `is` binds stay in scope to the end of the block.
            if cx.expect_is_bindings {
                body.extend(
                    super::lower::expect_bindings::declarations(&a.value, cx)
                        .into_iter()
                        .map(stmt_item),
                );
            }
        }
        Statement::Advance(a) => {
            // #2137: move the case's harness clock (`testgen/test_runtime/clock.ts`),
            // which every participant's `Clock` reads at `system` and the
            // `Clock` double reads below it. A negative amount faults there.
            let mut pre = Pre::new();
            let amount = pre.lower_node(&a.amount, cx);
            body.extend(pre.items());
            body.push(stmt_item(bynk_ts::TsStmt::expr_stmt(
                bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Ident("__bynkAdvanceClock".to_string())),
                    args: vec![amount],
                },
                None,
            )));
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
            body.push(do_item(value));
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
            body.push(assign_item(target, value));
        }
    }
}

/// `__bynkExpectFailure(<location>, <start>, <end>, <detail>)`.
fn expect_failure_call(
    location: &str,
    start: usize,
    end: usize,
    detail: bynk_ts::TsExpr,
) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident("__bynkExpectFailure".to_string())),
        args: vec![
            str_lit(location),
            num_lit(start.to_string()),
            num_lit(end.to_string()),
            detail,
        ],
    }
}

/// The failure message of an `expect <pred>` statement.
enum ExpectDetail {
    /// A top-level comparison: both operands, shown through `show`.
    Compare {
        show: &'static str,
        sym: &'static str,
        /// The left and right operands (boxed: a lowered expression is large).
        operands: Box<(bynk_ts::TsExpr, bynk_ts::TsExpr)>,
    },
    /// A fixed message (the unescaped text).
    Message(String),
}

/// `if (!(<cond>)) { throw __bynkExpectFailure(…); }` as an item (1A.17, Decision C): an `If`
/// over an `InlineBlock`. `location` and `src` are unescaped: the printer escapes them.
fn expect_item(
    cond: bynk_ts::TsExpr,
    detail: ExpectDetail,
    location: &str,
    src: &str,
    (start, end): (usize, usize),
) -> Item {
    use bynk_ts::{TsBinaryOp, TsExpr};
    let compare_head = format!("expect {src}\n  expected: {src}\n  actual:   ");
    let detail = match detail {
        ExpectDetail::Compare {
            show,
            sym,
            operands,
        } => {
            let (lv, rv) = *operands;
            let shown = |v: TsExpr| TsExpr::Call {
                callee: Box::new(TsExpr::Ident(show.to_string())),
                args: vec![TsExpr::Paren(Box::new(v))],
            };
            binary(
                TsBinaryOp::Add,
                binary(
                    TsBinaryOp::Add,
                    binary(TsBinaryOp::Add, str_lit(compare_head), shown(lv)),
                    str_lit(format!(" {sym} ")),
                ),
                shown(rv),
            )
        }
        ExpectDetail::Message(m) => str_lit(m),
    };
    expect_if(cond, location, start, end, detail)
}

/// `{ const __d = __bynkSnapshotCheck(name, json); if (__d !== undefined) { throw
/// __bynkExpectFailure(…); } }`: the `expect <value> matches snapshot` check over nodes.
fn snapshot_check(
    name: bynk_ts::TsExpr,
    json: bynk_ts::TsExpr,
    location: &str,
    (start, end): (usize, usize),
    src: &str,
    snapshot: &str,
) -> bynk_ts::TsStmt {
    use bynk_ts::{TsBinaryOp, TsBindingName, TsExpr, TsStmt};
    let message = TsExpr::Binary {
        op: TsBinaryOp::Add,
        left: Box::new(str_lit(format!(
            "expect {src} matches snapshot \"{snapshot}\"\n  "
        ))),
        right: Box::new(ident("__d")),
    };
    TsStmt::inline_block(
        vec![
            TsStmt::const_stmt(
                TsBindingName::Ident("__d".to_string()),
                None,
                TsExpr::Call {
                    callee: Box::new(ident("__bynkSnapshotCheck")),
                    args: vec![name, json],
                },
                None,
            ),
            TsStmt::if_stmt(
                TsExpr::Binary {
                    op: TsBinaryOp::StrictNotEq,
                    left: Box::new(ident("__d")),
                    right: Box::new(ident("undefined")),
                },
                TsStmt::inline_block(
                    vec![TsStmt::throw_stmt(
                        expect_failure_call(location, start, end, message),
                        None,
                    )],
                    None,
                ),
                None,
            ),
        ],
        None,
    )
}

/// `if (!(<cond>)) { throw __bynkExpectFailure(…); }` over nodes.
fn expect_if(
    cond: bynk_ts::TsExpr,
    location: &str,
    start: usize,
    end: usize,
    detail: bynk_ts::TsExpr,
) -> Item {
    use bynk_ts::{TsExpr, TsStmt, TsUnaryOp};
    let throw = TsStmt::throw_stmt(expect_failure_call(location, start, end, detail), None);
    stmt_item(TsStmt::if_stmt(
        TsExpr::Unary {
            op: TsUnaryOp::Not,
            expr: Box::new(TsExpr::Paren(Box::new(cond))),
        },
        TsStmt::inline_block(vec![throw], None),
        None,
    ))
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
        BinOp::Implies
        | BinOp::Or
        | BinOp::And
        | BinOp::Add
        | BinOp::Sub
        | BinOp::Mul
        | BinOp::Div => None,
    }
}

#[cfg(test)]
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
            .any(|t| {
                // One shape is asked for: an `is` test, whose pattern has bindings.
                if let ExprKind::Is { value, pattern } = &t.kind {
                    pattern_is_has_bindings(pattern, value, cx)
                } else {
                    false
                }
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
        Pattern::Wildcard(..)
        | Pattern::Binding(..)
        | Pattern::Literal { .. }
        | Pattern::Range(..)
        | Pattern::Refined { .. } => false,
    }
}

pub(super) fn if_tail(
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
    then_body.extend(is_bindings.iter().map(super::lower::stmt_item));
    emit_block_inner(&mut then_body, then_block, cx, async_tail);
    cx.shadow_scopes.pop();
    // #1654: the else-branch's bindings, those the condition's falsity proves.
    cx.shadow_scopes.push(HashMap::new());
    let mut else_bindings = Vec::new();
    gather_is_bindings_for_emit(cond, false, cx, &mut else_bindings, &mut found);
    let mut else_body = Body::new();
    else_body.extend(else_bindings.iter().map(super::lower::stmt_item));
    emit_block_inner(&mut else_body, else_block, cx, async_tail);
    cx.shadow_scopes.pop();
    push_if(body, cond_expr, then_body, else_body);
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
    match e.kind.chain_link() {
        Some(ChainLink::Root(_)) => true,
        Some(ChainLink::Field { receiver, .. } | ChainLink::Paren(receiver)) => {
            is_narrowable_path(receiver)
        }
        None => false,
    }
}

/// `throw new globalThis.Error("non-exhaustive match");`.
fn non_exhaustive_throw() -> Item {
    stmt_item(bynk_ts::TsStmt::throw_stmt(
        bynk_ts::TsExpr::New {
            callee: Box::new(member_node("globalThis", "Error")),
            args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                "non-exhaustive match".to_string(),
            ))],
        },
        None,
    ))
}

pub(super) fn match_tail(
    body: &mut Body,
    discriminant: &Expr,
    arms: &[MatchArm],
    cx: &mut LowerCtx,
    async_tail: bool,
) {
    // Anchor the discriminant lowering + `switch (…) {` header to the match's
    // scrutinee span (slice 1); each arm re-anchors to its own span below.
    body.push(Item::Mark(discriminant.span));
    let mut pre = Pre::new();
    let disc = pre.lower_node(discriminant, cx);
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
    let path = if !is_narrowable_path(discriminant) || cx.roots_at_const(discriminant) {
        let tmp = cx.fresh();
        body.push(const_item(&tmp, None, disc, false));
        PatPath::ident(&tmp)
    } else {
        PatPath::from_lowered(disc)
    };
    // #3980: an `expect false` arm names its arm and the value matched.
    let framed = {
        let frame = super::lower::expect_was::scrutinee_frame(discriminant, path.node(), arms, cx);
        cx.push_scrutinee(frame)
    };
    match_dispatch(body, &path, &disc_ty, arms, cx, async_tail);
    if framed {
        cx.pop_scrutinee();
    }
}

/// The `switch` or `if` chain over `arms`, then the non-exhaustive `throw`:
/// what follows the discriminant in a tail `match` and fills a value-position
/// `match`'s arrow.
pub(super) fn match_dispatch(
    body: &mut Body,
    path: &PatPath,
    disc_ty: &Option<TyId>,
    arms: &[MatchArm],
    cx: &mut LowerCtx,
    async_tail: bool,
) {
    let tys = cx.commons().tys();
    // ADR 0169: nested/guarded matches lower to an if-chain; flat/unguarded
    // matches keep the `switch`.
    if bynk_model::match_needs_if_chain(arms) {
        match_if_chain(body, path, disc_ty, arms, cx, async_tail);
        return;
    }
    // v0.130: literal-kind matches switch on the value; variant-kind on `.tag`.
    let scrutinee = if is_literal_match(disc_ty, tys) {
        path.clone()
    } else {
        path.member("tag")
    };
    let cases: Vec<MatchCase> = arms
        .iter()
        .map(|arm| match_case(path, disc_ty, arm, cx, async_tail))
        .collect();
    let cases = cases.into_iter().map(MatchCase::into_node).collect();
    body.push(stmt_item(bynk_ts::TsStmt::switch_stmt(
        scrutinee.node().clone(),
        cases,
        None,
    )));
    body.push(non_exhaustive_throw());
}

/// One lowered `switch` arm: its labels (none is `default`), the statements
/// inside its braces, and the arm's span.
struct MatchCase {
    labels: Vec<bynk_ts::TsExpr>,
    inner: Body,
    span: Span,
}

impl MatchCase {
    /// The arm as a typed case. Its span is carried on the case only when checkpoints are
    /// recorded.
    fn into_node(self) -> bynk_ts::TsSwitchCase {
        bynk_ts::TsSwitchCase {
            default_braced: self.labels.is_empty(),
            labels: self.labels,
            body: self.inner.into_stmts(),
            case_braced: true,
            span: Some(self.span),
        }
    }
}

fn match_case(
    disc: &PatPath,
    disc_ty: &Option<TyId>,
    arm: &MatchArm,
    cx: &mut LowerCtx,
    async_tail: bool,
) -> MatchCase {
    let tys = cx.commons().tys();
    // #908: this arm's payload/catch-all bindings live in the `case`'s own JS
    // block, so a read inside the body must resolve to them rather than an
    // outer `let` rename — push a frame and register each below, popped once
    // the whole arm (bindings + body) is emitted.
    cx.shadow_scopes.push(HashMap::new());
    let mut labels = Vec::new();
    let mut inner = Body::new();
    match &arm.pattern {
        Pattern::Wildcard(_) => {
            match_body(&mut inner, &arm.body, cx, async_tail);
        }
        // A bare name binding is a catch-all that binds the scrutinee (ADR 0169).
        Pattern::Binding(id) => {
            let local = cx.bind_local_name(&id.name);
            inner.push(const_node_item(&local, None, disc.node().clone(), false));
            match_body(&mut inner, &arm.body, cx, async_tail);
        }
        // v0.130: a literal pattern lowers to `case <literal>:`. JS `switch`
        // compares with `===`, matching the value semantics we want for
        // `Int`/`String`/`Bool`.
        Pattern::Literal { value, .. } => {
            labels.push(literal_label_node(value));
            match_body(&mut inner, &arm.body, cx, async_tail);
        }
        // #472: a refined arm always trips `match_needs_if_chain`, so the
        // switch path (this function) never sees one in practice — a
        // predicate has no `case`-label lowering, unlike a literal or a tag.
        Pattern::Refined { .. } => {
            unreachable!("a Pattern::Refined arm always routes through match_if_chain")
        }
        // #2641: a record has no tag to switch on, so a record pattern always
        // trips `match_needs_if_chain`.
        Pattern::Record { .. } => {
            unreachable!("a Pattern::Record arm always routes through match_if_chain")
        }
        Pattern::List { .. } => {
            unreachable!("a Pattern::List arm always routes through match_if_chain")
        }
        Pattern::Range(_) => {
            unreachable!("a Pattern::Range arm always routes through match_if_chain")
        }
        Pattern::Variant {
            variant, bindings, ..
        } => {
            labels.push(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                variant.name.clone(),
            )));
            // The switch path only handles flat, unguarded arms (nested/guarded
            // matches take the if-chain path), so each payload binding is an
            // irrefutable name or `_`.
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
                inner.push(const_node_item(
                    &local,
                    None,
                    disc.member(&field).node().clone(),
                    false,
                ));
            }
            match_body(&mut inner, &arm.body, cx, async_tail);
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
        Pattern::Or(_, _) if arm.pattern.is_irrefutable() => {
            match_body(&mut inner, &arm.body, cx, async_tail);
        }
        Pattern::Or(alts, _) => {
            for alt in alts {
                labels.push(match alt {
                    Pattern::Literal { value, .. } => literal_label_node(value),
                    Pattern::Variant { variant, .. } => {
                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(variant.name.clone()))
                    }
                    Pattern::Wildcard(_) | Pattern::Binding(_) => unreachable!(
                        "pattern.is_irrefutable() is false, so no alternative is a wildcard or binding"
                    ),
                    Pattern::Or(..) => unreachable!("an Or's alternatives are always leaves"),
                    Pattern::Record { .. } => {
                        unreachable!("a record alternative routes through match_if_chain")
                    }
                    Pattern::List { .. } => {
                        unreachable!("a list alternative routes through match_if_chain")
                    }
                    Pattern::Range(_) => {
                        unreachable!("a range alternative routes through match_if_chain")
                    }
                    // #472/#474: the parser only ever wraps a *whole*,
                    // already-folded `|`-chain in `Refined` (never the other
                    // way — see `parse_pattern_where_suffix`), so an `Or`'s
                    // alternatives (built from `parse_pattern_base`) can never
                    // themselves be `Refined`.
                    Pattern::Refined { .. } => {
                        unreachable!("an Or's alternatives are never a Refined pattern")
                    }
                });
            }
            match_body(&mut inner, &arm.body, cx, async_tail);
        }
    }
    cx.shadow_scopes.pop();
    MatchCase {
        labels,
        inner,
        span: arm.span,
    }
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
                match e.kind.branching() {
                    Some(Branching::Match { discriminant, arms }) => {
                        match_tail(body, discriminant, arms, cx, async_tail);
                        return;
                    }
                    Some(Branching::If {
                        cond,
                        then_block,
                        else_block,
                    }) => {
                        if_tail(body, cond, then_block, else_block, cx, async_tail);
                        return;
                    }
                    None => {}
                }
            }
            let tail = lower_tail_expr(e, cx, async_tail);
            body.extend(tail.pre_items());
            push_tail(body, cx, tail.expr);
        }
        MatchBody::Block(b) => emit_block_inner(body, b, cx, async_tail),
    }
}

/// `if (cond) { then }` as one `If` node over a body of statements (which keeps its marks).
fn push_if_then(body: &mut Body, cond: bynk_ts::TsExpr, then: Body) {
    body.push(stmt_item(bynk_ts::TsStmt::if_stmt(
        cond,
        bynk_ts::TsStmt::block(then.into_stmts(), None),
        None,
    )));
}

/// Lower a nested/guarded match to a sequence of independent `if` blocks
/// (ADR 0169, DECISION E). Each arm tests its structural pattern; on a match it
/// binds names, then (if guarded) tests the guard, then runs its body — whose
/// tail `return` short-circuits the remaining arms. A guard failing falls
/// through to the next arm, which a `switch` cannot express. Per-arm span
/// anchoring is preserved (ADR 0103).
fn match_if_chain(
    body: &mut Body,
    path: &PatPath,
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
        // reasoning in `match_case`. Pushed/popped once per arm
        // regardless of `has_tests` below, since the concern is which
        // binding a read resolves to, not whether the arm also happens to
        // emit a wrapping JS block.
        cx.shadow_scopes.push(HashMap::new());
        let mut tests = Vec::new();
        pattern_tests(path, *disc_ty, &arm.pattern, cx, &mut tests);
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
        let opens_block = has_tests || needs_block;
        // The arm's own lines sit one level in when it opened a block (`if (…) {`
        // or `{`), and at this level otherwise.
        let mut nested = Body::new();
        {
            let arm_body: &mut Body = if opens_block { &mut nested } else { &mut *body };
            emit_pattern_bindings(arm_body, path, *disc_ty, &arm.pattern, cx);
            if let Some(guard) = &arm.guard {
                let guard_lowered = lower_expr(guard, cx);
                arm_body.extend(guard_lowered.pre_items());
                let mut guarded = Body::new();
                match_body(&mut guarded, &arm.body, cx, async_tail);
                push_if_then(arm_body, guard_lowered.expr, guarded);
            } else {
                match_body(arm_body, &arm.body, cx, async_tail);
            }
        }
        if has_tests {
            push_if_then(body, and_join(tests), nested);
        } else if needs_block {
            body.push(stmt_item(bynk_ts::TsStmt::block(nested.into_stmts(), None)));
        }
        cx.shadow_scopes.pop();
    }
    if !has_catchall {
        body.push(non_exhaustive_throw());
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
            TypeRef::Effect(..)
            | TypeRef::HttpResult(..)
            | TypeRef::QueueResult(..)
            | TypeRef::Asset(..)
            | TypeRef::RequestHeaders(..)
            | TypeRef::RequestQuery(..)
            | TypeRef::RequestCookies(..)
            | TypeRef::TokenClaims(..)
            | TypeRef::BoundaryRejection(..)
            | TypeRef::Query(..)
            | TypeRef::Stream(..)
            | TypeRef::Connection(..)
            | TypeRef::History(..)
            | TypeRef::ValidationError(..)
            | TypeRef::DecodeError(..)
            | TypeRef::Unit(..)
            | TypeRef::Fn(..) => false,
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
            Ty::Error
            | Ty::Never
            | Ty::Effect(..)
            | Ty::HttpResult(..)
            | Ty::QueueResult
            | Ty::HttpOutcome(..)
            | Ty::BoundaryRejection
            | Ty::Asset
            | Ty::RequestHeaders
            | Ty::RequestQuery
            | Ty::RequestCookies
            | Ty::TokenClaims
            | Ty::Query(..)
            | Ty::Stream(..)
            | Ty::Connection(..)
            | Ty::TestConnection(..)
            | Ty::ValidationError
            | Ty::DecodeError
            | Ty::Unit
            | Ty::Actor(..)
            | Ty::ActorSum(..)
            | Ty::Fn { .. }
            | Ty::Var(..) => false,
        }
    }
    by_ty(ty, cx, &mut HashSet::new())
}

#[cfg(test)]
mod body_tests {
    use super::*;
    use crate::emitter::ctx::{BodyMode, ModuleCtx};
    use crate::ts_build::ident;
    use bynk_check::checker::TypedCommons;
    use bynk_ts::SourceMapBuilder;
    use std::cell::RefCell;

    /// Run `f` with a `LowerCtx` that records into `smb`.
    fn with_cx<R>(smb: &RefCell<SourceMapBuilder>, f: impl FnOnce(&LowerCtx) -> R) -> R {
        let commons = TypedCommons::empty();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(
                &commons,
                &cross_context,
                &runtime_use,
                UnitFacts::detached(),
            ),
            BodyMode::Method,
        )
        .with_source_map(Some(smb));
        f(&cx)
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

    /// `const <name> = 1;`
    fn one(name: &str) -> Item {
        node(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident(name.to_string()),
            None,
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("1".to_string())),
            None,
        ))
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

    /// A `Mark` records the offset where the next item starts.
    #[test]
    fn a_mark_records_the_offset_where_it_is_reached() {
        let mut body = Body::new();
        body.push(Item::Mark(Span::new(0, 10)));
        body.push(one("a"));
        body.push(Item::Mark(Span::new(10, 20)));
        body.push(one("b"));
        let (out, got) = rendered(&body, 0);
        let mut want = SourceMapBuilder::new();
        want.add_source("t.bynk", "x".repeat(200));
        want.record(0, Span::new(0, 10));
        want.record("const a = 1;\n".len(), Span::new(10, 20));
        assert_eq!(out, "const a = 1;\nconst b = 1;\n");
        assert_eq!(got.to_v3(&out, "t.js"), want.to_v3(&out, "t.js"));
    }

    /// The claim-scope wrapper (1A.15, Decision C of #3716) as nodes: the
    /// declaration, `let __r;`, the `try` over an async closure holding the
    /// body, the `catch` that releases the claims, the confirmation and the
    /// result. The body's checkpoints survive on its statements.
    #[test]
    fn the_claim_scope_wrapper_is_nodes_around_the_body() {
        let mut inner = Body::new();
        inner.push(Item::Mark(Span::new(0, 10)));
        inner.push(one("a"));
        inner.push(Item::Mark(Span::new(10, 30)));
        inner.push(Item::Node(Box::new(if_node())));
        let stmt = bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Function {
                name: "f".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: None,
                body: claim_scoped(inner).into_stmts(),
                is_async: false,
                inline: false,
            },
            None,
        );
        let mut out = String::new();
        let mut map = SourceMapBuilder::new();
        map.add_source("t.bynk", "x".repeat(200));
        bynk_ts::print_stmt_and_merge(&mut out, &stmt, 0, &mut map, 0);
        let declaration = format!(
            "function f() {{\n  {}\n",
            crate::emitter::lower::CLAIM_SCOPE_DECLARATION
        );
        let rest = out.strip_prefix(&declaration).expect("declaration first");
        assert_eq!(
            rest,
            "  let __r;\n  try {\n    __r = await (async () => {\n      const a = 1;\n      if (flag) {\n        done;\n      }\n    })();\n  } catch (__e) {\n    await __claims.settle(undefined, true);\n    throw __e;\n  }\n  await __claims.settle(__r, false);\n  return __r;\n}\n"
        );
        // Both checkpoints land on the statements they were put on.
        let mut want = SourceMapBuilder::new();
        want.add_source("t.bynk", "x".repeat(200));
        want.record(out.find("      const a = 1;").unwrap(), Span::new(0, 10));
        want.record(out.find("      if (flag)").unwrap(), Span::new(10, 30));
        assert_eq!(map.to_v3(&out, "t.js"), want.to_v3(&out, "t.js"));
    }

    /// `into_stmts` (1A.15, Decision A): the statements of a function body print
    /// to the text `render` writes at the same indent, and the printer records
    /// the checkpoints `render` records, for a body mixing marks and nodes.
    #[test]
    fn into_stmts_prints_what_render_writes_with_the_same_checkpoints() {
        let build = || {
            let mut body = Body::new();
            body.push(Item::Mark(Span::new(0, 10)));
            body.push(one("a"));
            body.push(Item::Mark(Span::new(10, 30)));
            body.push(Item::Node(Box::new(if_node())));
            body.push(Item::Mark(Span::new(30, 60)));
            body.push(Item::Node(Box::new(if_node())));
            body
        };
        for indent in [2usize, 4] {
            let (text, want) = rendered(&build(), indent);
            let method = bynk_ts::TsClassMethod {
                name: "m".to_string(),
                private: false,
                is_async: false,
                params: Vec::new(),
                return_type: None,
                doc: None,
                body: build().into_stmts(),
                span: None,
            };
            // A method's body sits two steps in, so only indent 4 is a method.
            if indent != 4 {
                let stmt = bynk_ts::TsStmt::decl(
                    bynk_ts::TsDecl::Function {
                        name: "f".to_string(),
                        generics: Vec::new(),
                        params: Vec::new(),
                        return_type: None,
                        body: build().into_stmts(),
                        is_async: false,
                        inline: false,
                    },
                    None,
                );
                let mut out = String::new();
                let mut map = SourceMapBuilder::new();
                map.add_source("t.bynk", "x".repeat(200));
                bynk_ts::print_stmt_and_merge(&mut out, &stmt, 0, &mut map, 0);
                let body_text = out
                    .strip_prefix("function f() {\n")
                    .and_then(|o| o.strip_suffix("}\n"))
                    .expect("a function");
                assert_eq!(body_text, text, "indent {indent}");
                // Compare the checkpoints line for line, shifted by the header.
                let mut shifted = SourceMapBuilder::new();
                shifted.add_source("t.bynk", "x".repeat(200));
                shifted.merge(&want, &text, &out, "function f() {\n".len(), 0);
                assert_eq!(map.to_v3(&out, "t.js"), shifted.to_v3(&out, "t.js"));
            } else {
                let mut out = String::from("class C {\n");
                let mut map = SourceMapBuilder::new();
                map.add_source("t.bynk", "x".repeat(200));
                bynk_ts::print_class_method_and_merge(&mut out, &method, 0, &mut map, 0);
                let header = "class C {\n  m() {\n";
                let body_text = out
                    .strip_prefix(header)
                    .and_then(|o| o.strip_suffix("  }\n"))
                    .expect("a method");
                assert_eq!(body_text, text, "indent {indent}");
                let mut shifted = SourceMapBuilder::new();
                shifted.add_source("t.bynk", "x".repeat(200));
                shifted.merge(&want, &text, &out, header.len(), 0);
                assert_eq!(map.to_v3(&out, "t.js"), shifted.to_v3(&out, "t.js"));
            }
        }
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

    /// `into_stmts` hands each mark to the next statement only.
    #[test]
    fn into_stmts_attaches_a_mark_to_the_next_statement_only() {
        let mut body = Body::new();
        body.push(Item::Mark(Span::new(1, 2)));
        body.push(konst("a", "x"));
        body.push(konst("b", "y"));
        body.push(Item::Mark(Span::new(3, 4)));
        body.push(Item::Mark(Span::new(5, 6)));
        body.push(ret("a"));
        let stmts = body.into_stmts();
        let spans: Vec<Option<Span>> = stmts.iter().map(|s| s.span).collect();
        // Two marks in a row sit at one offset: the later one wins, as
        // `SourceMapBuilder::record` has it.
        assert_eq!(spans, [Some(Span::new(1, 2)), None, Some(Span::new(5, 6))]);
    }

    /// An `if` built from bodies that carry marks renders, with its map, as the
    /// statement the printer lays out, at every indent: the checkpoints land at
    /// the start of the line each was put on.
    #[test]
    fn an_if_node_renders_with_its_checkpoints() {
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
            let mut body = Body::new();
            body.push(Item::Mark(Span::new(0, 10)));
            push_if(&mut body, ident("k"), then_body(), else_body());
            assert!(matches!(body.0.last(), Some(Item::Node(_))));
            let (text, got) = rendered(&body, indent);
            let pad = |n: usize| " ".repeat(indent + n);
            let want_text = format!(
                "{p0}if (k) {{\n{p2}const a = x;\n{p2}const b = y;\n{p2}return a;\n{p0}}} else {{\n{p2}return c;\n{p0}}}\n",
                p0 = pad(0),
                p2 = pad(2)
            );
            assert_eq!(text, want_text, "indent {indent}");
            let mut want = SourceMapBuilder::new();
            want.add_source("t.bynk", "x".repeat(200));
            want.record(0, Span::new(0, 10));
            want.record(
                text.find(&format!("{}const a", pad(2))).unwrap(),
                Span::new(10, 20),
            );
            want.record(
                text.find(&format!("{}return a", pad(2))).unwrap(),
                Span::new(20, 30),
            );
            want.record(
                text.find(&format!("{}return c", pad(2))).unwrap(),
                Span::new(40, 50),
            );
            assert_eq!(
                got.to_v3(&text, "t.js"),
                want.to_v3(&text, "t.js"),
                "indent {indent}"
            );
        }
    }

    /// `do` and assignment are nodes (they are reached from an agent handler, which the lib's own
    /// unit tests cannot compile in-process).
    #[test]
    fn do_and_assignment_are_nodes() {
        let printed = |item: &Item| match item {
            Item::Node(stmt) => bynk_ts::print_stmt(stmt, 0),
            _ => unreachable!("a statement node"),
        };
        let call = || bynk_ts::TsExpr::Call {
            callee: Box::new(ident("f")),
            args: vec![ident("x")],
        };
        assert_eq!(printed(&do_item(call())), "await f(x);\n");
        let member = || member_node("__state", "count");
        assert_eq!(
            printed(&assign_item(member(), call())),
            "__state.count = f(x);\n"
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

/// 1A.11 (#3712): the statement producers build nodes.
#[cfg(test)]
mod stmt_node_tests {
    /// The emitted TypeScript for a single commons.
    fn emitted(src: &str) -> String {
        crate::testkit::emit_source(src)
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
        let ts = emitted(&src);
        assert_eq!(
            body_of(&ts, "pure"),
            "  const y = x + 1;\n  return y * 2;\n"
        );
        assert_eq!(
            body_of(&ts, "eff"),
            "  const a = await bump(x);\n  await bump(a);\n  return a;\n"
        );
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
        let ts = emitted(src);
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
    fn a_ternary_tail_is_a_return_of_a_parenthesised_conditional() {
        let src = "commons tern\n\n\
            fn f(c: Bool, x: Int, y: Int) -> Int {\n  if c { x } else { y }\n}\n";
        let ts = emitted(src);
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
        let ts = emitted(src);
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
        let ts = emitted(src);
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
        let ts = emitted(src);
        assert_eq!(
            body_of(&ts, "f"),
            "  if (a) {\n    const __r0 = r;\n    if (__r0.tag === \"Err\") return __r0;\n    const x = __r0.value;\n    if (b) {\n      const __r1 = r;\n      if (__r1.tag === \"Err\") return __r1;\n      const y = __r1.value;\n      return Ok(x + y);\n    } else {\n      const __r2 = r;\n      if (__r2.tag === \"Err\") return __r2;\n      const z = __r2.value;\n      return Ok(x + z);\n    }\n  } else {\n    const __r3 = r;\n    if (__r3.tag === \"Err\") return __r3;\n    const w = __r3.value;\n    return Ok(w);\n  }\n"
        );
    }

    #[test]
    fn an_if_tail_with_an_is_binding_is_a_node() {
        // The `is`-binding declaration is a node (the variant payload is read through
        // the `Extract` cast), so the `if` is a node too.
        let src = "commons isb\n\n\
            fn f(o: Option[Int]) -> Int {\n  if o is Some(v) {\n    let k = v + 1\n    k\n  } else {\n    0\n  }\n}\n";
        let ts = emitted(src);
        assert_eq!(
            body_of(&ts, "f"),
            "  if (((__c: boolean) => __c)(o.tag === \"Some\")) {\n    const v = (o as Extract<typeof o, { tag: \"Some\" }>).value;\n    const k = v + 1;\n    return k;\n  } else {\n    return 0;\n  }\n"
        );
    }

    #[test]
    fn the_tail_slot_form_is_an_assignment_and_a_labelled_break() {
        // #1750: `slot = value; break label;` is two statement nodes (1A.35a part B), one per
        // line, inside the labelled block.
        let src = "commons slot\n\n\
            fn f(a: Option[Int], r: Result[Int, String]) -> Result[Int, String] {\n  \
            let n = if a is Some(v) { r? + v } else { 0 }\n  Ok(n)\n}\n";
        let ts = emitted(src);
        let body = body_of(&ts, "f");
        assert!(
            body.contains("      __r0 = __r1.value + v;\n      break __r0_out;\n"),
            "{body}"
        );
        assert!(
            body.contains("      __r0 = 0;\n      break __r0_out;\n"),
            "{body}"
        );
    }
}

/// 1A.12 (#3713): value-position `if`, block and lambda bodies are arrow nodes
/// printed at their real depth, with today's text when a part is text.
#[cfg(test)]
mod arrow_node_tests {
    /// The emitted TypeScript for a single commons.
    fn emitted(src: &str) -> String {
        crate::testkit::emit_source(src)
    }

    fn after(ts: &str, head: &str) -> String {
        let at = ts.find(head).unwrap_or_else(|| panic!("no {head}:\n{ts}"));
        let rest = &ts[at..];
        rest[..rest.find("\n}\n").expect("a closing brace") + 3].to_string()
    }

    #[test]
    fn an_if_iife_over_nodes_prints_at_its_real_depth() {
        let src = "commons p1\n\nfn f(c: Bool, x: Int) -> Int {\n  let r = if c {\n    let y = x + 1\n    y * 2\n  } else {\n    0\n  }\n  r + 1\n}\n";
        let ts = emitted(src);
        assert_eq!(
            after(&ts, "export function f("),
            "export function f(c: boolean, x: number): number {\n  const r = (() => {\n    if (c) {\n      const y = x + 1;\n      return y * 2;\n    } else {\n      return 0;\n    }\n  })();\n  return r + 1;\n}\n"
        );
    }

    #[test]
    fn an_async_if_iife_is_an_awaited_async_arrow() {
        // The #2 review's case: a bare `await` inside the arrow needs `async`.
        let src = "commons p2\n\nfn bump(n: Int) -> Effect[Int] {\n  Effect.pure(n)\n}\n\nfn f(c: Bool) -> Effect[Int] {\n  let r = if c {\n    let y <- bump(1)\n    y + 1\n  } else {\n    0\n  }\n  Effect.pure(r)\n}\n";
        let ts = emitted(src);
        assert!(
            ts.contains(
                "  const r = await (async () => {\n    if (c) {\n      const y = await bump(1);\n      return y + 1;\n    } else {\n      return 0;\n    }\n  })();\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn if_iifes_nest_three_deep_at_their_real_depths() {
        let src = "commons p4\n\nfn f(c: Bool, d: Bool, x: Int) -> Int {\n  let r = if c {\n    let a = x\n    if d {\n      let b = a + 1\n      if c {\n        let e = b + 1\n        e\n      } else {\n        0\n      }\n    } else {\n      1\n    }\n  } else {\n    2\n  }\n  r\n}\n";
        let ts = emitted(src);
        assert!(
            ts.contains(
                "  const r = (() => {\n    if (c) {\n      const a = x;\n      if (d) {\n        const b = a + 1;\n        if (c) {\n          const e = b + 1;\n          return e;\n        } else {\n          return 0;\n        }\n      } else {\n        return 1;\n      }\n    } else {\n      return 2;\n    }\n  })();\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn an_if_iife_with_a_text_condition_keeps_its_text_byte_for_byte() {
        let src = "commons p7\n\nfn f(a: Bool, b: Bool, x: Int) -> Int {\n  let r = if a && b {\n    let y = x + 1\n    y\n  } else {\n    0\n  }\n  r\n}\n";
        let ts = emitted(src);
        assert!(
            ts.contains(
                "  const r = (() => {\n    if (a && b) {\n      const y = x + 1;\n      return y;\n    } else {\n      return 0;\n    }\n  })();\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_lambda_with_typed_and_inferred_params_is_an_arrow_node() {
        let src = "commons p3\n\ntype P = { a: Int }\n\nfn g(xs: List[Int]) -> List[P] {\n  xs.map((x: Int) => P { a: x })\n}\n\nfn f(xs: List[Int]) -> List[Int] {\n  xs.map((x) => x + 1)\n}\n\nfn h(xs: List[Int]) -> List[Int] {\n  xs.map((x) => {\n    let y = x * 2\n    y + 1\n  })\n}\n";
        let ts = emitted(src);
        // Typed and inferred params; an object-literal body is bracketed.
        assert!(ts.contains("((x: number) => x + 1)(__x)"), "{ts}");
        assert!(ts.contains("((x: number) => ({ a: x }))(__x)"), "{ts}");
        assert!(
            ts.contains("((x: number) => {\n    const y = x * 2;\n    return y + 1;\n  })(__x)"),
            "{ts}"
        );
    }

    #[test]
    fn a_lambda_expression_body_that_hoists_a_question_is_a_block() {
        let src = "commons p6\n\nfn parse(x: Int) -> Result[Int, String] {\n  Ok(x)\n}\n\nfn f(x: Int) -> Result[Int, String] {\n  let g: (Int) -> Result[Int, String] = (y: Int) => Ok(parse(y)? + 1)\n  g(x)\n}\n";
        let ts = emitted(src);
        assert!(
            ts.contains(
                "= (y: number) => {\n    const __r0 = parse(y);\n    if (__r0.tag === \"Err\") return __r0;\n    return Ok(__r0.value + 1);\n  };\n"
            ),
            "{ts}"
        );
    }
}

/// 1A.13: `match` lowers to nodes.
#[cfg(test)]
mod match_node_tests {
    /// The emitted TypeScript for a single commons.
    fn emitted(src: &str) -> String {
        crate::testkit::emit_source(src)
    }

    #[test]
    fn a_tail_switch_over_variants_prints_stacked_cases_with_bindings() {
        let ts = emitted(
            "commons t1\n\ntype Shape = | Circle(r: Int) | Square(w: Int) | Dot\n\nfn f(s: Shape) -> Int {\n  match s {\n    Circle(r) => r\n    Square(w) => w * 2\n    Dot => 0\n  }\n}\n",
        );
        assert!(
            ts.contains(
                "  switch (s.tag) {\n    case \"Circle\": {\n      const r = s.r;\n      return r;\n    }\n    case \"Square\": {\n      const w = s.w;\n      return w * 2;\n    }\n    case \"Dot\": {\n      return 0;\n    }\n  }\n  throw new globalThis.Error(\"non-exhaustive match\");\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_tail_switch_over_literals_stacks_or_pattern_labels() {
        let ts = emitted(
            "commons t2\n\nfn f(n: Int) -> String {\n  match n {\n    1 => \"a\"\n    2 | 3 => \"b\"\n    _ => \"c\"\n  }\n}\n",
        );
        assert!(
            ts.contains(
                "    case 1: {\n      return \"a\";\n    }\n    case 2:\n    case 3: {\n      return \"b\";\n    }\n    default: {\n      return \"c\";\n    }\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn an_or_pattern_with_a_shared_binding_is_an_if_chain_over_a_disjunction() {
        let ts = emitted(
            "commons t3\n\ntype Shape = | Circle(r: Int) | Square(w: Int) | Dot\n\nfn f(s: Shape) -> Int {\n  match s {\n    Circle(r) | Square(r) => r\n    Dot => 0\n  }\n}\n",
        );
        assert!(
            ts.contains(
                "  if ((s.tag === \"Circle\" || s.tag === \"Square\")) {\n    let r;\n    if (s.tag === \"Circle\") {\n      r = s.r;\n    } else {\n      r = s.w;\n    }\n    return r;\n  }\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_guard_lowers_to_an_if_chain_with_the_guard_inside_the_bindings() {
        let ts = emitted(
            "commons t4\n\ntype Shape = | Circle(r: Int) | Square(w: Int) | Dot\n\nfn f(s: Shape, k: Int) -> Int {\n  match s {\n    Circle(r) if r > k => r\n    Circle(r) => 0\n    Square(w) => w\n    Dot => 1\n  }\n}\n",
        );
        assert!(
            ts.contains(
                "  if (s.tag === \"Circle\") {\n    const r = s.r;\n    if (r > k) {\n      return r;\n    }\n  }\n  if (s.tag === \"Circle\") {"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_nested_pattern_is_an_if_chain_over_a_conjunction() {
        let ts = emitted(
            "commons t5\n\ntype Shape = | Circle(r: Int) | Square(w: Int) | Dot\n\nfn f(s: Option[Shape]) -> Int {\n  match s {\n    Some(Circle(r)) => r\n    Some(Square(w)) => w\n    Some(Dot) => 1\n    None => 0\n  }\n}\n",
        );
        assert!(
            ts.contains(
                "  if (s.tag === \"Some\" && s.value.tag === \"Circle\") {\n    const r = s.value.r;\n    return r;\n  }\n"
            ),
            "{ts}"
        );
        assert!(
            ts.contains(
                "  if (s.tag === \"None\") {\n    return 0;\n  }\n  throw new globalThis.Error(\"non-exhaustive match\");\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_value_position_match_is_a_sync_arrow_over_the_discriminant() {
        let ts = emitted(
            "commons t6\n\ntype Shape = | Circle(r: Int) | Square(w: Int) | Dot\n\nfn f(s: Shape) -> Int {\n  let n = match s {\n    Circle(r) => r\n    Square(w) => w * 2\n    Dot => 0\n  }\n  n + 1\n}\n",
        );
        assert!(
            ts.contains(
                "  const n = ((__d) => {\n    switch (__d.tag) {\n      case \"Circle\": {\n        const r = __d.r;\n        return r;\n      }\n      case \"Square\": {\n        const w = __d.w;\n        return w * 2;\n      }\n      case \"Dot\": {\n        return 0;\n      }\n    }\n    throw new globalThis.Error(\"non-exhaustive match\");\n  })(s);\n  return n + 1;\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn an_effectful_value_position_match_is_an_awaited_async_arrow() {
        let ts = emitted(
            "commons t7\n\ntype Shape = | Circle(r: Int) | Square(w: Int) | Dot\n\nfn bump(n: Int) -> Effect[Int] {\n  Effect.pure(n)\n}\n\nfn f(s: Shape) -> Effect[Int] {\n  let n = match s {\n    Circle(r) => {\n      let y <- bump(r)\n      y\n    }\n    Square(w) => w\n    Dot => 0\n  }\n  Effect.pure(n)\n}\n",
        );
        assert!(
            ts.contains(
                "  const n = await (async (__d) => {\n    switch (__d.tag) {\n      case \"Circle\": {\n        const r = __d.r;\n        const y = await bump(r);\n        return y;\n      }\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_question_in_a_tail_arm_hoists_inside_its_case() {
        let ts = emitted(
            "commons t8\n\ntype Shape = | Circle(r: Int) | Square(w: Int) | Dot\n\nfn parse(x: Int) -> Result[Int, String] {\n  Ok(x)\n}\n\nfn f(s: Shape) -> Result[Int, String] {\n  match s {\n    Circle(r) => Ok(parse(r)? + 1)\n    Square(w) => Ok(w)\n    Dot => Ok(0)\n  }\n}\n",
        );
        assert!(
            ts.contains(
                "    case \"Circle\": {\n      const r = s.r;\n      const __r0 = parse(r);\n      if (__r0.tag === \"Err\") return __r0;\n      return Ok(__r0.value + 1);\n    }\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_question_in_a_value_position_arm_breaks_out_of_the_labelled_switch() {
        let ts = emitted(
            "commons t9\n\ntype Shape = | Circle(r: Int) | Square(w: Int) | Dot\n\nfn parse(x: Int) -> Result[Int, String] {\n  Ok(x)\n}\n\nfn f(s: Shape) -> Result[Int, String] {\n  let n = match s {\n    Circle(r) => parse(r)?\n    Square(w) => w\n    Dot => 0\n  }\n  Ok(n)\n}\n",
        );
        assert!(
            ts.contains(
                "  __r0_out: {\n    switch (s.tag) {\n      case \"Circle\": {\n        const r = s.r;\n        const __r1 = parse(r);\n        if (__r1.tag === \"Err\") return __r1;\n        __r0 = __r1.value;\n        break __r0_out;\n      }\n"
            ),
            "{ts}"
        );
    }

    /// A typed lambda parameter whose type is built by the type builders keeps
    /// the exact bytes `ts_type_ref` printed. (The node path's guard that the two
    /// prints agree cannot fire while `ts_type_ref` is `print_type` of the same
    /// builder, so there is no input that stays text for that reason.)
    #[test]
    fn a_typed_lambda_param_prints_as_it_always_did() {
        let ts = emitted(
            "commons l1\n\nfn i(xs: List[(Int) -> Int]) -> List[Int] {\n  xs.map((x: (Int) -> Int) => 1)\n}\n",
        );
        assert!(
            ts.contains("((x: (a0: number) => number) => 1)(__x)"),
            "{ts}"
        );
    }

    /// An `Effect`-returning lambda is an `async` arrow node with a block body.
    #[test]
    fn an_async_lambda_is_an_async_arrow_with_a_block_body() {
        let ts = emitted(
            "commons l2\n\nfn bump(n: Int) -> Effect[Int] {\n  Effect.pure(n)\n}\n\nfn f(x: Int) -> Effect[Int] {\n  let g: (Int) -> Effect[Int] = (y: Int) => {\n    let z <- bump(y)\n    Effect.pure(z + 1)\n  }\n  g(x)\n}\n",
        );
        assert!(
            ts.contains(
                "= async (y: number) => {\n    const z = await bump(y);\n    return z + 1;\n  };\n"
            ),
            "{ts}"
        );
    }
}

/// 1A.14: `par` lowers to a node (an `async` thunk because *its* lowering
/// awaited), and the 1A.13 review's two extra `match` shapes.
#[cfg(test)]
mod par_node_tests {
    /// The emitted TypeScript for a single commons.
    fn emitted(src: &str) -> String {
        crate::testkit::emit_source(src)
    }

    fn func(ts: &str, head: &str) -> String {
        let at = ts.find(head).unwrap_or_else(|| panic!("no {head}:\n{ts}"));
        let rest = &ts[at..];
        rest[..rest.find("\n}\n").expect("a closing brace") + 3].to_string()
    }

    const HDR: &str = "commons par1\n\ntype Shape = | Circle(r: Int) | Dot\nfn mk(s: Shape) -> Shape { s }\nfn mk2(t: String) -> Shape { Dot }\nfn bump(n: Int) -> Effect[Int] { Effect.pure(n) }\nfn pick(b: Bool) -> Effect[Int] { Effect.pure(1) }\n\n";

    #[test]
    fn a_single_line_par_is_a_destructuring_const_over_thunks() {
        let ts = emitted(&format!(
            "{HDR}fn f(x: Int) -> Effect[Int] {{\n  par {{\n    let a <- bump(x)\n    let b <- bump(2)\n  }}\n  Effect.pure(a + b)\n}}\n"
        ));
        assert_eq!(
            func(&ts, "export async function f("),
            "export async function f(x: number): globalThis.Promise<number> {\n  const [a, b] = await __bynkSettleAll([() => bump(x), () => bump(2)]);\n  return a + b;\n}\n"
        );
    }

    #[test]
    fn a_typed_part_annotates_its_thunk_and_a_do_leaves_a_hole() {
        let ts = emitted(&format!(
            "{HDR}fn f(x: Int) -> Effect[Int] {{\n  par {{\n    let a: Int <- bump(x)\n    do bump(2)\n    let c <- bump(3)\n  }}\n  Effect.pure(a + c)\n}}\n"
        ));
        assert!(
            ts.contains(
                "  const [a, , c] = await __bynkSettleAll([(): globalThis.Promise<number> => bump(x), () => bump(2), () => bump(3)]);\n"
            ),
            "{ts}"
        );
    }

    /// The multi-line form's items sit at the printer's own depth: the old text
    /// indented them by one column, an odd offset (group (b) of #3715).
    #[test]
    fn a_par_that_hoists_prints_one_thunk_per_line_at_the_printers_depth() {
        let ts = emitted(&format!(
            "{HDR}fn f(s: Shape) -> Effect[Int] {{\n  par {{\n    let a <- pick(mk(s) is Dot)\n    let b: Int <- bump(2)\n  }}\n  Effect.pure(a + b)\n}}\n"
        ));
        assert_eq!(
            func(&ts, "export async function f("),
            "export async function f(s: Shape): globalThis.Promise<number> {\n  const [a, b] = await __bynkSettleAll([\n    () => {\n      const __r0 = mk(s);\n      return pick(((__c: boolean) => __c)(__r0.tag === \"Dot\"));\n    },\n    (): globalThis.Promise<number> => bump(2),\n  ]);\n  return a + b;\n}\n"
        );
    }

    /// Decision D: a thunk is `async` because its lowering awaited, not because
    /// its hoisted text contains `"await "`. The string literal here puts that
    /// text in the hoisted `const`; the thunk must stay synchronous.
    #[test]
    fn a_string_literal_containing_await_does_not_make_a_thunk_async() {
        let ts = emitted(&format!(
            "{HDR}fn f() -> Effect[Int] {{\n  par {{\n    let a <- pick(mk2(\"await \") is Dot)\n    let b <- bump(2)\n  }}\n  Effect.pure(a + b)\n}}\n"
        ));
        assert!(
            ts.contains("    () => {\n      const __r0 = mk2(\"await \");\n"),
            "{ts}"
        );
        assert!(!ts.contains("async () =>"), "{ts}");
    }

    /// An effectful value-position `if` inside a part awaits in its own
    /// lowering, so that thunk is `async` (the old text search missed it, as the
    /// `await` sat in the value, not in a hoisted statement).
    #[test]
    fn a_part_whose_lowering_awaited_is_an_async_thunk() {
        let ts = emitted(&format!(
            "{HDR}fn f(c: Bool) -> Effect[Int] {{\n  par {{\n    let a <- bump(if c {{\n      let y <- bump(1)\n      y\n    }} else {{ 0 }})\n    let b <- bump(2)\n  }}\n  Effect.pure(a + b)\n}}\n"
        ));
        assert!(
            ts.contains("[async () => bump(await (async () => {\n"),
            "{ts}"
        );
        assert!(
            ts.contains("}, () => bump(2)]);\n") || ts.contains(", () => bump(2)]);"),
            "{ts}"
        );
    }

    /// 1A.13 review: a value-position `match` with a guard is an arrow over an
    /// `if` chain. The arms' `return` lines are still text (`match_body` writes
    /// them as lines, which a later slice converts), so this pins the bytes.
    #[test]
    fn a_guarded_value_match_is_an_arrow_over_an_if_chain() {
        let ts = emitted(
            "commons m1\n\ntype Shape = | Circle(r: Int) | Square(w: Int) | Dot\n\nfn f(s: Shape, k: Int) -> Int {\n  let x = match s {\n    Circle(r) if r > k => r\n    Circle(r) => 0\n    Square(w) => w\n    Dot => 1\n  }\n  x\n}\n",
        );
        assert_eq!(
            func(&ts, "export function f("),
            "export function f(s: Shape, k: number): number {\n  const x = ((__d) => {\n    if (__d.tag === \"Circle\") {\n      const r = __d.r;\n      if (r > k) {\n        return r;\n      }\n    }\n    if (__d.tag === \"Circle\") {\n      const r = __d.r;\n      return 0;\n    }\n    if (__d.tag === \"Square\") {\n      const w = __d.w;\n      return w;\n    }\n    if (__d.tag === \"Dot\") {\n      return 1;\n    }\n    throw new globalThis.Error(\"non-exhaustive match\");\n  })(s);\n  return x;\n}\n"
        );
        // 1A.15: the arm tails are nodes too, so the whole function is.
    }

    /// 1A.13 review: a list pattern's elements read `path[i]` and its `..rest`
    /// reads `path.slice(n)` (the binding `const`s and the arm's `return` are nodes).
    #[test]
    fn a_list_pattern_binds_elements_by_index_and_the_rest_by_slice() {
        let ts = emitted(
            "commons m2\n\nfn f(xs: List[Int]) -> Int {\n  let x = match xs {\n    [a, b, ..rest] => a + b\n    _ => 0\n  }\n  x\n}\n",
        );
        assert_eq!(
            func(&ts, "export function f("),
            "export function f(xs: readonly number[]): number {\n  const x = ((__d) => {\n    if (__d.length >= 2) {\n      const a = __d[0];\n      const b = __d[1];\n      const rest = __d.slice(2);\n      return a + b;\n    }\n    return 0;\n  })(xs);\n  return x;\n}\n"
        );
        // 1A.15: the arm tails are nodes too, so the whole function is.
    }
}

/// 1A.17 (#3718): the `expect` statement's node form.
#[cfg(test)]
mod expect_item_tests {
    use super::*;
    use crate::ts_build::ident;

    /// The item as the one line it prints to.
    fn line(item: Item) -> String {
        match item {
            Item::Node(n) => bynk_ts::print_stmt(&n, 0).trim_end().to_string(),
            Item::Mark(_) => unreachable!("an expect is one statement"),
        }
    }

    fn node(name: &str) -> bynk_ts::TsExpr {
        ident(name)
    }

    fn compare(lv: bynk_ts::TsExpr, rv: bynk_ts::TsExpr) -> ExpectDetail {
        ExpectDetail::Compare {
            show: "__bynkShow",
            sym: "==",
            operands: Box::new((lv, rv)),
        }
    }

    /// The one-line statement prints the location and source as the printer escapes them, whatever
    /// they hold: a backslash, a quote, a newline, non-ASCII, U+2028 and a control character.
    #[test]
    fn the_node_prints_the_escaped_location_and_source_on_one_line() {
        let esc = crate::emitter::escape_ts_string;
        for raw in [
            "plain",
            "a\\b",
            "say \"hi\"",
            "line one\nline two",
            "caf\u{e9} \u{1F600}",
            "sep\u{2028}arator\u{2029}",
            "bell\u{7} nul\u{1} del\u{7f}",
        ] {
            let got = line(expect_item(
                node("c"),
                compare(node("l"), node("r")),
                raw,
                raw,
                (3, 9),
            ));
            let head = esc(&format!("expect {raw}\n  expected: {raw}\n  actual:   "));
            let want = format!(
                "if (!(c)) {{ throw __bynkExpectFailure(\"{}\", 3, 9, \"{head}\" + __bynkShow((l)) + \" == \" + __bynkShow((r))); }}",
                esc(raw)
            );
            assert_eq!(got, want, "{raw:?}");
            assert!(!got.contains('\n'), "one line: {raw:?}");
            // And the plain-message form.
            let msg = line(expect_item(
                node("c"),
                ExpectDetail::Message(format!("expect {raw}")),
                raw,
                raw,
                (3, 9),
            ));
            assert_eq!(
                msg,
                format!(
                    "if (!(c)) {{ throw __bynkExpectFailure(\"{}\", 3, 9, \"{}\"); }}",
                    esc(raw),
                    esc(&format!("expect {raw}"))
                ),
                "{raw:?}"
            );
        }
    }

    /// The printer layout probes: `!(cond)` takes no extra brackets, and the
    /// `if`/`throw` stays on one line.
    #[test]
    fn the_node_prints_as_the_one_line_if_with_a_bare_not_paren() {
        let got = line(expect_item(
            node("ok"),
            ExpectDetail::Message("expect ok".to_string()),
            "t:1:1",
            "ok",
            (0, 2),
        ));
        assert_eq!(
            got,
            "if (!(ok)) { throw __bynkExpectFailure(\"t:1:1\", 0, 2, \"expect ok\"); }"
        );
        assert!(matches!(
            expect_item(
                node("ok"),
                ExpectDetail::Message(String::new()),
                "",
                "",
                (0, 0)
            ),
            Item::Node(_)
        ));
    }

    /// A snapshot name is always a string-literal node, spelled as the printer spells every
    /// string. A name `bynk_project::json_string` would spell differently (a control character:
    /// `\u0007` against the printer's `\u{7}`) used to take a text arm; it is the same string
    /// either way, and now one spelling.
    #[test]
    fn a_snapshot_name_is_spelled_as_the_printer_spells_a_string() {
        let name = "bell\u{7} \"q\"";
        let check = snapshot_check(str_lit(name), ident("json"), "t:1:1", (0, 2), "v", name);
        let printed = bynk_ts::print_stmt(&check, 0);
        assert!(
            printed.contains("__bynkSnapshotCheck(\"bell\\u{7} \\\"q\\\"\", json)"),
            "{printed}"
        );
        assert_ne!(
            bynk_project::json_string(name),
            bynk_ts::print_expr(&str_lit(name)),
            "the two spellings differ, which is why the name used to take a text arm"
        );
    }

    /// The snapshot name and the `"Cap.op"` observation key: identifiers need no
    /// escape, so Rust's Debug form and the printer's `Str` agree.
    #[test]
    fn the_capability_op_key_prints_as_rust_debug_did() {
        let key = "Logger.log";
        assert_eq!(
            bynk_ts::print_expr(&bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(key.to_string()))),
            format!("{key:?}")
        );
    }

    /// The `expect … matches snapshot` check as a node prints the one text line it replaced.
    #[test]
    fn the_snapshot_check_node_prints_the_text_it_replaced() {
        let (location, src, shown) = ("tests/a.bynk:3:5", "r.body", "body \"x\"");
        let node = snapshot_check(
            str_lit("snap"),
            bynk_ts::TsExpr::Ident("__json".to_string()),
            location,
            (10, 20),
            src,
            shown,
        );
        let esc = crate::emitter::escape_ts_string;
        let want = format!(
            "{{ const __d = __bynkSnapshotCheck(\"snap\", __json); if (__d !== undefined) {{ throw __bynkExpectFailure(\"{}\", 10, 20, \"expect {} matches snapshot \\\"{}\\\"\\n  \" + __d); }} }}\n",
            esc(location),
            esc(src),
            esc(shown)
        );
        assert_eq!(bynk_ts::print_stmt(&node, 0), want);
    }
}
