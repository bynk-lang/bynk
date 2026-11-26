//! `?` and value-position control flow (back-half paydown 1A.8, 1A.12, 2.2c, #4087): `?`, the
//! value-position `if`, lambdas, block expressions and the `match` IIFE. Moved out of `lower.rs`
//! byte for byte.

use std::collections::HashMap;
use std::sync::Arc;

use bynk_check::checker::{Ty, Types};
use bynk_syntax::ast::{Block, Expr, ExprKind, LambdaExpr, MatchArm};

use super::super::lower_ctx::LowerCtx;
use super::super::lower_stmt::{Body, block_body, block_body_into, match_dispatch, ternary_shaped};
use super::super::ts_ident;
use super::super::ts_types::{ts_ty_to_ts_type, ts_type_ref_to_ts_type};
use super::{
    Lowered, PatPath, Pre, expect_location_raw, expect_source_text_raw,
    gather_is_bindings_for_emit, lower_expr, member_node, stmt_item, ternary_built,
    type_member_node,
};
use crate::ts_build::{arrow_of, call, ident, num_lit, str_lit};

/// v0.154 (ADR 0178): the `Err`-wrap a `?` on a `Result` operand needs, if the
/// operand's error type differs from the enclosing return's and a declared
/// embedding converts it. Returns `(sum_type, variant)` to emit
/// `Err(sum_type.variant(err))`, or `None` to propagate the `Err` unchanged.
/// Uses the checker's `embedding_for`, so it can never diverge from what the
/// checker accepted.
fn embed_conversion(
    operand_ty: Option<bynk_check::checker::TyId>,
    cx: &LowerCtx,
) -> Option<(String, String)> {
    let tys = cx.ty_render();
    use bynk_check::checker::Ty;
    let operand_node = operand_ty.map(|t| tys.get(t));
    let Some(Ty::Result(_, e)) = operand_node.as_deref() else {
        return None;
    };
    let f_err = peel_result_err(cx.return_ty?, tys.types())?;
    // An exact/compatible match propagates as-is; only a genuine mismatch that a
    // declared embedding resolves gets wrapped.
    if bynk_check::checker::compatible(*e, f_err, tys.types()) {
        return None;
    }
    bynk_check::checker::embedding_for(f_err, *e, &cx.commons().types, tys.types())
}

/// Is `ty` an `HttpResult`, or an `Effect` of one?
fn peel_http_result(ty: bynk_check::checker::TyId, tys: &Types) -> bool {
    let node = tys.get(ty);
    matches!(&*node, bynk_check::checker::Ty::HttpResult(_))
        || node
            .as_effect()
            .is_some_and(|inner| peel_http_result(inner, tys))
}

/// #3966: does the enclosing body return an `Option` (or an `Effect` of one)? Then a `?` on an
/// `Option` returns the `None` itself, rather than answering an `HttpResult` handler's `NotFound`.
/// The checker accepted the `?` under the same rule.
fn option_returns_option(cx: &LowerCtx) -> bool {
    let tys = cx.ty_render();
    let mut ty = cx.return_ty;
    while let Some(t) = ty {
        let node = tys.get(t);
        if matches!(&*node, bynk_check::checker::Ty::Option(_)) {
            return true;
        }
        ty = node.as_effect();
    }
    false
}

/// Peel `Result[_, E]` / `Effect[Result[_, E]]` to the error type `E`.
fn peel_result_err(
    ty: bynk_check::checker::TyId,
    tys: &Arc<Types>,
) -> Option<bynk_check::checker::TyId> {
    let ty = tys.get(ty);
    if let Some((_, e)) = ty.as_result() {
        Some(e)
    } else {
        peel_result_err(ty.as_effect()?, tys)
    }
}

/// Slice 1A.8: the value-position `if`'s hoist: `let slot: T; if (cond) { …; slot = then; } else
/// { …; slot = else; }` as statement nodes, the slot typed as the checker typed `e`. Also the
/// short-circuit operators' (`hoist_bool_if`) and the async tail's (`lower_tail_expr`, whose slot
/// annotation admits a promise). Returns the slot.
pub(in crate::emitter) fn hoist_if_slot(
    pre: &mut Pre,
    slot_ty: Option<bynk_ts::TsType>,
    cond: bynk_ts::TsExpr,
    then_tail: Lowered,
    else_tail: Lowered,
    cx: &mut LowerCtx,
) -> String {
    let slot = cx.fresh();
    // The slot needs its type spelled out. TypeScript's "evolving `let`" inference gives up
    // once a branch also `return`s (TS7034/TS7005 under `--strict`), which is exactly the shape
    // a hoisted `?` produces, so the type is the one the checker recorded for the `if`.
    pre.push_node(bynk_ts::TsStmt::let_stmt(
        bynk_ts::TsBindingName::Ident(slot.clone()),
        slot_ty,
        None,
        None,
    ));
    let branch = |tail: Lowered| {
        let mut stmts: Vec<bynk_ts::TsStmt> = tail.pre.into_iter().collect();
        stmts.push(bynk_ts::TsStmt::assign(ident(&slot), tail.expr, None));
        bynk_ts::TsStmt::block(stmts, None)
    };
    let then_branch = branch(then_tail);
    let else_branch = branch(else_tail);
    pre.push_node(bynk_ts::TsStmt::if_else_same_line_stmt(
        cond,
        then_branch,
        else_branch,
        None,
    ));
    slot
}

/// The type the checker recorded for `e` as a type node, if any: the annotation of the slot a
/// hoisted `if` declares.
pub(in crate::emitter) fn checked_ty_node(e: &Expr, cx: &LowerCtx) -> Option<bynk_ts::TsType> {
    cx.commons()
        .expr_ty(e.id)
        .map(|_| ts_ty_to_ts_type(cx.commons().expr_types[&e.id].ty, cx.ty_render()))
}

/// How a `?` leaves the function when its operand is the failing side. One
/// value drives both the node guard and today's text guard, so they cannot
/// disagree about which case applies.
enum QuestionExit {
    /// #2857: a test case's `?` fails the case, reporting the `Err` at the
    /// `?`'s location. Strings are raw (the printer, or `escape_ts_string` on
    /// the text side, escapes them).
    Case {
        location: String,
        start: usize,
        end: usize,
        src: String,
        /// #3982: the operand is an `Option`, so `None` (not `Err`) fails the case.
        none: bool,
    },
    /// v0.153 (ADR 0177): `None` early-returns `HttpResult.NotFound`.
    NotFound,
    /// v0.154 (ADR 0178): `Err(T.V(__rN.error))`.
    Embed { ty_name: String, variant: String },
    /// `Err` propagates unchanged.
    Plain,
    /// #3966: in a body returning `Option`, `None` propagates unchanged.
    NoneUnchanged,
}

/// The `?` guard as a statement node: `if (__rN.tag === "Err") return …;`. The
/// return is not a `Block`, so the printer keeps it inline and the output is
/// byte-identical to the text form. The case form is an `InlineBlock`, which
/// prints `{ throw …; }` on the one line today's text has.
fn question_guard_node(tmp: &str, exit: &QuestionExit) -> bynk_ts::TsStmt {
    let tag = if matches!(
        exit,
        QuestionExit::NotFound
            | QuestionExit::NoneUnchanged
            | QuestionExit::Case { none: true, .. }
    ) {
        "None"
    } else {
        "Err"
    };
    let cond = bynk_ts::TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::StrictEq,
        left: Box::new(member_node(tmp, "tag")),
        right: Box::new(str_lit(tag)),
    };
    let then = match exit {
        QuestionExit::Case {
            location,
            start,
            end,
            src,
            none,
        } => {
            let detail = if *none {
                // #3982: a `None` carries no value to show.
                str_lit(format!("{src}\n  the operand was None"))
            } else {
                bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::Add,
                    left: Box::new(str_lit(format!("{src}\n  the operand was Err: "))),
                    right: Box::new(call(ident("__bynkShow"), vec![member_node(tmp, "error")])),
                }
            };
            let failure = call(
                ident("__bynkExpectFailure"),
                vec![
                    str_lit(location.clone()),
                    num_lit(start.to_string()),
                    num_lit(end.to_string()),
                    detail,
                ],
            );
            bynk_ts::TsStmt::inline_block(vec![bynk_ts::TsStmt::throw_stmt(failure, None)], None)
        }
        QuestionExit::NotFound => {
            bynk_ts::TsStmt::return_stmt(Some(member_node("HttpResult", "NotFound")), None)
        }
        QuestionExit::Embed { ty_name, variant } => bynk_ts::TsStmt::return_stmt(
            Some(call(
                ident("Err"),
                vec![call(
                    type_member_node(ty_name, variant),
                    vec![member_node(tmp, "error")],
                )],
            )),
            None,
        ),
        QuestionExit::Plain | QuestionExit::NoneUnchanged => {
            bynk_ts::TsStmt::return_stmt(Some(ident(tmp)), None)
        }
    };
    bynk_ts::TsStmt::if_stmt(cond, then, None)
}

/// `inner?` (slice 1A.8): hoists `const __rN = <inner>;` and the early-return
/// guard, and yields `__rN.value`. The two statements and the yielded member are nodes.
pub(super) fn lower_question(e: &Expr, inner: &Expr, cx: &mut LowerCtx) -> Lowered {
    let tys = cx.ty_render();
    // v0.153 (ADR 0177): an `Option[T]?` operand lifts into an
    // HttpResult handler — `None` early-returns `NotFound` (404),
    // `Some(v)` yields `v`. #3966: in a body returning `Option`, the `None`
    // returns unchanged. Any other operand is a `Result`: `Err`
    // propagates unchanged. Branch on the operand's checked type.
    // The checker rejects any other `?` operand, so a typed operand is
    // always present — assert it, so a future gap surfaces loudly in
    // tests rather than silently emitting the `Result` branch (which on
    // an untyped `Option` would leak `None` → `undefined`).
    let operand_ty = cx.commons().expr_types.get(&inner.id).map(|te| te.ty);
    let operand_node = operand_ty.map(|t| tys.get(t));
    debug_assert!(
        matches!(
            operand_node.as_deref(),
            Some(Ty::Option(_) | Ty::Result(_, _))
        ),
        "`?` operand has no `Option`/`Result` checked type at {:?}: {operand_node:?}",
        inner.span,
    );
    let is_option = matches!(operand_node.as_deref(), Some(Ty::Option(_)));
    // v0.154 (ADR 0178): a `Result` operand whose error type differs from
    // the enclosing return's is converted by a declared embedding
    // (`embeds E as V`) — the same rule the checker accepted it under.
    let embed = if is_option || cx.question_fails_case() {
        None
    } else {
        embed_conversion(operand_ty, cx)
    };
    let mut pre = Pre::new();
    let operand = pre.lower_node(inner, cx);
    let tmp = cx.fresh();
    let exit = if cx.question_fails_case() {
        // #2857: a case returns a `CaseResult`, not the operand's
        // `Result` — so the `?` fails the case, reporting the `Err`
        // value at the `?`'s location. #3982: and an `Option`'s `None` fails it too.
        QuestionExit::Case {
            location: expect_location_raw(cx, e.span.start),
            start: e.span.start,
            end: e.span.end,
            src: expect_source_text_raw(cx, e.span),
            none: is_option,
        }
    } else if is_option && option_returns_option(cx) {
        QuestionExit::NoneUnchanged
    } else if is_option {
        // The checker accepted this `?` only in an `HttpResult` body or an `Option` body (#3966).
        // The latter was taken above, so the enclosing return must be an `HttpResult`: a body
        // site that never bound `return_ty` would otherwise miscompile an `Option` function.
        debug_assert!(
            cx.return_ty
                .is_some_and(|t| peel_http_result(t, tys.types())),
            "`?` on an `Option` lowered with no enclosing `HttpResult` return at {:?}",
            e.span,
        );
        QuestionExit::NotFound
    } else if let Some((ty_name, variant)) = embed {
        QuestionExit::Embed { ty_name, variant }
    } else {
        QuestionExit::Plain
    };
    pre.push_node(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(tmp.clone()),
        None,
        operand,
        None,
    ));
    pre.push_node(question_guard_node(&tmp, &exit));
    // T2.3 (R6.3): every branch above just pushed a `return` that must
    // exit the enclosing function, not whatever scope this `Lowered`
    // eventually lands in — see `LowerCtx::emitted_early_return`.
    cx.emitted_early_return = true;
    pre.finish(member_node(&tmp, "value"))
}

pub(super) fn lower_if(
    e: &Expr,
    cond: &Expr,
    then_block: &Block,
    else_block: &Block,
    cx: &mut LowerCtx,
) -> Lowered {
    let mut pre = Pre::new();
    let cond_lowered = pre.lower_node(cond, cx);
    // If the cond contains `is`-bindings, the then-branch needs a place
    // for the `const name = receiver.field;` declarations — a ternary
    // has no such place. Force the IIFE form. (`ternary_shaped`.)
    if ternary_shaped(cond, then_block, else_block, cx) {
        // T2.1 (R6.2): lower each branch's tail once, then ask the branches
        // themselves whether they hoisted, rather than asking `simple_expr` to
        // predict it.
        //
        // #1029 review: `return_ty` is deliberately **not** cleared here, unlike
        // the IIFE path below. That path clears it because a `return` in its
        // arms exits the arrow rather than the enclosing function, so an
        // embedding `?` must behave like a plain one (ADR 0178). This path
        // inverts that premise: when a branch hoists, `hoist_if_slot`
        // puts the statements in the *caller's* statement position, so the `?`
        // expansion's `return` does exit the enclosing function and the declared
        // embedding must still apply. Clearing it emitted a bare `return __rN;`
        // — an `Err` of the operand's error type returned from a function
        // declared with the embedding sum's.
        //
        // Preserving it cannot move the no-hoist branch: `return_ty` is read
        // only by `embed_conversion`, only from the `?` expansion, and a `?`
        // expansion always pushes to `pre` — so where `pre` is empty, nothing
        // read it.
        let then_tail = lower_expr(&then_block.tail, cx);
        let else_tail = lower_expr(&else_block.tail, cx);
        if then_tail.pre.is_empty() && else_tail.pre.is_empty() {
            // The `Paren` keeps today's outer brackets.
            return pre.finish(ternary_built(cond_lowered, then_tail.expr, else_tail.expr));
        }
        // A branch hoisted. Before T2.1 there was nowhere to put its statements
        // from here — a value position had no statement slot — and the only
        // shape available was the arrow below, which swallows a hoisted `?`'s
        // early return. `lower_if` now returns its own `pre`, so the `if` hoists
        // as a real statement in the caller's statement position and the
        // `return` keeps exiting the enclosing function. See
        // `hoist_if_slot`.
        let slot_ty = checked_ty_node(e, cx);
        let slot = hoist_if_slot(&mut pre, slot_ty, cond_lowered, then_tail, else_tail, cx);
        pre.finish(ident(&slot))
    } else {
        // T2.2 (R6.4): isolate the effectfulness flag to just this IIFE's own
        // body. Unlike `lower_match_as_iife`'s `disc_expr` (spliced *outside*
        // the returned arrow, at the call site), `cond`'s *text* is spliced
        // inside this arrow's own `if (...)` below — only its hoisted
        // statements landed in `pre` above. A bare `await` in `cond`'s text
        // would need to feed `needs_async`, not `saved_await`; the
        // debug_assert after the arrow closes documents that this can't
        // happen today (a literal `await` comes only from `EffectLet`/`Do`,
        // both hoisted to `pre`, or from a nested `finish_async_iife` wrap,
        // which would require a value-position `match`/`if` needing the async
        // wrap to itself type as `Bool` — ruled out by the checker's
        // Effect-typing) rather than silently trusting it.
        let saved_await = std::mem::take(&mut cx.emitted_await);
        debug_assert!(
            !bynk_ts::expr_contains_await(&cond_lowered),
            "cond lands inside this arrow; its awaits must feed `needs_async`, not `saved_await`"
        );
        // Inject is-binding declarations on the truthy side. #908: scoped to
        // the then-branch only — pushed here, popped once it's emitted —
        // so a read inside resolves to the binding rather than an outer
        // `let` rename, and the registration doesn't leak into the else
        // branch or beyond.
        cx.shadow_scopes.push(HashMap::new());
        let mut is_bindings = Vec::new();
        let mut found = false;
        gather_is_bindings_for_emit(cond, true, cx, &mut is_bindings, &mut found);
        // 1A.12: each branch is built as a `Body` and rendered, or turned into
        // nodes, once both are in hand (its `is` bindings, nodes or text lines,
        // open it); the context saves and restores below
        // sit around the same lowering calls, in the same order, as when each
        // branch was rendered straight into the text.
        let mut then_body = Body::new();
        // The `is` bindings open their branch, before its statements.
        then_body.extend(is_bindings.iter().map(stmt_item));
        // v0.154 (ADR 0178): a value-position `if` lowers to an IIFE, whose
        // `return`s exit the arrow, so clear the enclosing `return_ty`. #1750:
        // an `if` whose branches hold a `?` no longer reaches here (it becomes a
        // statement, `hoist_value_as_statement`, so the `?` exits the function
        // and keeps its embedding), so nothing inside this arrow reads it.
        let saved = cx.return_ty.take();
        // #1750: the arrow's own tails `return` from the arrow.
        let saved_slot = cx.tail_slot.take();
        // The branches keep their marks (1A.15, Decision D): as nodes the printer
        // records them, and the text fallback below drops them (`render_text`).
        block_body_into(&mut then_body, then_block, cx, false);
        cx.shadow_scopes.pop();
        // #1654: the bindings the condition's falsity proves, scoped to the
        // else-branch the same way.
        cx.shadow_scopes.push(HashMap::new());
        let mut else_bindings = Vec::new();
        gather_is_bindings_for_emit(cond, false, cx, &mut else_bindings, &mut found);
        let mut else_body = Body::new();
        else_body.extend(else_bindings.iter().map(stmt_item));
        block_body_into(&mut else_body, else_block, cx, false);
        cx.shadow_scopes.pop();
        cx.return_ty = saved;
        cx.tail_slot = saved_slot;
        // #2 review: unlike `lower_match_as_iife`'s IIFE, this one used to never
        // consult the async-wrap decision at all — confirmed live: `let r = if
        // c { let y <- fetch(); y + 1 } else { 0 }` emitted a bare `await`
        // inside this arrow with no `async` keyword, a hard JS syntax error
        // ("Unexpected reserved word"), not merely a missed optimisation.
        let needs_async = cx.emitted_await;
        cx.emitted_await = saved_await || needs_async;
        let branch = bynk_ts::TsStmt::if_else_same_line_stmt(
            cond_lowered,
            bynk_ts::TsStmt::block(then_body.into_stmts(), None),
            bynk_ts::TsStmt::block(else_body.into_stmts(), None),
            None,
        );
        pre.finish(iife_call(needs_async, vec![branch]))
    }
}

/// `(() => { <body> })()`, an immediately invoked arrow over statement nodes,
/// `async` and awaited when `is_async` (1A.12).
fn iife_call(is_async: bool, body: Vec<bynk_ts::TsStmt>) -> bynk_ts::TsExpr {
    arrow_call(Vec::new(), Vec::new(), is_async, body)
}

/// `((params) => { <body> })(args)`, `async` and awaited when `is_async`: the
/// `match` IIFE's `(__d) => { … }` over its discriminant, and [`iife_call`]'s
/// parameterless one (1A.13).
pub(super) fn arrow_call(
    params: Vec<bynk_ts::TsParam>,
    args: Vec<bynk_ts::TsExpr>,
    is_async: bool,
    body: Vec<bynk_ts::TsStmt>,
) -> bynk_ts::TsExpr {
    let call = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Arrow {
            params,
            is_async,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Multiline(body)),
        }))),
        args,
    };
    if is_async {
        bynk_ts::TsExpr::Await(Box::new(call))
    } else {
        call
    }
}

/// True if `e` proves at least one `is` test when true — the tests
/// `gather_is_bindings_for_emit` declares, by the shared rule
/// [`bynk_check::narrowing::matched_is_tests`] (#1654), which descends `!`,
/// `&&`, `||` and `implies` with polarity. Used by `lower_and_with_is` to decide
/// whether the `is`-binding flow applies before doing any lowering.
pub(super) fn cond_contains_is(e: &Expr) -> bool {
    !bynk_check::narrowing::matched_is_tests(e, true).is_empty()
}

pub(super) fn lower_lambda(e: &Expr, lambda: &LambdaExpr, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let tys = cx.ty_render();
    let is_async = matches!(
        cx.commons().expr_ty(e.id).as_deref(),
        Some(bynk_check::checker::Ty::Fn { ret, .. }) if ret.is_effect(tys.types())
    );
    // #2712: a record-pattern parameter has no annotation to print; its type is
    // the checked function type's, so a lambda with nothing to type it from
    // (held in a `let`) still compiles under `tsc --strict`. #3228: the same
    // holds for any unannotated parameter, which TypeScript can type from
    // context only when the lambda is handed straight to a typed parameter: a
    // spread element (`[...acc, (x) => …]`) or an immediately invoked arrow
    // (`((x) => …)(__x)`) gives it none, and `--strict` rejects the implicit
    // `any` (TS7006). So every parameter is annotated with its checked type.
    let fn_params = match cx.commons().expr_ty(e.id).as_deref() {
        Some(bynk_check::checker::Ty::Fn { params, .. }) => params.clone(),
        _ => Vec::new(),
    };
    let params: Vec<bynk_ts::TsParam> = lambda
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ty = match (&p.type_ref, &p.pattern, fn_params.get(i)) {
                (Some(tr), _, _) => Some(ts_type_ref_to_ts_type(tr, None)),
                (None, _, Some(ty)) => Some(ts_ty_to_ts_type(*ty, tys)),
                _ => None,
            };
            bynk_ts::TsParam {
                name: ts_ident(&p.name.name),
                ty,
                optional: false,
                rest: false,
                array_pattern: None,
                default: None,
            }
        })
        .collect();
    // #908: a param's name is the lambda body's own binding — push a frame and
    // register each so a body read resolves to the param rather than an
    // outer `let` rename, popped once the body is lowered so the
    // registration doesn't leak past the lambda.
    cx.shadow_scopes.push(HashMap::new());
    cx.lambda_depth += 1;
    for p in &lambda.params {
        cx.declare_binder(&p.name.name);
    }
    // v0.154 (ADR 0178): a lambda is its own return scope — a `return` in its
    // body (from a `?`) exits the arrow, not the enclosing function. Rebind
    // `return_ty` to the lambda's own return (its `Fn` type's `ret`) so an
    // embedding `?` anywhere in the body — block-bodied or expression-bodied —
    // uses that, not the outer one; restore after either form.
    let lam_ret = match cx.commons().expr_ty(e.id).as_deref() {
        Some(bynk_check::checker::Ty::Fn { ret, .. }) => Some(*ret),
        _ => None,
    };
    let saved = cx.return_ty.take();
    cx.return_ty = lam_ret;
    // #1750: the lambda's own tails `return` from the lambda.
    let saved_slot = cx.tail_slot.take();
    // #1764 review: an enclosing short-circuit's temp hoist stops here. A
    // receiver temp inside the lambda is read only inside it, and hoisting its
    // declaration out would share one slot between the lambda's invocations.
    let saved_hoist = cx.is_temp_hoist.take();
    // A block body or an expression body: one shape tested, nothing to decide per construct.
    let result: bynk_ts::TsExpr = if let ExprKind::Block(b) = &lambda.body.kind {
        // The body keeps its marks (1A.15, Decision D).
        let body = block_body(b, cx, is_async);
        arrow_of(
            params,
            is_async,
            bynk_ts::TsArrowBody::Multiline(body.into_stmts()),
        )
    } else {
        let lowered = lower_expr(&lambda.body, cx);
        let hoisted = lowered.pre;
        let value = lowered.expr;
        let body = if hoisted.is_empty() {
            // An object-literal body is bracketed by the printer.
            bynk_ts::TsArrowBody::Expr(Box::new(value))
        } else {
            let mut stmts: Vec<bynk_ts::TsStmt> = hoisted.into_iter().collect();
            stmts.push(bynk_ts::TsStmt::return_stmt(Some(value), None));
            bynk_ts::TsArrowBody::Multiline(stmts)
        };
        arrow_of(params, is_async, body)
    };
    cx.is_temp_hoist = saved_hoist;
    cx.tail_slot = saved_slot;
    cx.return_ty = saved;
    cx.lambda_depth -= 1;
    cx.shadow_scopes.pop();
    result
}

pub(super) fn lower_block_as_expr(b: &Block, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    // #1750: the arrow's own tails `return` from the arrow.
    let saved_slot = cx.tail_slot.take();
    // IIFE is a synchronous arrow function; the surrounding expression context
    // expects a concrete value, so `Effect.pure(...)` must still wrap as
    // `Promise.resolve(...)`.
    // v0.154 (ADR 0178): a `return` here exits the arrow, not the function, so
    // clear `return_ty`. #1750: a block holding a `?` no longer reaches here
    // (`hoist_value_as_statement`).
    let saved = cx.return_ty.take();
    // The body keeps its marks (1A.15, Decision D).
    let body = block_body(b, cx, false);
    cx.return_ty = saved;
    cx.tail_slot = saved_slot;
    iife_call(false, body.into_stmts())
}

/// #1 review: the discriminant's hoisted statements (`pre`) are the
/// caller's own — pushed straight into whatever scope is lowering this
/// `match` expression, exactly like every other `ExprKind` arm in
/// `lower_expr_into`. This used to lower the discriminant into a *local*,
/// throwaway vector and — when non-empty — splice it into a freshly
/// synthesised wrapper arrow via `(() => { ...stmts; return inner; })()`. A
/// `?` in the discriminant hoists `if (__rN.tag === "Err") return __rN;`; a
/// `return` inside that wrapper arrow exits the wrapper, not the enclosing
/// function, so the early return's `Err` silently became the whole match
/// expression's *value* instead of propagating out — a miscompile with no
/// diagnostic and no assertion, unlike its two ternary-path siblings (see
/// `lower_if`/`lower_tail_expr`, which at least `debug_assert!`ed the
/// analogous case before this same class of fix).
pub(super) fn lower_match_as_iife(
    discriminant: &Expr,
    arms: &[MatchArm],
    cx: &mut LowerCtx,
) -> Lowered {
    let mut pre = Pre::new();
    let disc_ty = cx
        .commons()
        .expr_types
        .get(&discriminant.id)
        .map(|te| te.ty);
    let disc = pre.lower_node(discriminant, cx);
    // v0.154 (ADR 0178): a value-position `match` lowers to an IIFE, whose
    // `return`s exit the arrow, so clear `return_ty`. #1750: a `match` whose
    // arms hold a `?` no longer reaches here (`hoist_value_as_statement`).
    let saved = cx.return_ty.take();
    // #1750: the arrow's own tails `return` from the arrow.
    let saved_slot = cx.tail_slot.take();
    // T2.2 (R6.4): isolate the effectfulness flag to just this IIFE's own
    // body — `disc` was already lowered above, so any await it needed landed
    // in `pre`, not here. Restore as `saved_await || needs_async` so an
    // `await (async …)()` this call produces (if it does) still marks the
    // *caller's* scope — the propagation the old text scan got for free from
    // the substring surviving in the returned string.
    let saved_await = std::mem::take(&mut cx.emitted_await);
    // The arrow's body keeps its marks (1A.15, Decision D).
    let mut body = Body::new();
    // #3980: an `expect false` arm names its arm and the value matched.
    let framed = {
        let frame = super::expect_was::scrutinee_frame(discriminant, &ident("__d"), arms, cx);
        cx.push_scrutinee(frame)
    };
    match_dispatch(&mut body, &PatPath::ident("__d"), &disc_ty, arms, cx, false);
    if framed {
        cx.pop_scrutinee();
    }
    let needs_async = cx.emitted_await;
    cx.emitted_await = saved_await || needs_async;
    cx.return_ty = saved;
    cx.tail_slot = saved_slot;
    let params = vec![bynk_ts::TsParam::new("__d".to_string(), None)];
    pre.finish(arrow_call(
        params,
        vec![disc],
        needs_async,
        body.into_stmts(),
    ))
}

/// T2.2 (#1018, R6.4): `maybe_async_iife`'s `if !iife.contains("await ")`
/// replaced by `LowerCtx::emitted_await`, a flag set at the two statement
/// sites that emit a literal `await` (`EffectLet`, `Do`) and read-and-reset
/// around a value-position `match`/`if` IIFE's own body construction — see
/// `finish_async_iife`. Same precedent as `conditional_runtime_import_tests`
/// (`body.contains("__bynkBytes")` → `RuntimeUse`): the scan and the flag
/// agree on every case except where the scan's substring match wasn't really
/// about *this* scope's own effectfulness.
#[cfg(test)]
mod async_iife_effectfulness_tests {
    use crate::testkit::emit_source;

    /// The over-match this slice closes: a `Query`/broadcast iterator terminal
    /// (`forEach`) lowers to a self-contained, always-`async () => {...}` IIFE
    /// — a plain `Effect`-typed *value*, not something this arm itself awaits.
    /// The old scan saw the literal `"await "` inside that embedded string and
    /// wrongly wrapped the enclosing `match` arrow as `async` too, producing a
    /// spurious `await await (async (__d) => …)()`. Neither arm needs the
    /// switch's own arrow to be async, so it must stay a plain `(__d) => {…}`,
    /// invoked with a single `await` from the enclosing `let r <- …`.
    #[test]
    fn match_arm_iterator_terminal_does_not_force_the_switch_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn noop(n: Int) -> Effect[()] {\n  Effect.pure(())\n}\n\n\
             fn run(names: List[Int], flag: Bool) -> Effect[()] {\n  \
             let r <- match flag {\n    \
             true => names.forEach(noop)\n    \
             false => Effect.pure(())\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await ((__d) => {"),
            "the switch arrow must stay synchronous, awaited once by the `let <-`: {ts}"
        );
        assert!(!ts.contains("await await"), "{ts}");
    }

    /// The same over-match, reached through the `if`-IIFE path (an `is`-binding
    /// forces the IIFE form rather than a ternary) instead of `match`.
    #[test]
    fn if_arm_iterator_terminal_does_not_force_the_iife_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn noop(n: Int) -> Effect[()] {\n  Effect.pure(())\n}\n\n\
             fn run(o: Option[Int], names: List[Int]) -> Effect[()] {\n  \
             let r <- if o is Some(n) {\n    \
             names.forEach(noop)\n  \
             } else {\n    \
             Effect.pure(())\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await (() => {"),
            "the if-IIFE arrow must stay synchronous, awaited once by the `let <-`: {ts}"
        );
        assert!(!ts.contains("await await"), "{ts}");
    }

    /// Non-regression: the defect `#2 review` fixed in `8068c0db` (a genuinely
    /// effectful `is`-binding `if` in value position) must still emit a real
    /// `async` IIFE — this is not an over-match, so it keeps needing the wrap
    /// that closed the original hard `SyntaxError`.
    #[test]
    fn if_arm_genuine_effect_still_forces_the_iife_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn fetch(n: Int) -> Effect[Int] {\n  Effect.pure(n)\n}\n\n\
             fn run(o: Option[Int], c: Bool) -> Effect[Int] {\n  \
             let r <- if o is Some(n) && c {\n    \
             let y <- fetch(n)\n    \
             Effect.pure(y + 1)\n  \
             } else {\n    \
             Effect.pure(0)\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("async () => {") && ts.contains("await fetch(n)"),
            "a real await inside the IIFE must still force the async wrap: {ts}"
        );
    }

    /// Non-regression: a genuinely effectful *nested* `match` (embedded
    /// directly as an outer arm's value, not via an intermediate `let <-`)
    /// must still propagate its own async-wrap need to the enclosing switch
    /// arrow — the flag's `saved || needs_async` restore is what replaces the
    /// old scan's implicit propagation (the substring surviving in the
    /// returned string).
    #[test]
    fn nested_match_effectfulness_propagates_to_the_outer_switch_arrow() {
        let ts = emit_source(
            "commons t\n\n\
             fn fetch(n: Int) -> Effect[Int] {\n  Effect.pure(n)\n}\n\n\
             fn run(a: Bool, b: Bool) -> Effect[Int] {\n  \
             let r <- match a {\n    \
             true => match b {\n      \
             true => {\n        let x <- fetch(1)\n        Effect.pure(x + 1)\n      }\n      \
             false => Effect.pure(2)\n    \
             }\n    \
             false => Effect.pure(3)\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await await (async (__d) => {"),
            "the outer switch arrow must also become async: {ts}"
        );
    }

    /// Non-regression, mirroring the `EffectLet` case above for the *other*
    /// write site: an arm whose only effect is a `do` (no binder) must still
    /// force its switch arrow async. The old scan was blind to which
    /// statement produced the `await`, so `Do` came along for free; the flag
    /// has to name it explicitly at its own site (`emit_statement`'s
    /// `Statement::Do` arm) and nothing else pins that it does.
    #[test]
    fn match_arm_do_only_effect_still_forces_the_switch_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn noop(n: Int) -> Effect[()] {\n  Effect.pure(())\n}\n\n\
             fn run(flag: Bool) -> Effect[()] {\n  \
             let r <- match flag {\n    \
             true => {\n      do noop(1)\n      Effect.pure(())\n    }\n    \
             false => Effect.pure(())\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await await (async (__d) => {"),
            "a do-only effectful arm must also force the switch arrow async: {ts}"
        );
    }
}
