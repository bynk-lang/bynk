//! The `List` kernel's lowering: `lower_list_kernel` and the helpers its arms
//! share. Split out of `lower.rs` (back-half paydown 1A.18a, #3719). The
//! dispatcher matches the checker's typed [`ListOp`]; each operation is its own
//! builder, and the pieces the sibling kernels will want (comparator binding,
//! keyed-helper choice, key-function types) are plain functions here.

mod basics;

use bynk_check::checker::{Ty, TyId, Types};
use bynk_check::kernel_op::ListOp;
use bynk_ts::{TsBinaryOp, TsExpr, TsLit, TsParam, TsStmt, TsType, TsUnaryOp};

use super::internal_fault::FaultSite;
use super::lower::{Pre, decimal_lit};
use super::lower_ctx::LowerCtx;
use super::ts_types::{TyRender, ts_ty_to_ts_type};
use super::{BaseType, Expr};
use crate::ts_build::{
    arrow, arrow_with, assign, binary, block, block_arrow, call, const_typed, expr_stmt, for_of,
    ident, let_init, member, method_call, num_lit, paren, ret, spread,
};

/// v0.150 (ADR 0174): peel a `traverseTry`/`parTraverseTry` call's checked type
/// `Effect[Result[List[U], E]]` to the `U` TS — the short-circuit collect's
/// output-array element annotation. A call whose checked type is not that shape
/// is a missing checker fact (1A.34).
pub(super) fn list_ok_elem(call_ty: Option<TyId>, tys: TyRender<'_>, at: FaultSite<'_>) -> TsType {
    match call_ty.and_then(|t| tys.get(t).as_effect()) {
        Some(inner) => match tys.get(inner).as_result() {
            Some((ok, _)) => ts_ty_to_ts_type(tys.get(ok).as_list().unwrap_or(ok), tys),
            None => ts_ty_to_ts_type(inner, tys),
        },
        None => {
            at.missing_fact_ty("`Effect[Result[List[_], _]]` type for a short-circuit collect call")
        }
    }
}

/// `any`, the one place the lowering spells it, for what is deliberate and not a fallback: the
/// `HttpResult` cookie read and the test scaffold's `as any` (typing choices), and the named
/// widening [`super::internal_fault::unnameable_receiver_type_arg`] (#2795).
pub(super) fn any_ty() -> TsType {
    TsType::named("any")
}

/// What every arm reads: the call, its receiver and arguments, and the element type as a node.
struct ListArm<'a> {
    e: &'a Expr,
    receiver: &'a Expr,
    args: &'a [Expr],
    elem: TsType,
}

// ---- Helpers over `crate::ts_build`, shared by the arms and the sibling kernels ----

/// The `N` children of an `N`-child builder, in order.
pub(super) fn take<const N: usize, T>(children: Vec<T>) -> [T; N] {
    children
        .try_into()
        .unwrap_or_else(|_| unreachable!("an {N}-child builder is given exactly {N} children"))
}

/// `(recv).method(args)`: the receiver is always bracketed, as the text forms did. This is the
/// one builder whose tree differs from [`crate::ts_build::method_call`], which leaves its
/// receiver bare, so it keeps its own name and the arms that need the brackets say so.
pub(super) fn bracketed_method_call(recv: TsExpr, method: &str, args: Vec<TsExpr>) -> TsExpr {
    method_call(paren(recv), method, args)
}

/// `(f)(x)`: a Bynk function value applied to one argument.
pub(super) fn applied(f: TsExpr, x: TsExpr) -> TsExpr {
    call(paren(f), vec![x])
}

/// An immediately-invoked arrow: the kernels' way of naming the receiver once.
fn iife(params: Vec<TsParam>, body: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    call(arrow(params, body), args)
}

/// An immediately-invoked block-bodied arrow. `is_async` is the arm's own: an
/// `await` in its body needs it, and the call is not awaited here (the
/// surrounding `<-` bind awaits the promise).
pub(super) fn iife_block(
    params: Vec<TsParam>,
    is_async: bool,
    body: Vec<TsStmt>,
    args: Vec<TsExpr>,
) -> TsExpr {
    call(block_arrow(params, is_async, body), args)
}

// ---- Statements ----

/// `await (f)(__x)`: an effectful step applied to the loop's element.
pub(super) fn awaited_apply(f: TsExpr) -> TsExpr {
    TsExpr::Await(Box::new(applied(f, ident("__x"))))
}

/// `(a: A, b: B) => R`: a function type whose parameter names are part of its spelling.
pub(super) fn fn_type(params: &[(&str, &TsType)], ret: TsType) -> TsType {
    TsType::Fn {
        rest: false,
        params: params.iter().map(|(_, t)| (*t).clone()).collect(),
        names: params.iter().map(|(n, _)| (*n).to_string()).collect(),
        ret: Box::new(ret),
    }
}

/// `globalThis.Promise<T>`.
pub(super) fn promise(t: TsType) -> TsType {
    TsType::named_with_args("globalThis.Promise", vec![t])
}

pub(super) fn zero() -> TsExpr {
    num_lit("0".to_string())
}

/// `readonly E[]`: the array type of a list parameter.
fn xs_ty(elem: &TsType) -> TsType {
    TsType::readonly_array(elem.clone())
}

/// `(__xs: readonly E[])`: the single list parameter most arms close over.
fn xs_param(elem: &TsType) -> TsParam {
    TsParam::new("__xs", Some(xs_ty(elem)))
}

/// `globalThis.Math.max(0, n)`.
pub(super) fn clamped_at_zero(n: TsExpr) -> TsExpr {
    call(
        member(member(ident("globalThis"), "Math"), "max"),
        vec![zero(), n],
    )
}

/// `[...xs].sort((__a: E, __b: E) => body)`.
fn sorted_copy(xs: TsExpr, elem: &TsType, body: TsExpr) -> TsExpr {
    let pair = arrow(
        vec![
            TsParam::new("__a", Some(elem.clone())),
            TsParam::new("__b", Some(elem.clone())),
        ],
        body,
    );
    method_sort(TsExpr::array(vec![spread(xs)]), pair)
}

/// `array.sort(compare)`: the array is not bracketed, so `[...xs].sort(…)` prints bare.
pub(super) fn method_sort(array: TsExpr, compare: TsExpr) -> TsExpr {
    call(member(array, "sort"), vec![compare])
}

/// The comparator when it is a bare identifier (a runtime comparator or an
/// in-scope witness), which is then substituted directly rather than bound.
fn bare_comparator(cmp: &TsExpr) -> Option<&TsExpr> {
    // One shape is asked for: a plain identifier.
    if let TsExpr::Ident(name) = cmp
        && name.chars().all(|c| c.is_alphanumeric() || c == '_')
    {
        Some(cmp)
    } else {
        None
    }
}

/// v0.20b: lower a built-in `List` kernel method. Returns None for a method
/// name the kernel doesn't own (the checker has already rejected it; this
/// keeps the dispatch defensive). All forms are pure expressions; `foldEff`
/// returns a Promise that the surrounding `<-` bind awaits. Every arm builds a node,
/// the statement-bodied ones as block-bodied arrows (1A.18b). Anything an
/// operand hoists lands in `pre`.
pub(super) fn lower_list_kernel(
    e: &Expr,
    receiver: &Expr,
    op: ListOp,
    args: &[Expr],
    elem: TyId,
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    let tys = cx.ty_render();
    let a = ListArm {
        e,
        receiver,
        args,
        elem: ts_ty_to_ts_type(elem, tys),
    };
    let p = pre;
    let built: Option<bynk_ts::TsExpr> = match (op, args) {
        (ListOp::Length | ListOp::Count, []) => Some(list_length(&a, p, cx)),
        (ListOp::Get, [index]) => Some(list_get(&a, index, p, cx)),
        (ListOp::Prepend, [head]) => Some(list_prepend(&a, head, p, cx)),
        (ListOp::Append, [item]) => Some(list_append(&a, item, p, cx)),
        (ListOp::Concat, [other]) => Some(list_concat(&a, other, p, cx)),
        (ListOp::Fold, [init, f]) => Some(list_fold(&a, init, f, p, cx)),
        (ListOp::FoldEff, [init, f]) => Some(list_fold_eff(&a, init, f, p, cx)),
        (ListOp::ForEach, [f]) => Some(list_for_each(&a, f, p, cx)),
        (ListOp::ParForEach, [f]) => Some(list_par_traverse(&a, f, p, cx)),
        (ListOp::TraverseAll | ListOp::Traverse, [f]) => Some(list_traverse_all(&a, f, p, cx)),
        (ListOp::ParTraverseAll | ListOp::ParTraverse, [f]) => {
            Some(list_par_traverse_all(&a, f, p, cx))
        }
        (name @ (ListOp::SequenceAll | ListOp::SequenceTry), []) => {
            Some(list_sequence(&a, name, p, cx))
        }
        (ListOp::TraverseTry, [f]) => Some(list_traverse_try(&a, f, p, cx)),
        (ListOp::ParTraverseTry, [f]) => Some(list_par_traverse_try(&a, f, p, cx)),
        (ListOp::Map, [f]) => Some(list_callback(&a, "map", f, p, cx)),
        (ListOp::Filter, [f]) => Some(list_callback(&a, "filter", f, p, cx)),
        (ListOp::FlatMap, [f]) => Some(list_callback(&a, "flatMap", f, p, cx)),
        (ListOp::Any, [f]) => Some(list_callback(&a, "some", f, p, cx)),
        (ListOp::All, [f]) => Some(list_callback(&a, "every", f, p, cx)),
        (ListOp::Take, [n]) => Some(list_take(&a, n, p, cx)),
        (ListOp::Skip, [n]) => Some(list_skip(&a, n, p, cx)),
        (ListOp::UpTo, [step]) => Some(list_up_to(&a, step, p, cx)),
        (ListOp::First, []) => Some(list_first(&a, p, cx)),
        (ListOp::FirstOrElse, [default]) => Some(list_first_or_else(&a, default, p, cx)),
        (ListOp::SortBy, [key]) => Some(list_sort_by(&a, key, false, p, cx)),
        (ListOp::SortByDescending, [key]) => Some(list_sort_by(&a, key, true, p, cx)),
        (ListOp::Distinct, []) if !key_is_plain(elem, tys.types()) => {
            Some(list_keyed(&a, "__bynkDistinct", p, cx))
        }
        (ListOp::DistinctBy, [key]) if !key_fn_is_plain(key, cx) => {
            Some(list_keyed(&a, "__bynkDistinctBy", p, cx))
        }
        (ListOp::JoinOn, [_, left, ..]) if !key_fn_is_plain(left, cx) => {
            Some(list_keyed(&a, "__bynkJoinOn", p, cx))
        }
        (ListOp::LeftJoin, [_, left, ..]) if !key_fn_is_plain(left, cx) => {
            Some(list_keyed(&a, "__bynkLeftJoin", p, cx))
        }
        (ListOp::GroupBy, [key, _]) if !key_fn_is_plain(key, cx) => {
            Some(list_keyed(&a, "__bynkGroupBy", p, cx))
        }
        (ListOp::Distinct, []) => Some(list_distinct(&a, p, cx)),
        (ListOp::DistinctBy, [key]) => Some(list_distinct_by(&a, key, p, cx)),
        (ListOp::Sum, [key]) => Some(list_sum(&a, key, p, cx)),
        (ListOp::Min | ListOp::Max, [key]) => Some(list_min_max(&a, op, key, p, cx)),
        (
            ListOp::Reverse
            | ListOp::Find
            | ListOp::Contains
            | ListOp::Last
            | ListOp::LastOrElse
            | ListOp::IsEmpty
            | ListOp::Sort
            | ListOp::Min
            | ListOp::Max,
            _,
        ) => basics::lower_list_basic(&a, op, elem, p, cx),
        (ListOp::Average, [key]) => Some(list_average(&a, key, p, cx)),
        (ListOp::JoinOn, [other, left, right, into]) => {
            Some(list_join_on(&a, false, other, left, right, into, p, cx))
        }
        (ListOp::LeftJoin, [other, left, right, into]) => {
            Some(list_join_on(&a, true, other, left, right, into, p, cx))
        }
        (ListOp::Join, [other, on, into]) => Some(list_join(&a, other, on, into, p, cx)),
        (ListOp::GroupBy, [key, into]) => Some(list_group_by(&a, key, into, p, cx)),
        _ => None,
    };
    built
}

fn list_length(a: &ListArm, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    {
        let n = vec![recv];
        let [recv] = take(n);
        member(paren(recv), "length")
    }
}

fn list_get(a: &ListArm, index: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let idx = pre.lower_node(index, cx);
    {
        let n = vec![recv, idx];
        let [recv, idx] = take(n);
        let in_range = binary(
            TsBinaryOp::And,
            binary(TsBinaryOp::GreaterThanEq, ident("__i"), zero()),
            binary(
                TsBinaryOp::LessThan,
                ident("__i"),
                member(ident("__xs"), "length"),
            ),
        );
        let item = TsExpr::As {
            expr: Box::new(TsExpr::Index {
                object: Box::new(ident("__xs")),
                index: Box::new(ident("__i")),
            }),
            ty: a.elem.clone(),
        };
        let body = TsExpr::Conditional {
            test: Box::new(in_range),
            consequent: Box::new(call(ident("Some"), vec![item])),
            alternate: Box::new(ident("None")),
        };
        iife(
            vec![
                xs_param(&a.elem),
                TsParam::new("__i", Some(TsType::named("number"))),
            ],
            body,
            vec![recv, idx],
        )
    }
}

fn list_prepend(a: &ListArm, head: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let head = pre.lower_node(head, cx);
    let recv = pre.lower_node(a.receiver, cx);
    {
        let n = vec![head, recv];
        let [head, recv] = take(n);
        TsExpr::array(vec![head, spread(recv)])
    }
}

// #1889: the receiver lowers before the argument, as written, so any
// hoisted effects keep source order.
fn list_append(a: &ListArm, item: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let item = pre.lower_node(item, cx);
    {
        let n = vec![recv, item];
        let [recv, item] = take(n);
        TsExpr::array(vec![spread(recv), item])
    }
}

fn list_concat(a: &ListArm, other: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let other = pre.lower_node(other, cx);
    {
        let n = vec![recv, other];
        let [recv, other] = take(n);
        TsExpr::array(vec![spread(recv), spread(other)])
    }
}

fn list_fold(
    a: &ListArm,
    init: &Expr,
    f: &Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let e = a.e;
    let tys = cx.ty_render();
    // The call's checked type is the accumulator type.
    let acc = cx
        .commons()
        .expr_types
        .get(&e.id)
        .map(|te| ts_ty_to_ts_type(te.ty, tys))
        .unwrap_or_else(|| cx.missing_fact_ty(e.span, "type for a `fold` call"));
    let recv = pre.lower_node(a.receiver, cx);
    let init = pre.lower_node(init, cx);
    let f = pre.lower_node(f, cx);
    {
        let n = vec![recv, init, f];
        let [recv, init, f] = take(n);
        let step = fn_type(&[("acc", &acc), ("x", &a.elem)], acc.clone());
        let body = vec![
            for_of(
                "__x",
                ident("__xs"),
                assign(
                    ident("__acc"),
                    call(ident("__f"), vec![ident("__acc"), ident("__x")]),
                ),
            ),
            ret(ident("__acc")),
        ];
        iife_block(
            vec![
                xs_param(&a.elem),
                TsParam::new("__acc", Some(acc.clone())),
                TsParam::new("__f", Some(step)),
            ],
            false,
            body,
            vec![recv, init, f],
        )
    }
}

fn list_fold_eff(
    a: &ListArm,
    init: &Expr,
    f: &Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let e = a.e;
    let tys = cx.ty_render();
    // The call's checked type is `Effect[Acc]` — peel for the TS
    // accumulator annotation.
    let acc = match cx.commons().expr_ty(e.id).as_deref() {
        Some(Ty::Effect(acc)) => ts_ty_to_ts_type(*acc, tys),
        Some(_) => ts_ty_to_ts_type(cx.commons().expr_types[&e.id].ty, tys),
        None => cx.missing_fact_ty(e.span, "type for a `foldEff` call"),
    };
    let recv = pre.lower_node(a.receiver, cx);
    let init = pre.lower_node(init, cx);
    let f = pre.lower_node(f, cx);
    {
        let n = vec![recv, init, f];
        let [recv, init, f] = take(n);
        let step = fn_type(&[("acc", &acc), ("x", &a.elem)], promise(acc.clone()));
        let awaited = TsExpr::Await(Box::new(call(
            ident("__f"),
            vec![ident("__acc"), ident("__x")],
        )));
        let body = vec![
            for_of("__x", ident("__xs"), assign(ident("__acc"), awaited)),
            ret(ident("__acc")),
        ];
        iife_block(
            vec![
                xs_param(&a.elem),
                TsParam::new("__acc", Some(acc.clone())),
                TsParam::new("__f", Some(step)),
            ],
            true,
            body,
            vec![recv, init, f],
        )
    }
}

// v0.146 (ADR 0170): `forEach` — run an effectful step per element in
// order, awaiting each; yields `Promise<void>`. The eager `List`
// analogue of the `Query.forEach` terminal, emitted inline.
fn list_for_each(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    {
        let n = vec![recv, f];
        let [recv, f] = take(n);
        let each = block(vec![expr_stmt(awaited_apply(f))]);
        iife_block(
            vec![xs_param(&a.elem)],
            true,
            vec![for_of("__x", ident("__xs"), each)],
            vec![recv],
        )
    }
}

// v0.147 (ADR 0171): `parForEach` — issue the effectful fn over every
// element concurrently and await them together, so one slow element does
// not head-of-line-block the rest. The eager `List` analogue of the
// `Query.parTraverse` terminal, emitted inline.
fn list_par_traverse(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    {
        let n = vec![recv, f];
        let [recv, f] = take(n);
        let settled = TsExpr::Await(Box::new(call(
            ident("__bynkSettleAll"),
            vec![settle_thunks(a, f)],
        )));
        iife_block(
            vec![xs_param(&a.elem)],
            true,
            vec![expr_stmt(settled)],
            vec![recv],
        )
    }
}

// v0.148 (ADR 0172): the collect-all iterators — every element's
// `Result` outcome is gathered (an `Err` is a value, so nothing rejects).
// `traverseAll` awaits each in order into a typed array; `parTraverseAll`
// issues all at once and `Promise.all`s the resolved `Result`s. Both
// yield `Promise<Result<…>[]>`.
fn list_traverse_all(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let e = a.e;
    let tys = cx.ty_render();
    // The call's checked type is `Effect[List[Result[B, E]]]` — peel to
    // the `Result[B, E]` TS for the output-array annotation.
    let res = match cx
        .commons()
        .expr_ty(e.id)
        .as_deref()
        .and_then(Ty::as_effect)
    {
        Some(inner) => ts_ty_to_ts_type(tys.get(inner).as_list().unwrap_or(inner), tys),
        None => cx.missing_fact_ty(e.span, "`Effect[List[_]]` type for a `traverseAll` call"),
    };
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    {
        let n = vec![recv, f];
        let [recv, f] = take(n);
        let push = expr_stmt(method_call(ident("__out"), "push", vec![awaited_apply(f)]));
        let body = vec![
            const_typed(
                "__out",
                Some(TsType::array(res.clone())),
                TsExpr::array(vec![]),
            ),
            for_of("__x", ident("__xs"), block(vec![push])),
            ret(ident("__out")),
        ];
        iife_block(vec![xs_param(&a.elem)], true, body, vec![recv])
    }
}

fn list_par_traverse_all(
    a: &ListArm,
    f: &Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    {
        let n = vec![recv, f];
        let [recv, f] = take(n);
        let settled = TsExpr::Await(Box::new(call(
            ident("__bynkSettleAll"),
            vec![settle_thunks(a, f)],
        )));
        call(
            arrow_with(vec![xs_param(&a.elem)], true, settled),
            vec![recv],
        )
    }
}

/// `__xs.map((__x: E) => () => (f)(__x))`: one thunk per element, for `__bynkSettleAll`.
fn settle_thunks(a: &ListArm, f: TsExpr) -> TsExpr {
    call(
        member(ident("__xs"), "map"),
        vec![arrow(
            vec![TsParam::new("__x", Some(a.elem.clone()))],
            arrow(vec![], applied(f, ident("__x"))),
        )],
    )
}

// #2806: pure `Result` accumulation. The call's checked type is
// `Result[List[T], E']`; `T` and `E` (the element's own `Err` type) type
// the accumulators.
fn list_sequence(a: &ListArm, name: ListOp, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let e = a.e;
    let tys = cx.ty_render();
    let (ok, err) = match cx
        .commons()
        .expr_ty(e.id)
        .as_deref()
        .and_then(Ty::as_result)
    {
        Some((ok, err)) => {
            let ok_ty = ts_ty_to_ts_type(tys.get(ok).as_list().unwrap_or(ok), tys);
            let err_elem = if name == ListOp::SequenceAll {
                tys.get(err).as_list().unwrap_or(err)
            } else {
                err
            };
            (ok_ty, ts_ty_to_ts_type(err_elem, tys))
        }
        None => {
            let missing = cx.missing_fact_ty(e.span, "`Result` type for a `sequence` call");
            (missing.clone(), missing)
        }
    };
    let recv = pre.lower_node(a.receiver, cx);
    {
        let n = vec![recv];
        let [recv] = take(n);
        let ok_decl = const_typed(
            "__ok",
            Some(TsType::array(ok.clone())),
            TsExpr::array(vec![]),
        );
        let is_err = result_is_err("__r");
        let body = if name == ListOp::SequenceAll {
            let collect = TsStmt::if_else_same_line_stmt(
                is_err,
                block(vec![expr_stmt(method_call(
                    ident("__errs"),
                    "push",
                    vec![member(ident("__r"), "error")],
                ))]),
                block(vec![expr_stmt(method_call(
                    ident("__ok"),
                    "push",
                    vec![member(ident("__r"), "value")],
                ))]),
                None,
            );
            let outcome = TsExpr::Conditional {
                test: Box::new(binary(
                    TsBinaryOp::GreaterThan,
                    member(ident("__errs"), "length"),
                    zero(),
                )),
                consequent: Box::new(call(ident("Err"), vec![ident("__errs")])),
                alternate: Box::new(call(ident("Ok"), vec![ident("__ok")])),
            };
            vec![
                ok_decl,
                const_typed(
                    "__errs",
                    Some(TsType::array(err.clone())),
                    TsExpr::array(vec![]),
                ),
                for_of("__r", ident("__xs"), block(vec![collect])),
                ret(outcome),
            ]
        } else {
            let bail = TsStmt::if_stmt(
                is_err,
                block(vec![ret(call(
                    ident("Err"),
                    vec![member(ident("__r"), "error")],
                ))]),
                None,
            );
            let push = expr_stmt(method_call(
                ident("__ok"),
                "push",
                vec![member(ident("__r"), "value")],
            ));
            vec![
                ok_decl,
                for_of("__r", ident("__xs"), block(vec![bail, push])),
                ret(call(ident("Ok"), vec![ident("__ok")])),
            ]
        };
        iife_block(vec![xs_param(&a.elem)], false, body, vec![recv])
    }
}

// v0.150 (ADR 0174): the short-circuit collect iterators — stop at the
// first `Err` and return it (`Effect[Result[List[U], E]]`). `traverseTry`
// awaits each in order, bailing on the first `Err`; `parTraverseTry`
// issues all at once, then scans the resolved `Result`s in input order.
fn list_traverse_try(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let e = a.e;
    let tys = cx.ty_render();
    let u = list_ok_elem(
        cx.commons().expr_types.get(&e.id).map(|te| te.ty),
        tys,
        cx.fault_site(e.span),
    );
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    {
        let n = vec![recv, f];
        let [recv, f] = take(n);
        let step = vec![
            const_typed("__r", None, awaited_apply(f)),
            bail_on_err(),
            push_value(),
        ];
        let body = vec![
            const_typed(
                "__out",
                Some(TsType::array(u.clone())),
                TsExpr::array(vec![]),
            ),
            for_of("__x", ident("__xs"), block(step)),
            ret(call(ident("Ok"), vec![ident("__out")])),
        ];
        iife_block(vec![xs_param(&a.elem)], true, body, vec![recv])
    }
}

fn list_par_traverse_try(
    a: &ListArm,
    f: &Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let e = a.e;
    let tys = cx.ty_render();
    let u = list_ok_elem(
        cx.commons().expr_types.get(&e.id).map(|te| te.ty),
        tys,
        cx.fault_site(e.span),
    );
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    {
        let n = vec![recv, f];
        let [recv, f] = take(n);
        let settled = TsExpr::Await(Box::new(call(
            ident("__bynkSettleAll"),
            vec![settle_thunks(a, f)],
        )));
        let scan = vec![bail_on_err(), push_value()];
        let body = vec![
            const_typed("__rs", None, settled),
            const_typed(
                "__out",
                Some(TsType::array(u.clone())),
                TsExpr::array(vec![]),
            ),
            for_of("__r", ident("__rs"), block(scan)),
            ret(call(ident("Ok"), vec![ident("__out")])),
        ];
        iife_block(vec![xs_param(&a.elem)], true, body, vec![recv])
    }
}

/// `__r.tag === "Err"`.
fn result_is_err(result: &str) -> TsExpr {
    binary(
        TsBinaryOp::StrictEq,
        member(ident(result), "tag"),
        TsExpr::Lit(TsLit::Str("Err".to_string())),
    )
}

/// `if (__r.tag === "Err") { return Err(__r.error); }`.
pub(super) fn bail_on_err() -> TsStmt {
    TsStmt::if_stmt(
        result_is_err("__r"),
        block(vec![ret(call(
            ident("Err"),
            vec![member(ident("__r"), "error")],
        ))]),
        None,
    )
}

/// `__out.push(__r.value);`.
pub(super) fn push_value() -> TsStmt {
    expr_stmt(method_call(
        ident("__out"),
        "push",
        vec![member(ident("__r"), "value")],
    ))
}

// v0.88 (ADR 0116): the eager builder/terminal vocabulary. Most lower
// to native array methods; callbacks are wrapped in a single-arg arrow
// so the array index/array extra args never reach a Bynk one-param fn.
// `map`, `filter`, `flatMap`, `any` (`some`) and `all` (`every`) share this.
fn list_callback(
    a: &ListArm,
    method: &str,
    f: &Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    {
        let n = vec![recv, f];
        let [recv, f] = take(n);
        let each = arrow(
            vec![TsParam::new("__x", Some(a.elem.clone()))],
            applied(f, ident("__x")),
        );
        bracketed_method_call(recv, method, vec![each])
    }
}

fn list_take(a: &ListArm, n: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let n = pre.lower_node(n, cx);
    {
        let c = vec![recv, n];
        let [recv, n] = take(c);
        bracketed_method_call(recv, "slice", vec![zero(), clamped_at_zero(n)])
    }
}

fn list_skip(a: &ListArm, n: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let n = pre.lower_node(n, cx);
    {
        let c = vec![recv, n];
        let [recv, n] = take(c);
        bracketed_method_call(recv, "slice", vec![clamped_at_zero(n)])
    }
}

// v0.119 (ADR 0155, DECISION C-a): `run.upTo(step)` — the driven history
// strictly before `step`. Steps are distinct object instances in the run
// array, so `indexOf` is reference identity. An IIFE avoids re-evaluating
// the receiver.
fn list_up_to(a: &ListArm, step: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let step = pre.lower_node(step, cx);
    {
        let n = vec![recv, step];
        let [recv, step] = take(n);
        let index = call(member(ident("__xs"), "indexOf"), vec![ident("__s")]);
        iife(
            vec![xs_param(&a.elem), TsParam::new("__s", Some(a.elem.clone()))],
            call(member(ident("__xs"), "slice"), vec![zero(), index]),
            vec![recv, step],
        )
    }
}

fn list_first(a: &ListArm, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    {
        let n = vec![recv];
        let [recv] = take(n);
        let body = TsExpr::Conditional {
            test: Box::new(binary(
                TsBinaryOp::GreaterThan,
                member(ident("__xs"), "length"),
                zero(),
            )),
            consequent: Box::new(call(ident("Some"), vec![first_item()])),
            alternate: Box::new(ident("None")),
        };
        iife(vec![xs_param(&a.elem)], body, vec![recv])
    }
}

/// `__xs[0]`.
fn first_item() -> TsExpr {
    TsExpr::Index {
        object: Box::new(ident("__xs")),
        index: Box::new(zero()),
    }
}

fn list_first_or_else(
    a: &ListArm,
    default: &Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let default = pre.lower_node(default, cx);
    {
        let n = vec![recv, default];
        let [recv, default] = take(n);
        let body = TsExpr::Conditional {
            test: Box::new(binary(
                TsBinaryOp::GreaterThan,
                member(ident("__xs"), "length"),
                zero(),
            )),
            consequent: Box::new(first_item()),
            alternate: Box::new(ident("__d")),
        };
        iife(
            vec![xs_param(&a.elem), TsParam::new("__d", Some(a.elem.clone()))],
            body,
            vec![recv, default],
        )
    }
}

/// The `distinctBy` kernel over `iter`: first-seen order, keyed by `key` (shared with the
/// `Query` vocabulary, whose iterable is `__q`).
pub(super) fn distinct_by_body(elem: &TsType, iter: TsExpr, key: TsExpr) -> Vec<TsStmt> {
    let seen = TsExpr::New {
        callee: Box::new(member(ident("globalThis"), "Set")),
        args: vec![],
    };
    let fresh = TsStmt::if_stmt(
        TsExpr::Unary {
            op: TsUnaryOp::Not,
            expr: Box::new(method_call(ident("__seen"), "has", vec![ident("__k")])),
        },
        block(vec![
            expr_stmt(method_call(ident("__seen"), "add", vec![ident("__k")])),
            expr_stmt(method_call(ident("__out"), "push", vec![ident("__x")])),
        ]),
        None,
    );
    vec![
        const_typed("__seen", None, seen),
        const_typed(
            "__out",
            Some(TsType::array(elem.clone())),
            TsExpr::array(vec![]),
        ),
        for_of(
            "__x",
            iter,
            block(vec![
                const_typed("__k", None, applied(key, ident("__x"))),
                fresh,
            ]),
        ),
        ret(ident("__out")),
    ]
}

// v0.88 (ADR 0116 D2/D3/D4): ordering + aggregates. The comparator
// `<`/`>` works for the numeric- and string-erased orderable keys
// alike, so no key-type branch is needed (except average's rounding).
// #2805: `sortByDescending` is the exact reverse of `sortBy`'s order
// (the comparator's arguments swapped, so `NaN` is first), and stable.
fn list_sort_by(
    a: &ListArm,
    key: &Expr,
    descending: bool,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    // The comparator is bound before the receiver and key lower: `note_cmp` fires first.
    let cmp = kernel_key_comparator(&a.args[0], cx);
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    let Some(cmp) = cmp else {
        return {
            let n = vec![recv, key];
            let [recv, key] = take(n);
            sort_plain_node(a, descending, recv, key)
        };
    };
    {
        let n = vec![recv, key];
        let [recv, key] = take(n);
        sort_node(a, descending, &cmp, recv, key)
    }
}

/// The argument order of the key comparison: swapped for `sortByDescending`.
pub(super) fn sort_order(descending: bool) -> (&'static str, &'static str) {
    if descending {
        ("__b", "__a")
    } else {
        ("__a", "__b")
    }
}

/// The comparator forms as nodes: a bare comparator substituted directly, or a
/// built one bound as `__c` beside the receiver (`__r`).
fn sort_node(a: &ListArm, descending: bool, cmp: &TsExpr, recv: TsExpr, key: TsExpr) -> TsExpr {
    let (first, second) = sort_order(descending);
    let compare = |c: TsExpr, xs: TsExpr| {
        let body = call(
            c,
            vec![
                applied(key.clone(), ident(first)),
                applied(key.clone(), ident(second)),
            ],
        );
        sorted_copy(xs, &a.elem, body)
    };
    match bare_comparator(cmp) {
        Some(cmp) => compare(cmp.clone(), recv),
        None => iife(
            vec![
                TsParam::new("__c", None),
                TsParam::new("__r", Some(xs_ty(&a.elem))),
            ],
            compare(ident("__c"), ident("__r")),
            vec![cmp.clone(), recv],
        ),
    }
}

/// The plain-key form as a node: the bare operators over two bound keys.
fn sort_plain_node(a: &ListArm, descending: bool, recv: TsExpr, key: TsExpr) -> TsExpr {
    sort_plain_by(Some(&a.elem), descending, recv, key)
}

/// [`sort_plain_node`] over a copy of `recv`; the pair's parameters are annotated with `elem`
/// when it is known (the `Query` vocabulary's are not).
pub(super) fn sort_plain_by(
    elem: Option<&TsType>,
    descending: bool,
    recv: TsExpr,
    key: TsExpr,
) -> TsExpr {
    let (lo, hi) = if descending { ("1", "-1") } else { ("-1", "1") };
    let ordered = |op, then: &str, otherwise: TsExpr| TsExpr::Conditional {
        test: Box::new(binary(op, ident("__ka"), ident("__kb"))),
        consequent: Box::new(num_lit(then.to_string())),
        alternate: Box::new(otherwise),
    };
    let outcome = ordered(
        TsBinaryOp::LessThan,
        lo,
        ordered(TsBinaryOp::GreaterThan, hi, zero()),
    );
    let body = vec![
        const_typed("__ka", None, applied(key.clone(), ident("__a"))),
        const_typed("__kb", None, applied(key, ident("__b"))),
        ret(outcome),
    ];
    let pair = block_arrow(
        vec![
            TsParam::new("__a", elem.cloned()),
            TsParam::new("__b", elem.cloned()),
        ],
        false,
        body,
    );
    method_sort(TsExpr::array(vec![spread(recv)]), pair)
}

/// `distinct`, `distinctBy`, `joinOn`, `leftJoin` and `groupBy` over a key that
/// is not a JS primitive: the `keys` runtime helper `helper` takes the
/// receiver, then each remaining argument in order. `note_key` fires before
/// anything lowers.
fn list_keyed(
    a: &ListArm,
    helper: &'static str,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    cx.runtime_use().note_key(helper);
    let mut children = vec![pre.lower_node(a.receiver, cx)];
    children.extend(a.args.iter().map(|x| pre.lower_node(x, cx)));
    {
        let n = children;
        call(ident(helper), n)
    }
}

fn list_distinct(a: &ListArm, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    {
        let n = vec![recv];
        let [recv] = take(n);
        let set = TsExpr::New {
            callee: Box::new(member(ident("globalThis"), "Set")),
            args: vec![recv],
        };
        TsExpr::array(vec![spread(set)])
    }
}

fn list_distinct_by(a: &ListArm, key: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    {
        let n = vec![recv, key];
        let [recv, key] = take(n);
        let body = distinct_by_body(&a.elem, ident("__xs"), key);
        iife_block(vec![xs_param(&a.elem)], false, body, vec![recv])
    }
}

fn list_sum(a: &ListArm, key: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let tys = cx.ty_render();
    let decimal = key_fn_ret(key, cx).and_then(|t| t.base(tys.types())) == Some(BaseType::Decimal);
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    if decimal {
        // #2622: an exact sum, from the zero decimal.
        cx.note_decimal("__bynkDecimalAdd");
    }
    {
        let n = vec![recv, key];
        let [recv, key] = take(n);
        let (acc_ty, add, init) = if decimal {
            (
                TsType::named(crate::emitter::ts_types::DECIMAL_TS_TYPE),
                call(
                    ident("__bynkDecimalAdd"),
                    vec![ident("__s"), applied(key, ident("__x"))],
                ),
                decimal_lit("0", 0),
            )
        } else {
            (
                TsType::named("number"),
                binary(TsBinaryOp::Add, ident("__s"), applied(key, ident("__x"))),
                zero(),
            )
        };
        let step = arrow(
            vec![
                TsParam::new("__s", Some(acc_ty)),
                TsParam::new("__x", Some(a.elem.clone())),
            ],
            add,
        );
        bracketed_method_call(recv, "reduce", vec![step, init])
    }
}

fn list_min_max(
    a: &ListArm,
    op: ListOp,
    key: &Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let cmp_op = if op == ListOp::Min {
        TsBinaryOp::LessThan
    } else {
        TsBinaryOp::GreaterThan
    };
    let key_cmp = kernel_key_comparator(&a.args[0], cx);
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    let [recv, key] = take(vec![recv, key]);
    min_max_node(a, cmp_op, key_cmp, recv, key)
}

/// The `min`/`max` scan over lowered operands: `key_cmp` is the key's comparator
/// when it is not plain, `cmp_op` the operator that prefers the new key.
fn min_max_node(
    a: &ListArm,
    cmp_op: TsBinaryOp,
    key_cmp: Option<TsExpr>,
    recv: TsExpr,
    key: TsExpr,
) -> TsExpr {
    {
        {
            // The kernel over `__xs`, ordering by `better(__k, __m)`.
            let kernel = |better: &dyn Fn() -> TsExpr| {
                let take_key = TsStmt::if_stmt(better(), assign(ident("__m"), ident("__k")), None);
                let scan = block(vec![
                    const_typed("__k", None, applied(key.clone(), ident("__x"))),
                    take_key,
                ]);
                vec![
                    TsStmt::if_stmt(
                        binary(
                            TsBinaryOp::StrictEq,
                            member(ident("__xs"), "length"),
                            zero(),
                        ),
                        ret(ident("None")),
                        None,
                    ),
                    let_init("__m", applied(key.clone(), first_item())),
                    for_of("__x", ident("__xs"), scan),
                    ret(call(ident("Some"), vec![ident("__m")])),
                ]
            };
            match &key_cmp {
                None => iife_block(
                    vec![xs_param(&a.elem)],
                    false,
                    kernel(&|| binary(cmp_op, ident("__k"), ident("__m"))),
                    vec![recv],
                ),
                Some(key_cmp) => with_comparator_node(key_cmp, |c| {
                    iife_block(
                        vec![xs_param(&a.elem)],
                        false,
                        kernel(&|| {
                            binary(
                                cmp_op,
                                call(c.clone(), vec![ident("__k"), ident("__m")]),
                                zero(),
                            )
                        }),
                        vec![recv],
                    )
                }),
            }
        }
    }
}

fn list_average(a: &ListArm, key: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    let e = a.e;
    let tys = cx.ty_render();
    // D3: Duration averages round to integer millis; Int/Float -> Float.
    let round = matches!(
        cx.commons().expr_ty(e.id).as_deref(),
        Some(Ty::Option(inner)) if matches!(&*tys.get(*inner), Ty::Base(BaseType::Duration))
    );
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    {
        let n = vec![recv, key];
        let [recv, key] = take(n);
        let ratio = binary(
            TsBinaryOp::Div,
            ident("__s"),
            member(ident("__xs"), "length"),
        );
        let mean = if round {
            call(
                member(member(ident("globalThis"), "Math"), "round"),
                vec![ratio],
            )
        } else {
            ratio
        };
        let add = expr_stmt(TsExpr::AddAssign {
            target: Box::new(ident("__s")),
            value: Box::new(applied(key, ident("__x"))),
        });
        let body = vec![
            TsStmt::if_stmt(
                binary(
                    TsBinaryOp::StrictEq,
                    member(ident("__xs"), "length"),
                    zero(),
                ),
                ret(ident("None")),
                None,
            ),
            let_init("__s", zero()),
            for_of("__x", ident("__xs"), add),
            ret(call(ident("Some"), vec![mean])),
        ];
        iife_block(vec![xs_param(&a.elem)], false, body, vec![recv])
    }
}

// v0.94 (ADR 0116/0120): joins & grouping. Hash on a stringified key
// (value-keyable, like the `@indexed` posting list), probe, and project
// each result through `into` — there is no pair value. `joinOn`/`leftJoin`
// build the hash from `other`'s key; `join` is a nested-loop predicate;
// `groupBy` partitions in first-seen key order. Group/`into` receive the
// **original** key (re-derived from a representative row), not the
// stringified hash key.
#[allow(clippy::too_many_arguments)]
fn list_join_on(
    a: &ListArm,
    left_join: bool,
    other: &Expr,
    left: &Expr,
    right: &Expr,
    into: &Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let u = join_other_elem(a.args, cx);
    let other = pre.lower_node(other, cx);
    let left = pre.lower_node(left, cx);
    let right = pre.lower_node(right, cx);
    let into = pre.lower_node(into, cx);
    {
        let n = vec![recv, other, left, right, into];
        let [recv, other, left, right, into] = take(n);
        let body = join_body(
            &JoinSides {
                left_join,
                elem: &a.elem,
                other: &u,
            },
            other,
            [left, right, into],
            |probe| bracketed_method_call(recv, "flatMap", vec![probe]),
        );
        iife_block(vec![], false, body, vec![])
    }
}

/// What a keyed join is over: the receiver's element, the `other` side's element, and
/// whether unmatched rows survive (`leftJoin`).
#[derive(Clone, Copy)]
pub(super) struct JoinSides<'a> {
    pub left_join: bool,
    pub elem: &'a TsType,
    pub other: &'a TsType,
}

/// The hash-join kernel of `joinOn`/`leftJoin`: bucket `iter` (the `other` rows) by
/// `right`, then probe by `left` and project through `into`. `probed` wraps the probe arrow
/// in the receiver's own `flatMap` call (the list kernel brackets its receiver, the `Query`
/// vocabulary's is the bare `__q`).
pub(super) fn join_body(
    sides: &JoinSides,
    iter: TsExpr,
    [left, right, into]: [TsExpr; 3],
    probed: impl FnOnce(TsExpr) -> TsExpr,
) -> Vec<TsStmt> {
    let JoinSides {
        left_join,
        elem,
        other: u,
    } = *sides;
    // `const __h: Record<string, U[]> = Object.create(null)`, filled from `other`.
    let bucket = TsExpr::Assign {
        target: Box::new(hash_slot()),
        value: Box::new(binary(
            TsBinaryOp::NullishCoalescing,
            hash_slot(),
            TsExpr::array(vec![]),
        )),
    };
    let fill = block(vec![
        const_typed("__k", None, string_of(applied(right, ident("__u")))),
        expr_stmt(method_call(paren(bucket), "push", vec![ident("__u")])),
    ]);
    let matches = TsExpr::Index {
        object: Box::new(ident("__h")),
        index: Box::new(string_of(applied(left, ident("__t")))),
    };
    let found = const_typed(
        "__m",
        None,
        binary(
            TsBinaryOp::NullishCoalescing,
            matches,
            TsExpr::array(vec![]),
        ),
    );
    let project = |u_arg: TsExpr| {
        arrow(
            vec![TsParam::new("__u", Some(u.clone()))],
            call(paren(into.clone()), vec![ident("__t"), u_arg]),
        )
    };
    let result = if left_join {
        TsExpr::Conditional {
            test: Box::new(binary(
                TsBinaryOp::GreaterThan,
                member(ident("__m"), "length"),
                zero(),
            )),
            consequent: Box::new(method_call(
                ident("__m"),
                "map",
                vec![project(call(ident("Some"), vec![ident("__u")]))],
            )),
            alternate: Box::new(TsExpr::array(vec![call(
                paren(into.clone()),
                vec![ident("__t"), ident("None")],
            )])),
        }
    } else {
        method_call(ident("__m"), "map", vec![project(ident("__u"))])
    };
    let probe = block_arrow(
        vec![TsParam::new("__t", Some(elem.clone()))],
        false,
        vec![found, ret(result)],
    );
    vec![
        const_typed("__h", Some(record_of(u)), create_null()),
        for_of("__u", iter, fill),
        ret(probed(probe)),
    ]
}

/// `__h[__k]`.
pub(super) fn hash_slot() -> TsExpr {
    TsExpr::Index {
        object: Box::new(ident("__h")),
        index: Box::new(ident("__k")),
    }
}

/// `String(x)`: the stringified hash key.
pub(super) fn string_of(x: TsExpr) -> TsExpr {
    call(ident("String"), vec![x])
}

/// `globalThis.Record<string, E[]>`: the hash of buckets.
pub(super) fn record_of(elem: &TsType) -> TsType {
    TsType::named_with_args(
        "globalThis.Record",
        vec![TsType::named("string"), TsType::array(elem.clone())],
    )
}

/// `globalThis.Object.create(null)`.
pub(super) fn create_null() -> TsExpr {
    call(
        member(member(ident("globalThis"), "Object"), "create"),
        vec![TsExpr::Lit(TsLit::Null)],
    )
}

fn list_join(
    a: &ListArm,
    other: &Expr,
    on: &Expr,
    into: &Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let u = join_other_elem(a.args, cx);
    let other = pre.lower_node(other, cx);
    let on = pre.lower_node(on, cx);
    let into = pre.lower_node(into, cx);
    {
        let n = vec![recv, other, on, into];
        let [recv, other, on, into] = take(n);
        let pair = |f: TsExpr| call(paren(f), vec![ident("__t"), ident("__u")]);
        let each_u = |body: TsExpr| arrow(vec![TsParam::new("__u", Some(u.clone()))], body);
        let matched = method_call(ident("__b"), "filter", vec![each_u(pair(on))]);
        let joined = method_call(matched, "map", vec![each_u(pair(into))]);
        let flat = bracketed_method_call(
            recv,
            "flatMap",
            vec![arrow(
                vec![TsParam::new("__t", Some(a.elem.clone()))],
                joined,
            )],
        );
        let body = vec![const_typed("__b", Some(xs_ty(&u)), other), ret(flat)];
        iife_block(vec![], false, body, vec![])
    }
}

fn list_group_by(
    a: &ListArm,
    key: &Expr,
    into: &Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    let into = pre.lower_node(into, cx);
    {
        let n = vec![recv, key, into];
        let [recv, key, into] = take(n);
        let body = group_by_body(&a.elem, recv, key, into);
        iife_block(vec![], false, body, vec![])
    }
}

/// The `groupBy` kernel over `iter`: rows partitioned by `String(key)` in first-seen order,
/// each group handed to `into` with its original key.
pub(super) fn group_by_body(elem: &TsType, iter: TsExpr, key: TsExpr, into: TsExpr) -> Vec<TsStmt> {
    let first_seen = TsStmt::if_stmt(
        TsExpr::Unary {
            op: TsUnaryOp::Not,
            expr: Box::new(binary(TsBinaryOp::In, ident("__k"), ident("__h"))),
        },
        block(vec![
            assign(hash_slot(), TsExpr::array(vec![])),
            expr_stmt(method_call(ident("__order"), "push", vec![ident("__k")])),
        ]),
        None,
    );
    let partition = block(vec![
        const_typed("__k", None, string_of(applied(key.clone(), ident("__t")))),
        first_seen,
        expr_stmt(method_call(hash_slot(), "push", vec![ident("__t")])),
    ]);
    let rows = TsExpr::Index {
        object: Box::new(ident("__rows")),
        index: Box::new(zero()),
    };
    let emit = block_arrow(
        vec![TsParam::new("__k", None)],
        false,
        vec![
            const_typed("__rows", None, hash_slot()),
            ret(call(paren(into), vec![applied(key, rows), ident("__rows")])),
        ],
    );
    vec![
        const_typed("__h", Some(record_of(elem)), create_null()),
        const_typed(
            "__order",
            Some(TsType::array(TsType::named("string"))),
            TsExpr::array(vec![]),
        ),
        for_of("__t", iter, partition),
        ret(method_call(ident("__order"), "map", vec![emit])),
    ]
}

/// v0.94 (ADR 0120): the TS element type of a join's `other` collection (its
/// `List`/`Query` element), for typing the hash-map buckets. `unknown` when the
/// first argument is not a collection: every query method that takes an argument
/// computes this (the `QueryShape`'s `other_elem`), and only a join reads it, so
/// `unknown` there is a legitimate default and not a missing fact (1A.34 sweep).
pub(super) fn join_other_elem(args: &[Expr], cx: &LowerCtx) -> TsType {
    let tys = cx.ty_render();
    match cx.commons().expr_ty(args[0].id).as_deref() {
        Some(Ty::List(u) | Ty::Query(u)) => ts_ty_to_ts_type(*u, tys),
        _ => TsType::named("unknown"),
    }
}

/// The comparator an ordering kernel (`sortBy`/`min`/`max`) needs for its key
/// function's result type, or `None` when the key is a plain base type that keeps
/// the bare `<`/`>` (constrained-generics slice 1, DECISION H). `Float` is not
/// plain: the kernels use its total order (DECISION G), so `NaN` sorts last
/// instead of making the sort inconsistent. A key type the checker did not record
/// keeps the bare operators.
pub(super) fn kernel_key_comparator(key: &Expr, cx: &LowerCtx) -> Option<TsExpr> {
    let tys = cx.ty_render();
    let fn_ty = cx.commons().expr_types.get(&key.id)?.ty;
    let Ty::Fn { ret, .. } = &*tys.get(fn_ty) else {
        return None;
    };
    let plain = matches!(
        ret.base(tys.types()),
        Some(BaseType::Int | BaseType::Duration | BaseType::Instant)
    );
    (!plain).then(|| cx.comparator(*ret))
}

/// Constrained generics slice 2: whether a `Map` key / `Set` element of this
/// static type is a JS primitive (`string`, `number`, `boolean`), so the plain
/// `Map`/`Set` compares it by value. Every other `Key` type (`Bytes`, `Option`,
/// `List`, a record, a sum, a type parameter) takes the keyed helpers in
/// `keys.ts`; a plain-keyed site emits exactly what it did before.
pub(super) fn key_is_plain(ty: TyId, tys: &Types) -> bool {
    tys.get(ty)
        .scalar_base()
        .is_some_and(|b| !matches!(b, BaseType::Bytes | BaseType::Decimal))
}

/// The result type of a function-typed argument (a kernel's key function).
pub(super) fn key_fn_ret(key: &Expr, cx: &LowerCtx) -> Option<TyId> {
    let tys = cx.ty_render();
    let fn_ty = cx.commons().expr_types.get(&key.id)?.ty;
    let Ty::Fn { ret, .. } = &*tys.get(fn_ty) else {
        return None;
    };
    Some(*ret)
}

/// Whether a kernel keyed by the result of `key` hashes through the plain
/// `String(key)` path. An unknown type takes the helpers, which are correct for
/// every key.
pub(super) fn key_fn_is_plain(key: &Expr, cx: &LowerCtx) -> bool {
    key_fn_ret(key, cx).is_some_and(|t| key_is_plain(t, cx.commons().tys()))
}

/// [`with_comparator`] for a node body: a bare comparator is substituted, a built
/// one is bound as `((__c) => body)(cmp)`.
pub(super) fn with_comparator_node(cmp: &TsExpr, body: impl FnOnce(TsExpr) -> TsExpr) -> TsExpr {
    match bare_comparator(cmp) {
        Some(c) => body(c.clone()),
        None => call(
            arrow(vec![TsParam::new("__c", None)], body(ident("__c"))),
            vec![cmp.clone()],
        ),
    }
}

/// 1A.18a (#3719): the `List` kernel's expression-bodied arms build nodes.
#[cfg(test)]
mod list_kernel_tests {
    use crate::testkit::emit_source;

    /// The emitted TypeScript for a one-function commons.
    fn emitted(body: &str) -> String {
        emit_source(&format!("commons lk\n{body}\n"))
    }

    /// The returned expression of `export function f`.
    fn returned(ts: &str) -> String {
        let at = ts.find("export function f(").expect("f");
        let rest = &ts[at..];
        let ret = rest.find("  return ").expect("a return") + "  return ".len();
        let end = rest[ret..].find(";\n}\n").expect("end");
        rest[ret..ret + end].to_string()
    }

    /// Each arm over node operands prints exactly today's text.
    #[test]
    fn expression_arms_over_nodes_print_as_before() {
        for (body, want) in [
            ("fn f(xs: List[Int]) -> Int { xs.length() }", "(xs).length"),
            ("fn f(xs: List[Int]) -> Int { xs.count() }", "(xs).length"),
            (
                "fn f(xs: List[Int], i: Int) -> Option[Int] { xs.get(i) }",
                "((__xs: readonly number[], __i: number) => __i >= 0 && __i < __xs.length ? Some(__xs[__i] as number) : None)(xs, i)",
            ),
            (
                "fn f(xs: List[Int], y: List[Int]) -> List[Int] { xs.concat(y).append(1).prepend(0) }",
                "[0, ...[...[...xs, ...y], 1]]",
            ),
            (
                "fn f(xs: List[Int]) -> List[Int] { xs.map((x) => x + 1).filter((x) => x > 1).take(2).skip(1) }",
                "((((xs).map((__x: number) => ((x: number) => x + 1)(__x))).filter((__x: number) => ((x: number) => x > 1)(__x))).slice(0, globalThis.Math.max(0, 2))).slice(globalThis.Math.max(0, 1))",
            ),
            (
                "fn f(xs: List[Int]) -> Bool { xs.any((x) => x > 1) && xs.all((x) => x > 0) }",
                "(xs).some((__x: number) => ((x: number) => x > 1)(__x)) && (xs).every((__x: number) => ((x: number) => x > 0)(__x))",
            ),
            (
                "fn f(xs: List[Int]) -> List[Int] { xs.flatMap((x) => [x, x]) }",
                "(xs).flatMap((__x: number) => ((x: number) => [x, x])(__x))",
            ),
            (
                "fn f(xs: List[Int]) -> Option[Int] { xs.first() }",
                "((__xs: readonly number[]) => __xs.length > 0 ? Some(__xs[0]) : None)(xs)",
            ),
            (
                "fn f(xs: List[Int]) -> Int { xs.firstOrElse(0) }",
                "((__xs: readonly number[], __d: number) => __xs.length > 0 ? __xs[0] : __d)(xs, 0)",
            ),
            (
                "fn f(xs: List[Int]) -> Int { xs.sum((x) => x) }",
                "(xs).reduce((__s: number, __x: number) => __s + ((x: number) => x)(__x), 0)",
            ),
            (
                "fn f(xs: List[Int]) -> List[Int] { xs.distinct() }",
                "[...new globalThis.Set(xs)]",
            ),
            (
                "fn f(xs: List[Bytes]) -> List[Bytes] { xs.distinct() }",
                "__bynkDistinct(xs)",
            ),
            (
                "fn f(xs: List[Float]) -> List[Float] { xs.sortBy((x) => x) }",
                "[...xs].sort((__a: number, __b: number) => __bynkCmpFloat(((x: number) => x)(__a), ((x: number) => x)(__b)))",
            ),
            (
                "fn f(xs: List[Float]) -> List[Float] { xs.sortByDescending((x) => x) }",
                "[...xs].sort((__a: number, __b: number) => __bynkCmpFloat(((x: number) => x)(__b), ((x: number) => x)(__a)))",
            ),
        ] {
            let ts = emitted(body);
            assert_eq!(returned(&ts), want, "{body}");
        }
    }

    /// The `Decimal` sum starts from the zero decimal and notes its helper.
    #[test]
    fn a_decimal_sum_is_a_node_and_imports_its_helper() {
        let ts = emitted("fn f(xs: List[Decimal]) -> Decimal { xs.sum((x) => x) }");
        assert!(ts.contains("__bynkDecimalAdd(__s, ((x: { readonly coefficient: bigint; readonly scale: number }) => x)(__x)), { coefficient: 0n, scale: 0 })"), "{ts}");
        assert!(ts.contains("import { __bynkDecimalAdd }"), "{ts}");
    }

    /// The one returned expression of `f`, async or not.
    fn returned_any(ts: &str) -> String {
        let at = ts.find("function f(").expect("f");
        let rest = &ts[at..];
        let ret = rest.find("  return ").expect("a return") + "  return ".len();
        let end = rest[ret..].find(";\n}\n").expect("end");
        rest[ret..ret + end].to_string()
    }

    fn arm_source(elem: &str, ret: &str, extra: &str, expr: &str) -> String {
        let param = format!("xs: List[{elem}]");
        let body = expr.replace("{R}", "xs");
        format!("fn f({param}{extra}) -> {ret} {{ {body} }}")
    }

    /// The layout of the statement arms: a block-bodied arrow, one statement per
    /// line, an unbraced loop body inline, `} else {` on one line, and the
    /// accumulator's function type keeps its parameter names.
    #[test]
    fn statement_arms_print_as_multi_line_arrows() {
        for (elem, ret, extra, expr, want) in [
            (
                "Int",
                "Effect[Int]",
                ", g: (Int, Int) -> Effect[Int]",
                "{R}.foldEff(0, g)",
                "(async (__xs: readonly number[], __acc: number, __f: (acc: number, x: number) => globalThis.Promise<number>) => {
    for (const __x of __xs) __acc = await __f(__acc, __x);
    return __acc;
  })(xs, 0, g)",
            ),
            (
                "Result[Int, String]",
                "Result[List[Int], List[String]]",
                "",
                "{R}.sequenceAll()",
                "((__xs: readonly Result<number, string>[]) => {
    const __ok: number[] = [];
    const __errs: string[] = [];
    for (const __r of __xs) {
      if (__r.tag === \"Err\") {
        __errs.push(__r.error);
      } else {
        __ok.push(__r.value);
      }
    }
    return __errs.length > 0 ? Err(__errs) : Ok(__ok);
  })(xs)",
            ),
            (
                "Int",
                "List[Int]",
                "",
                "{R}.sortByDescending((x) => x)",
                "[...xs].sort((__a: number, __b: number) => {
    const __ka = ((x: number) => x)(__a);
    const __kb = ((x: number) => x)(__b);
    return __ka < __kb ? 1 : __ka > __kb ? -1 : 0;
  })",
            ),
            (
                "Int",
                "List[Int]",
                ", ys: List[Int]",
                "{R}.joinOn(ys, (x) => x, (y) => y, (x, y) => x + y)",
                "(() => {
    const __h: globalThis.Record<string, number[]> = globalThis.Object.create(null);
    for (const __u of ys) {
      const __k = String(((y: number) => y)(__u));
      (__h[__k] = __h[__k] ?? []).push(__u);
    }
    return (xs).flatMap((__t: number) => {
      const __m = __h[String(((x: number) => x)(__t))] ?? [];
      return __m.map((__u: number) => ((x: number, y: number) => x + y)(__t, __u));
    });
  })()",
            ),
        ] {
            let ts = emitted(&arm_source(elem, ret, extra, expr));
            assert_eq!(returned_any(&ts), want, "{expr}");
        }
    }

    /// The async arms stay async by construction and still import the settle
    /// helper that `par*` calls: `note_settle_all` fires as it did.
    #[test]
    fn par_arms_import_their_settle_helper() {
        let ts = emitted(&arm_source(
            "Int",
            "Effect[()]",
            ", g: (Int) -> Effect[()]",
            "{R}.parForEach(g)",
        ));
        assert!(ts.contains("__bynkSettleAll"), "{ts}");
        assert!(
            ts.contains("(async (__xs: readonly number[]) => {\n"),
            "{ts}"
        );
    }

    /// Hoisted effects keep source order: the receiver's `?` lowers before the
    /// argument's for `append`/`concat`, after it for `prepend`.
    #[test]
    fn hoisted_operands_keep_source_order() {
        let src = "commons lk\n\
            fn p(x: Int) -> Result[Int, String] { Ok(x) }\n\
            fn f(xs: List[Int]) -> Result[List[Int], String] { Ok(xs.append(p(1)?).concat([p(2)?])) }\n\
            fn g(xs: List[Int]) -> Result[List[Int], String] { Ok(xs.prepend(p(3)?)) }\n";
        let ts = emit_source(src);
        let at = |n: &str| ts.find(n).unwrap_or_else(|| panic!("{n}\n{ts}"));
        let f = at("export function f(");
        let one = ts[f..].find("p(1)").unwrap();
        let two = ts[f..].find("p(2)").unwrap();
        assert!(one < two, "{ts}");
        assert!(
            ts.contains("return Ok([...[...xs, __r0.value], ...[__r1.value]]);"),
            "{ts}"
        );
        assert!(ts.contains("return Ok([__r0.value, ...xs]);"), "{ts}");
    }

    /// Decision G: the imports derive from the printed output, so a module using
    /// `get`, `first`, `sequenceAll` and `traverseTry` imports what it uses: the
    /// `Result` and `Option` names, and nothing from `errors` (1A.36b-1).
    #[test]
    fn imports_follow_the_printed_kernel_output() {
        let src = "commons lk\n\
            fn a(xs: List[Int]) -> Option[Int] { xs.get(0) }\n\
            fn a2(xs: List[Int]) -> Option[Int] { xs.first() }\n\
            fn b(xs: List[Result[Int, String]]) -> Result[List[Int], List[String]] { xs.sequenceAll() }\n\
            fn c(xs: List[Int]) -> Effect[Result[List[Int], String]] {\n\
              xs.traverseTry((x) => { let r: Result[Int, String] = Ok(x)\n  Effect.pure(r) })\n\
            }\n";
        let ts = emit_source(src);
        assert!(
            ts.contains(
                "import { Ok, Err, Some, None, type Result, type Option } from \"./__bynk_runtime/result.js\";"
            ),
            "{ts}"
        );
        assert_eq!(ts.matches("import ").count(), 1, "{ts}");
    }
}
