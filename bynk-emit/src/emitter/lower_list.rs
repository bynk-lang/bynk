//! The `List` kernel's lowering: `lower_list_kernel` and the helpers its arms
//! share. Split out of `lower.rs` (back-half paydown 1A.18a, #3719). The
//! dispatcher matches the checker's typed [`ListOp`]; each operation is its own
//! builder, and the pieces the sibling kernels will want (comparator binding,
//! keyed-helper choice, key-function types) are plain functions here.

use bynk_check::checker::{NamedKind, Ty, TyId, Types};
use bynk_check::kernel_op::ListOp;
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsExpr, TsLit, TsParam, TsStmt, TsType, TsUnaryOp,
    print_type,
};

use super::lower::{Built, Pre, decimal_lit, ident_node, node_or_text, num_lit};
use super::lower_ctx::LowerCtx;
use super::ts_types::{TyRender, ts_ty, ts_ty_to_ts_type};
use super::{BaseType, Expr};

/// v0.150 (ADR 0174): peel a `traverseTry`/`parTraverseTry` call's checked type
/// `Effect[Result[List[U], E]]` to the `U` TS — the short-circuit collect's
/// output-array element annotation. Falls back to `any` if the shape is absent.
pub(super) fn list_ok_elem(call_ty: Option<TyId>, tys: TyRender<'_>) -> TsType {
    match call_ty.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
            Ty::Result(ok, _) => match &*tys.get(*ok) {
                Ty::List(u) => ts_ty_to_ts_type(*u, tys),
                _ => ts_ty_to_ts_type(*ok, tys),
            },
            _ => ts_ty_to_ts_type(*inner, tys),
        },
        _ => any_ty(),
    }
}

/// The `any` fallback of a checked shape that is absent (an already-diagnosed
/// program), the one place the lowering names it: the list kernel's and a UFCS
/// call's receiver argument that is not in scope everywhere.
pub(super) fn any_ty() -> TsType {
    TsType::named("any")
}

/// [`list_ok_elem`] as text, for the `Query` kernel's still-text builders.
pub(super) fn list_ok_elem_ts(call_ty: Option<TyId>, tys: TyRender<'_>) -> String {
    print_type(&list_ok_elem(call_ty, tys))
}

/// What every arm reads: the call, its receiver and arguments, and the element
/// type spelled once as a type (`elem_ts`) and as the list parameter's type
/// (`xs_ts`, `readonly E[]`). Both come from the node, so the text fallbacks and
/// the node forms bracket a type the same way.
struct ListArm<'a> {
    e: &'a Expr,
    receiver: &'a Expr,
    args: &'a [Expr],
    elem_ts: String,
    xs_ts: String,
    /// The same element type as a node, for the arms that build nodes.
    elem: TsType,
}

// ---- Node builders shared by the arms and, by design, the sibling kernels ----

/// The `N` children of an `N`-child builder, in order.
fn take<const N: usize, T>(children: Vec<T>) -> [T; N] {
    children
        .try_into()
        .unwrap_or_else(|_| unreachable!("an {N}-child builder is given exactly {N} children"))
}

fn id(name: &str) -> TsExpr {
    ident_node(name)
}

fn member(object: TsExpr, property: &str) -> TsExpr {
    TsExpr::Member {
        object: Box::new(object),
        property: property.to_string(),
    }
}

fn call(callee: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(callee),
        args,
    }
}

fn paren(e: TsExpr) -> TsExpr {
    TsExpr::Paren(Box::new(e))
}

/// `(recv).method(args)`: the receiver is always bracketed, as the text forms did.
fn method_call(recv: TsExpr, method: &str, args: Vec<TsExpr>) -> TsExpr {
    call(member(paren(recv), method), args)
}

/// `(f)(x)`: a Bynk function value applied to one argument.
fn applied(f: TsExpr, x: TsExpr) -> TsExpr {
    call(paren(f), vec![x])
}

fn param(name: &str, ty: Option<TsType>) -> TsParam {
    TsParam {
        name: name.to_string(),
        ty,
        optional: false,
        default: None,
    }
}

/// An expression-bodied arrow; `is_async` makes it `async`.
fn arrow_with(params: Vec<TsParam>, is_async: bool, body: TsExpr) -> TsExpr {
    TsExpr::Arrow {
        params,
        is_async,
        generics: vec![],
        return_type: None,
        body: Box::new(TsArrowBody::Expr(Box::new(body))),
    }
}

fn arrow(params: Vec<TsParam>, body: TsExpr) -> TsExpr {
    arrow_with(params, false, body)
}

/// An immediately-invoked arrow: the kernels' way of naming the receiver once.
fn iife(params: Vec<TsParam>, body: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    call(arrow(params, body), args)
}

fn binary(op: TsBinaryOp, left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn spread(e: TsExpr) -> TsExpr {
    TsExpr::Spread(Box::new(e))
}

/// `array.method(args)`: the object is not bracketed (`__xs.map(…)`, `__out.push(…)`).
fn method_call_bare(object: TsExpr, method: &str, args: Vec<TsExpr>) -> TsExpr {
    call(member(object, method), args)
}

/// An arrow with a multi-line block body, one statement per line.
fn block_arrow(params: Vec<TsParam>, is_async: bool, body: Vec<TsStmt>) -> TsExpr {
    TsExpr::Arrow {
        params,
        is_async,
        generics: vec![],
        return_type: None,
        body: Box::new(TsArrowBody::Multiline(body)),
    }
}

/// An immediately-invoked block-bodied arrow. `is_async` is the arm's own: an
/// `await` in its body needs it, and the call is not awaited here (the
/// surrounding `<-` bind awaits the promise).
fn iife_block(
    params: Vec<TsParam>,
    is_async: bool,
    body: Vec<TsStmt>,
    args: Vec<TsExpr>,
) -> TsExpr {
    call(block_arrow(params, is_async, body), args)
}

// ---- Statements ----

fn const_(name: &str, ty: Option<TsType>, init: TsExpr) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.to_string()), ty, init, None)
}

fn let_(name: &str, init: TsExpr) -> TsStmt {
    TsStmt::let_stmt(
        TsBindingName::Ident(name.to_string()),
        None,
        Some(init),
        None,
    )
}

fn ret(e: TsExpr) -> TsStmt {
    TsStmt::return_stmt(Some(e), None)
}

fn expr_stmt(e: TsExpr) -> TsStmt {
    TsStmt::expr_stmt(e, None)
}

fn assign(target: TsExpr, value: TsExpr) -> TsStmt {
    TsStmt::assign(target, value, None)
}

fn block(stmts: Vec<TsStmt>) -> TsStmt {
    TsStmt::block(stmts, None)
}

fn for_of(binding: &str, iter: TsExpr, body: TsStmt) -> TsStmt {
    TsStmt::for_of(binding, iter, body, None)
}

/// `await (f)(__x)`: an effectful step applied to the loop's element.
fn awaited_apply(f: TsExpr) -> TsExpr {
    TsExpr::Await(Box::new(applied(f, id("__x"))))
}

/// `(a: A, b: B) => R`: a function type whose parameter names are part of its spelling.
fn fn_type(params: &[(&str, &TsType)], ret: TsType) -> TsType {
    TsType::Fn {
        params: params.iter().map(|(_, t)| (*t).clone()).collect(),
        names: params.iter().map(|(n, _)| (*n).to_string()).collect(),
        ret: Box::new(ret),
    }
}

/// `globalThis.Promise<T>`.
fn promise(t: TsType) -> TsType {
    TsType::named_with_args("globalThis.Promise", vec![t])
}

/// `T[]` as text, for the text fallbacks.
fn array_ts(t: &TsType) -> String {
    print_type(&TsType::array(t.clone()))
}

fn zero() -> TsExpr {
    num_lit("0".to_string())
}

/// `readonly E[]`: the array type of a list parameter.
fn xs_ty(elem: &TsType) -> TsType {
    TsType::readonly_array(elem.clone())
}

/// `(__xs: readonly E[])`: the single list parameter most arms close over.
fn xs_param(elem: &TsType) -> TsParam {
    param("__xs", Some(xs_ty(elem)))
}

/// `globalThis.Math.max(0, n)`.
fn clamped_at_zero(n: TsExpr) -> TsExpr {
    call(
        member(member(id("globalThis"), "Math"), "max"),
        vec![zero(), n],
    )
}

/// `[...xs].sort((__a: E, __b: E) => body)`.
fn sorted_copy(xs: TsExpr, elem: &TsType, body: TsExpr) -> TsExpr {
    let pair = arrow(
        vec![
            param("__a", Some(elem.clone())),
            param("__b", Some(elem.clone())),
        ],
        body,
    );
    method_sort(TsExpr::array(vec![spread(xs)]), pair)
}

/// `array.sort(compare)`: the array is not bracketed, so `[...xs].sort(…)` prints bare.
fn method_sort(array: TsExpr, compare: TsExpr) -> TsExpr {
    call(member(array, "sort"), vec![compare])
}

/// The comparator when it is a bare identifier (a runtime comparator or an
/// in-scope witness), which is then substituted directly rather than bound.
fn bare_comparator(cmp: &TsExpr) -> Option<&TsExpr> {
    match cmp {
        TsExpr::Ident(name) if name.chars().all(|c| c.is_alphanumeric() || c == '_') => Some(cmp),
        _ => None,
    }
}

/// v0.20b: lower a built-in `List` kernel method. Returns None for a method
/// name the kernel doesn't own (the checker has already rejected it; this
/// keeps the dispatch defensive). All forms are pure expressions; `foldEff`
/// returns a Promise that the surrounding `<-` bind awaits. A node when the
/// operands are nodes, else text (see `node_or_text`); every arm builds a node,
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
) -> Option<Built> {
    let tys = cx.ty_render();
    let elem_ts = ts_ty(elem, tys);
    let elem_node = ts_ty_to_ts_type(elem, tys);
    let a = ListArm {
        e,
        receiver,
        args,
        xs_ts: print_type(&xs_ty(&elem_node)),
        elem_ts,
        elem: elem_node,
    };
    let p = pre;
    let built: Option<Built> = match (op, args) {
        (ListOp::Length | ListOp::Count, []) => Some(list_length(&a, p, cx)),
        (ListOp::Get, [index]) => Some(list_get(&a, index, p, cx)),
        (ListOp::Prepend, [head]) => Some(list_prepend(&a, head, p, cx)),
        (ListOp::Append, [item]) => Some(list_append(&a, item, p, cx)),
        (ListOp::Concat, [other]) => Some(list_concat(&a, other, p, cx)),
        (ListOp::Fold, [init, f]) => Some(list_fold(&a, init, f, p, cx)),
        (ListOp::FoldEff, [init, f]) => Some(list_fold_eff(&a, init, f, p, cx)),
        (ListOp::ForEach, [f]) => Some(list_for_each(&a, f, p, cx)),
        (ListOp::ParTraverse, [f]) => Some(list_par_traverse(&a, f, p, cx)),
        (ListOp::TraverseAll, [f]) => Some(list_traverse_all(&a, f, p, cx)),
        (ListOp::ParTraverseAll, [f]) => Some(list_par_traverse_all(&a, f, p, cx)),
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

fn list_length(a: &ListArm, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let recv = pre.lower_node(a.receiver, cx);
    node_or_text(
        vec![recv],
        |n| {
            let [recv] = take(n);
            member(paren(recv), "length")
        },
        |t| {
            let [recv] = take(t);
            format!("({recv}).length")
        },
    )
}

fn list_get(a: &ListArm, index: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (xs_ts, elem_ts) = (&a.xs_ts, &a.elem_ts);
    let recv = pre.lower_node(a.receiver, cx);
    let idx = pre.lower_node(index, cx);
    node_or_text(
        vec![recv, idx],
        |n| {
            let [recv, idx] = take(n);
            let in_range = binary(
                TsBinaryOp::And,
                binary(TsBinaryOp::GreaterThanEq, id("__i"), zero()),
                binary(
                    TsBinaryOp::LessThan,
                    id("__i"),
                    member(id("__xs"), "length"),
                ),
            );
            let item = TsExpr::As {
                expr: Box::new(TsExpr::Index {
                    object: Box::new(id("__xs")),
                    index: Box::new(id("__i")),
                }),
                ty: a.elem.clone(),
            };
            let body = TsExpr::Conditional {
                test: Box::new(in_range),
                consequent: Box::new(call(id("Some"), vec![item])),
                alternate: Box::new(id("None")),
            };
            iife(
                vec![
                    xs_param(&a.elem),
                    param("__i", Some(TsType::named("number"))),
                ],
                body,
                vec![recv, idx],
            )
        },
        |t| {
            let [recv, idx] = take(t);
            format!(
                "((__xs: {xs_ts}, __i: number) => __i >= 0 && __i < __xs.length ? Some(__xs[__i] as {elem_ts}) : None)({recv}, {idx})"
            )
        },
    )
}

fn list_prepend(a: &ListArm, head: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let head = pre.lower_node(head, cx);
    let recv = pre.lower_node(a.receiver, cx);
    node_or_text(
        vec![head, recv],
        |n| {
            let [head, recv] = take(n);
            TsExpr::array(vec![head, spread(recv)])
        },
        |t| {
            let [head, recv] = take(t);
            format!("[{head}, ...{recv}]")
        },
    )
}

// #1889: the receiver lowers before the argument, as written, so any
// hoisted effects keep source order.
fn list_append(a: &ListArm, item: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let recv = pre.lower_node(a.receiver, cx);
    let item = pre.lower_node(item, cx);
    node_or_text(
        vec![recv, item],
        |n| {
            let [recv, item] = take(n);
            TsExpr::array(vec![spread(recv), item])
        },
        |t| {
            let [recv, item] = take(t);
            format!("[...{recv}, {item}]")
        },
    )
}

fn list_concat(a: &ListArm, other: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let recv = pre.lower_node(a.receiver, cx);
    let other = pre.lower_node(other, cx);
    node_or_text(
        vec![recv, other],
        |n| {
            let [recv, other] = take(n);
            TsExpr::array(vec![spread(recv), spread(other)])
        },
        |t| {
            let [recv, other] = take(t);
            format!("[...{recv}, ...{other}]")
        },
    )
}

fn list_fold(a: &ListArm, init: &Expr, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (e, elem_ts, xs_ts) = (a.e, &a.elem_ts, &a.xs_ts);
    let tys = cx.ty_render();
    // The call's checked type is the accumulator type.
    let acc = cx
        .commons()
        .expr_types
        .get(&e.id)
        .map(|te| ts_ty_to_ts_type(te.ty, tys))
        .unwrap_or_else(|| {
            panic!(
                "bynk internal error (finding #28): emitter has no recorded type for a \
                     `fold` call at {:?} — the checker should already have typed it",
                e.span
            )
        });
    let acc_ts = print_type(&acc);
    let recv = pre.lower_node(a.receiver, cx);
    let init = pre.lower_node(init, cx);
    let f = pre.lower_node(f, cx);
    node_or_text(
        vec![recv, init, f],
        |n| {
            let [recv, init, f] = take(n);
            let step = fn_type(&[("acc", &acc), ("x", &a.elem)], acc.clone());
            let body = vec![
                for_of(
                    "__x",
                    id("__xs"),
                    assign(id("__acc"), call(id("__f"), vec![id("__acc"), id("__x")])),
                ),
                ret(id("__acc")),
            ];
            iife_block(
                vec![
                    xs_param(&a.elem),
                    param("__acc", Some(acc.clone())),
                    param("__f", Some(step)),
                ],
                false,
                body,
                vec![recv, init, f],
            )
        },
        |t| {
            let [recv, init, f] = take(t);
            format!(
                "((__xs: {xs_ts}, __acc: {acc_ts}, __f: (acc: {acc_ts}, x: {elem_ts}) => {acc_ts}) => {{ for (const __x of __xs) __acc = __f(__acc, __x); return __acc; }})({recv}, {init}, {f})"
            )
        },
    )
}

fn list_fold_eff(a: &ListArm, init: &Expr, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (e, elem_ts, xs_ts) = (a.e, &a.elem_ts, &a.xs_ts);
    let tys = cx.ty_render();
    // The call's checked type is `Effect[Acc]` — peel for the TS
    // accumulator annotation.
    let acc = match cx.commons().expr_ty(e.id).as_deref() {
        Some(Ty::Effect(acc)) => ts_ty_to_ts_type(*acc, tys),
        Some(_) => ts_ty_to_ts_type(cx.commons().expr_types[&e.id].ty, tys),
        None => panic!(
            "bynk internal error (finding #28): emitter has no recorded type for a \
                     `foldEff` call at {:?} — the checker should already have typed it",
            e.span
        ),
    };
    let acc_ts = print_type(&acc);
    let recv = pre.lower_node(a.receiver, cx);
    let init = pre.lower_node(init, cx);
    let f = pre.lower_node(f, cx);
    node_or_text(
        vec![recv, init, f],
        |n| {
            let [recv, init, f] = take(n);
            let step = fn_type(&[("acc", &acc), ("x", &a.elem)], promise(acc.clone()));
            let awaited = TsExpr::Await(Box::new(call(id("__f"), vec![id("__acc"), id("__x")])));
            let body = vec![
                for_of("__x", id("__xs"), assign(id("__acc"), awaited)),
                ret(id("__acc")),
            ];
            iife_block(
                vec![
                    xs_param(&a.elem),
                    param("__acc", Some(acc.clone())),
                    param("__f", Some(step)),
                ],
                true,
                body,
                vec![recv, init, f],
            )
        },
        |t| {
            let [recv, init, f] = take(t);
            format!(
                "(async (__xs: {xs_ts}, __acc: {acc_ts}, __f: (acc: {acc_ts}, x: {elem_ts}) => globalThis.Promise<{acc_ts}>) => {{ for (const __x of __xs) __acc = await __f(__acc, __x); return __acc; }})({recv}, {init}, {f})"
            )
        },
    )
}

// v0.146 (ADR 0170): `forEach` — run an effectful step per element in
// order, awaiting each; yields `Promise<void>`. The eager `List`
// analogue of the `Query.forEach` terminal, emitted inline.
fn list_for_each(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let xs_ts = &a.xs_ts;
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    node_or_text(
        vec![recv, f],
        |n| {
            let [recv, f] = take(n);
            let each = block(vec![expr_stmt(awaited_apply(f))]);
            iife_block(
                vec![xs_param(&a.elem)],
                true,
                vec![for_of("__x", id("__xs"), each)],
                vec![recv],
            )
        },
        |t| {
            let [recv, f] = take(t);
            format!(
                "(async (__xs: {xs_ts}) => {{ for (const __x of __xs) {{ await ({f})(__x); }} }})({recv})"
            )
        },
    )
}

// v0.147 (ADR 0171): `parTraverse` — issue the effectful fn over every
// element concurrently and await them together, so one slow element does
// not head-of-line-block the rest. The eager `List` analogue of the
// `Query.parTraverse` terminal, emitted inline.
fn list_par_traverse(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (xs_ts, elem_ts) = (&a.xs_ts, &a.elem_ts);
    cx.note_settle_all();
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    node_or_text(
        vec![recv, f],
        |n| {
            let [recv, f] = take(n);
            let settled = TsExpr::Await(Box::new(call(
                id("__bynkSettleAll"),
                vec![settle_thunks(a, f)],
            )));
            iife_block(
                vec![xs_param(&a.elem)],
                true,
                vec![expr_stmt(settled)],
                vec![recv],
            )
        },
        |t| {
            let [recv, f] = take(t);
            format!(
                "(async (__xs: {xs_ts}) => {{ await __bynkSettleAll(__xs.map((__x: {elem_ts}) => () => ({f})(__x))); }})({recv})"
            )
        },
    )
}

// v0.148 (ADR 0172): the collect-all iterators — every element's
// `Result` outcome is gathered (an `Err` is a value, so nothing rejects).
// `traverseAll` awaits each in order into a typed array; `parTraverseAll`
// issues all at once and `Promise.all`s the resolved `Result`s. Both
// yield `Promise<Result<…>[]>`.
fn list_traverse_all(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (e, xs_ts) = (a.e, &a.xs_ts);
    let tys = cx.ty_render();
    // The call's checked type is `Effect[List[Result[B, E]]]` — peel to
    // the `Result[B, E]` TS for the output-array annotation.
    let res = match cx.commons().expr_ty(e.id).as_deref() {
        Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
            Ty::List(el) => ts_ty_to_ts_type(*el, tys),
            _ => ts_ty_to_ts_type(*inner, tys),
        },
        other => panic!(
            "bynk internal error (finding #28): emitter expected `Effect[List[_]]` \
                     recorded for a `traverseAll` call at {:?}, but found {other:?} — the \
                     checker should already have typed it",
            e.span
        ),
    };
    let res_arr = array_ts(&res);
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    node_or_text(
        vec![recv, f],
        |n| {
            let [recv, f] = take(n);
            let push = expr_stmt(method_call_bare(
                id("__out"),
                "push",
                vec![awaited_apply(f)],
            ));
            let body = vec![
                const_(
                    "__out",
                    Some(TsType::array(res.clone())),
                    TsExpr::array(vec![]),
                ),
                for_of("__x", id("__xs"), block(vec![push])),
                ret(id("__out")),
            ];
            iife_block(vec![xs_param(&a.elem)], true, body, vec![recv])
        },
        |t| {
            let [recv, f] = take(t);
            format!(
                "(async (__xs: {xs_ts}) => {{ const __out: {res_arr} = []; for (const __x of __xs) {{ __out.push(await ({f})(__x)); }} return __out; }})({recv})"
            )
        },
    )
}

fn list_par_traverse_all(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (xs_ts, elem_ts) = (&a.xs_ts, &a.elem_ts);
    cx.note_settle_all();
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    node_or_text(
        vec![recv, f],
        |n| {
            let [recv, f] = take(n);
            let settled = TsExpr::Await(Box::new(call(
                id("__bynkSettleAll"),
                vec![settle_thunks(a, f)],
            )));
            call(
                arrow_with(vec![xs_param(&a.elem)], true, settled),
                vec![recv],
            )
        },
        |t| {
            let [recv, f] = take(t);
            format!(
                "(async (__xs: {xs_ts}) => await __bynkSettleAll(__xs.map((__x: {elem_ts}) => () => ({f})(__x))))({recv})"
            )
        },
    )
}

/// `__xs.map((__x: E) => () => (f)(__x))`: one thunk per element, for `__bynkSettleAll`.
fn settle_thunks(a: &ListArm, f: TsExpr) -> TsExpr {
    call(
        member(id("__xs"), "map"),
        vec![arrow(
            vec![param("__x", Some(a.elem.clone()))],
            arrow(vec![], applied(f, id("__x"))),
        )],
    )
}

// #2806: pure `Result` accumulation. The call's checked type is
// `Result[List[T], E']`; `T` and `E` (the element's own `Err` type) type
// the accumulators.
fn list_sequence(a: &ListArm, name: ListOp, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (e, xs_ts) = (a.e, &a.xs_ts);
    let tys = cx.ty_render();
    let (ok, err) = match cx.commons().expr_ty(e.id).as_deref() {
        Some(Ty::Result(ok, err)) => {
            let ok_ty = match &*tys.get(*ok) {
                Ty::List(t) => ts_ty_to_ts_type(*t, tys),
                _ => ts_ty_to_ts_type(*ok, tys),
            };
            let err_ty = match &*tys.get(*err) {
                Ty::List(t) if name == ListOp::SequenceAll => ts_ty_to_ts_type(*t, tys),
                _ => ts_ty_to_ts_type(*err, tys),
            };
            (ok_ty, err_ty)
        }
        _ => (any_ty(), any_ty()),
    };
    let (ok_arr, err_arr) = (array_ts(&ok), array_ts(&err));
    let recv = pre.lower_node(a.receiver, cx);
    node_or_text(
        vec![recv],
        |n| {
            let [recv] = take(n);
            let ok_decl = const_(
                "__ok",
                Some(TsType::array(ok.clone())),
                TsExpr::array(vec![]),
            );
            let is_err = result_is_err("__r");
            let body = if name == ListOp::SequenceAll {
                let collect = TsStmt::if_else_same_line_stmt(
                    is_err,
                    block(vec![expr_stmt(method_call_bare(
                        id("__errs"),
                        "push",
                        vec![member(id("__r"), "error")],
                    ))]),
                    block(vec![expr_stmt(method_call_bare(
                        id("__ok"),
                        "push",
                        vec![member(id("__r"), "value")],
                    ))]),
                    None,
                );
                let outcome = TsExpr::Conditional {
                    test: Box::new(binary(
                        TsBinaryOp::GreaterThan,
                        member(id("__errs"), "length"),
                        zero(),
                    )),
                    consequent: Box::new(call(id("Err"), vec![id("__errs")])),
                    alternate: Box::new(call(id("Ok"), vec![id("__ok")])),
                };
                vec![
                    ok_decl,
                    const_(
                        "__errs",
                        Some(TsType::array(err.clone())),
                        TsExpr::array(vec![]),
                    ),
                    for_of("__r", id("__xs"), block(vec![collect])),
                    ret(outcome),
                ]
            } else {
                let bail = TsStmt::if_stmt(
                    is_err,
                    block(vec![ret(call(id("Err"), vec![member(id("__r"), "error")]))]),
                    None,
                );
                let push = expr_stmt(method_call_bare(
                    id("__ok"),
                    "push",
                    vec![member(id("__r"), "value")],
                ));
                vec![
                    ok_decl,
                    for_of("__r", id("__xs"), block(vec![bail, push])),
                    ret(call(id("Ok"), vec![id("__ok")])),
                ]
            };
            iife_block(vec![xs_param(&a.elem)], false, body, vec![recv])
        },
        |t| {
            let [recv] = take(t);
            if name == ListOp::SequenceAll {
                format!(
                    "((__xs: {xs_ts}) => {{ const __ok: {ok_arr} = []; const __errs: {err_arr} = []; for (const __r of __xs) {{ if (__r.tag === \"Err\") {{ __errs.push(__r.error); }} else {{ __ok.push(__r.value); }} }} return __errs.length > 0 ? Err(__errs) : Ok(__ok); }})({recv})"
                )
            } else {
                format!(
                    "((__xs: {xs_ts}) => {{ const __ok: {ok_arr} = []; for (const __r of __xs) {{ if (__r.tag === \"Err\") {{ return Err(__r.error); }} __ok.push(__r.value); }} return Ok(__ok); }})({recv})"
                )
            }
        },
    )
}

// v0.150 (ADR 0174): the short-circuit collect iterators — stop at the
// first `Err` and return it (`Effect[Result[List[U], E]]`). `traverseTry`
// awaits each in order, bailing on the first `Err`; `parTraverseTry`
// issues all at once, then scans the resolved `Result`s in input order.
fn list_traverse_try(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (e, xs_ts) = (a.e, &a.xs_ts);
    let tys = cx.ty_render();
    let u = list_ok_elem(cx.commons().expr_types.get(&e.id).map(|te| te.ty), tys);
    let u_arr = array_ts(&u);
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    node_or_text(
        vec![recv, f],
        |n| {
            let [recv, f] = take(n);
            let step = vec![
                const_("__r", None, awaited_apply(f)),
                bail_on_err(),
                push_value(),
            ];
            let body = vec![
                const_(
                    "__out",
                    Some(TsType::array(u.clone())),
                    TsExpr::array(vec![]),
                ),
                for_of("__x", id("__xs"), block(step)),
                ret(call(id("Ok"), vec![id("__out")])),
            ];
            iife_block(vec![xs_param(&a.elem)], true, body, vec![recv])
        },
        |t| {
            let [recv, f] = take(t);
            format!(
                "(async (__xs: {xs_ts}) => {{ const __out: {u_arr} = []; for (const __x of __xs) {{ const __r = await ({f})(__x); if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})({recv})"
            )
        },
    )
}

fn list_par_traverse_try(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (e, xs_ts, elem_ts) = (a.e, &a.xs_ts, &a.elem_ts);
    let tys = cx.ty_render();
    cx.note_settle_all();
    let u = list_ok_elem(cx.commons().expr_types.get(&e.id).map(|te| te.ty), tys);
    let u_arr = array_ts(&u);
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    node_or_text(
        vec![recv, f],
        |n| {
            let [recv, f] = take(n);
            let settled = TsExpr::Await(Box::new(call(
                id("__bynkSettleAll"),
                vec![settle_thunks(a, f)],
            )));
            let scan = vec![bail_on_err(), push_value()];
            let body = vec![
                const_("__rs", None, settled),
                const_(
                    "__out",
                    Some(TsType::array(u.clone())),
                    TsExpr::array(vec![]),
                ),
                for_of("__r", id("__rs"), block(scan)),
                ret(call(id("Ok"), vec![id("__out")])),
            ];
            iife_block(vec![xs_param(&a.elem)], true, body, vec![recv])
        },
        |t| {
            let [recv, f] = take(t);
            format!(
                "(async (__xs: {xs_ts}) => {{ const __rs = await __bynkSettleAll(__xs.map((__x: {elem_ts}) => () => ({f})(__x))); const __out: {u_arr} = []; for (const __r of __rs) {{ if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})({recv})"
            )
        },
    )
}

/// `__r.tag === "Err"`.
fn result_is_err(result: &str) -> TsExpr {
    binary(
        TsBinaryOp::StrictEq,
        member(id(result), "tag"),
        TsExpr::Lit(TsLit::Str("Err".to_string())),
    )
}

/// `if (__r.tag === "Err") { return Err(__r.error); }`.
fn bail_on_err() -> TsStmt {
    TsStmt::if_stmt(
        result_is_err("__r"),
        block(vec![ret(call(id("Err"), vec![member(id("__r"), "error")]))]),
        None,
    )
}

/// `__out.push(__r.value);`.
fn push_value() -> TsStmt {
    expr_stmt(method_call_bare(
        id("__out"),
        "push",
        vec![member(id("__r"), "value")],
    ))
}

// v0.88 (ADR 0116): the eager builder/terminal vocabulary. Most lower
// to native array methods; callbacks are wrapped in a single-arg arrow
// so the array index/array extra args never reach a Bynk one-param fn.
// `map`, `filter`, `flatMap`, `any` (`some`) and `all` (`every`) share this.
fn list_callback(a: &ListArm, method: &str, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let elem_ts = &a.elem_ts;
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    node_or_text(
        vec![recv, f],
        |n| {
            let [recv, f] = take(n);
            let each = arrow(
                vec![param("__x", Some(a.elem.clone()))],
                applied(f, id("__x")),
            );
            method_call(recv, method, vec![each])
        },
        |t| {
            let [recv, f] = take(t);
            format!("({recv}).{method}((__x: {elem_ts}) => ({f})(__x))")
        },
    )
}

fn list_take(a: &ListArm, n: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let recv = pre.lower_node(a.receiver, cx);
    let n = pre.lower_node(n, cx);
    node_or_text(
        vec![recv, n],
        |c| {
            let [recv, n] = take(c);
            method_call(recv, "slice", vec![zero(), clamped_at_zero(n)])
        },
        |t| {
            let [recv, n] = take(t);
            format!("({recv}).slice(0, globalThis.Math.max(0, {n}))")
        },
    )
}

fn list_skip(a: &ListArm, n: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let recv = pre.lower_node(a.receiver, cx);
    let n = pre.lower_node(n, cx);
    node_or_text(
        vec![recv, n],
        |c| {
            let [recv, n] = take(c);
            method_call(recv, "slice", vec![clamped_at_zero(n)])
        },
        |t| {
            let [recv, n] = take(t);
            format!("({recv}).slice(globalThis.Math.max(0, {n}))")
        },
    )
}

// v0.119 (ADR 0155, DECISION C-a): `run.upTo(step)` — the driven history
// strictly before `step`. Steps are distinct object instances in the run
// array, so `indexOf` is reference identity. An IIFE avoids re-evaluating
// the receiver.
fn list_up_to(a: &ListArm, step: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (xs_ts, elem_ts) = (&a.xs_ts, &a.elem_ts);
    let recv = pre.lower_node(a.receiver, cx);
    let step = pre.lower_node(step, cx);
    node_or_text(
        vec![recv, step],
        |n| {
            let [recv, step] = take(n);
            let index = call(member(id("__xs"), "indexOf"), vec![id("__s")]);
            iife(
                vec![xs_param(&a.elem), param("__s", Some(a.elem.clone()))],
                call(member(id("__xs"), "slice"), vec![zero(), index]),
                vec![recv, step],
            )
        },
        |t| {
            let [recv, step] = take(t);
            format!(
                "((__xs: {xs_ts}, __s: {elem_ts}) => __xs.slice(0, __xs.indexOf(__s)))({recv}, {step})"
            )
        },
    )
}

fn list_first(a: &ListArm, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let xs_ts = &a.xs_ts;
    let recv = pre.lower_node(a.receiver, cx);
    node_or_text(
        vec![recv],
        |n| {
            let [recv] = take(n);
            let body = TsExpr::Conditional {
                test: Box::new(binary(
                    TsBinaryOp::GreaterThan,
                    member(id("__xs"), "length"),
                    zero(),
                )),
                consequent: Box::new(call(id("Some"), vec![first_item()])),
                alternate: Box::new(id("None")),
            };
            iife(vec![xs_param(&a.elem)], body, vec![recv])
        },
        |t| {
            let [recv] = take(t);
            format!("((__xs: {xs_ts}) => __xs.length > 0 ? Some(__xs[0]) : None)({recv})")
        },
    )
}

/// `__xs[0]`.
fn first_item() -> TsExpr {
    TsExpr::Index {
        object: Box::new(id("__xs")),
        index: Box::new(zero()),
    }
}

fn list_first_or_else(a: &ListArm, default: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (xs_ts, elem_ts) = (&a.xs_ts, &a.elem_ts);
    let recv = pre.lower_node(a.receiver, cx);
    let default = pre.lower_node(default, cx);
    node_or_text(
        vec![recv, default],
        |n| {
            let [recv, default] = take(n);
            let body = TsExpr::Conditional {
                test: Box::new(binary(
                    TsBinaryOp::GreaterThan,
                    member(id("__xs"), "length"),
                    zero(),
                )),
                consequent: Box::new(first_item()),
                alternate: Box::new(id("__d")),
            };
            iife(
                vec![xs_param(&a.elem), param("__d", Some(a.elem.clone()))],
                body,
                vec![recv, default],
            )
        },
        |t| {
            let [recv, default] = take(t);
            format!(
                "((__xs: {xs_ts}, __d: {elem_ts}) => __xs.length > 0 ? __xs[0] : __d)({recv}, {default})"
            )
        },
    )
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
) -> Built {
    // The comparator is bound before the receiver and key lower: `note_cmp` fires first.
    let cmp = kernel_key_comparator(&a.args[0], cx);
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    let Some(cmp) = cmp else {
        return node_or_text(
            vec![recv, key],
            |n| {
                let [recv, key] = take(n);
                sort_plain_node(a, descending, recv, key)
            },
            |t| {
                let [recv, key] = take(t);
                sort_plain_text(a, descending, &recv, &key)
            },
        );
    };
    node_or_text(
        vec![recv, key],
        |n| {
            let [recv, key] = take(n);
            sort_node(a, descending, &cmp, recv, key)
        },
        |t| {
            let [recv, key] = take(t);
            sort_comparator_text(a, descending, &cmp, &recv, &key)
        },
    )
}

/// The argument order of the key comparison: swapped for `sortByDescending`.
fn sort_order(descending: bool) -> (&'static str, &'static str) {
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
                applied(key.clone(), id(first)),
                applied(key.clone(), id(second)),
            ],
        );
        sorted_copy(xs, &a.elem, body)
    };
    match bare_comparator(cmp) {
        Some(cmp) => compare(cmp.clone(), recv),
        None => iife(
            vec![param("__c", None), param("__r", Some(xs_ty(&a.elem)))],
            compare(id("__c"), id("__r")),
            vec![cmp.clone(), recv],
        ),
    }
}

/// The text of the two comparator forms, for a text receiver or key.
fn sort_comparator_text(
    a: &ListArm,
    descending: bool,
    cmp: &TsExpr,
    recv: &str,
    key: &str,
) -> String {
    let (xs_ts, elem_ts) = (&a.xs_ts, &a.elem_ts);
    let (first, second) = sort_order(descending);
    let cmp_text = bynk_ts::print_expr(cmp);
    match bare_comparator(cmp) {
        Some(_) => format!(
            "[...{recv}].sort((__a: {elem_ts}, __b: {elem_ts}) => {cmp_text}(({key})({first}), ({key})({second})))"
        ),
        None => format!(
            "((__c, __r: {xs_ts}) => [...__r].sort((__a: {elem_ts}, __b: {elem_ts}) => __c(({key})({first}), ({key})({second}))))({cmp_text}, {recv})"
        ),
    }
}

/// The plain-key form as a node: the bare operators over two bound keys.
fn sort_plain_node(a: &ListArm, descending: bool, recv: TsExpr, key: TsExpr) -> TsExpr {
    let (lo, hi) = if descending { ("1", "-1") } else { ("-1", "1") };
    let ordered = |op, then: &str, otherwise: TsExpr| TsExpr::Conditional {
        test: Box::new(binary(op, id("__ka"), id("__kb"))),
        consequent: Box::new(num_lit(then.to_string())),
        alternate: Box::new(otherwise),
    };
    let outcome = ordered(
        TsBinaryOp::LessThan,
        lo,
        ordered(TsBinaryOp::GreaterThan, hi, zero()),
    );
    let body = vec![
        const_("__ka", None, applied(key.clone(), id("__a"))),
        const_("__kb", None, applied(key, id("__b"))),
        ret(outcome),
    ];
    let pair = block_arrow(
        vec![
            param("__a", Some(a.elem.clone())),
            param("__b", Some(a.elem.clone())),
        ],
        false,
        body,
    );
    method_sort(TsExpr::array(vec![spread(recv)]), pair)
}

/// The plain-key form's text, for a text receiver or key.
fn sort_plain_text(a: &ListArm, descending: bool, recv: &str, key: &str) -> String {
    let elem_ts = &a.elem_ts;
    let (lo, hi) = if descending { ("1", "-1") } else { ("-1", "1") };
    format!(
        "[...{recv}].sort((__a: {elem_ts}, __b: {elem_ts}) => {{ const __ka = ({key})(__a), __kb = ({key})(__b); return __ka < __kb ? {lo} : __ka > __kb ? {hi} : 0; }})"
    )
}

/// `distinct`, `distinctBy`, `joinOn`, `leftJoin` and `groupBy` over a key that
/// is not a JS primitive: the `keys` runtime helper `helper` takes the
/// receiver, then each remaining argument in order. `note_key` fires before
/// anything lowers.
fn list_keyed(a: &ListArm, helper: &'static str, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    cx.runtime_use().note_key(helper);
    let mut children = vec![pre.lower_node(a.receiver, cx)];
    children.extend(a.args.iter().map(|x| pre.lower_node(x, cx)));
    node_or_text(
        children,
        |n| call(id(helper), n),
        |t| format!("{helper}({})", t.join(", ")),
    )
}

fn list_distinct(a: &ListArm, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let recv = pre.lower_node(a.receiver, cx);
    node_or_text(
        vec![recv],
        |n| {
            let [recv] = take(n);
            let set = TsExpr::New {
                callee: Box::new(member(id("globalThis"), "Set")),
                args: vec![recv],
            };
            TsExpr::array(vec![spread(set)])
        },
        |t| {
            let [recv] = take(t);
            format!("[...new globalThis.Set({recv})]")
        },
    )
}

fn list_distinct_by(a: &ListArm, key: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let xs_ts = &a.xs_ts;
    let out_arr = array_ts(&a.elem);
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    node_or_text(
        vec![recv, key],
        |n| {
            let [recv, key] = take(n);
            let seen = TsExpr::New {
                callee: Box::new(member(id("globalThis"), "Set")),
                args: vec![],
            };
            let fresh = TsStmt::if_stmt(
                TsExpr::Unary {
                    op: TsUnaryOp::Not,
                    expr: Box::new(method_call_bare(id("__seen"), "has", vec![id("__k")])),
                },
                block(vec![
                    expr_stmt(method_call_bare(id("__seen"), "add", vec![id("__k")])),
                    expr_stmt(method_call_bare(id("__out"), "push", vec![id("__x")])),
                ]),
                None,
            );
            let body = vec![
                const_("__seen", None, seen),
                const_(
                    "__out",
                    Some(TsType::array(a.elem.clone())),
                    TsExpr::array(vec![]),
                ),
                for_of(
                    "__x",
                    id("__xs"),
                    block(vec![const_("__k", None, applied(key, id("__x"))), fresh]),
                ),
                ret(id("__out")),
            ];
            iife_block(vec![xs_param(&a.elem)], false, body, vec![recv])
        },
        |t| {
            let [recv, key] = take(t);
            format!(
                "((__xs: {xs_ts}) => {{ const __seen = new globalThis.Set(); const __out: {out_arr} = []; for (const __x of __xs) {{ const __k = ({key})(__x); if (!__seen.has(__k)) {{ __seen.add(__k); __out.push(__x); }} }} return __out; }})({recv})"
            )
        },
    )
}

fn list_sum(a: &ListArm, key: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let elem_ts = &a.elem_ts;
    let tys = cx.ty_render();
    let decimal = key_fn_ret(key, cx).and_then(|t| t.base(tys.types())) == Some(BaseType::Decimal);
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    if decimal {
        // #2622: an exact sum, from the zero decimal.
        cx.note_decimal("__bynkDecimalAdd");
    }
    node_or_text(
        vec![recv, key],
        |n| {
            let [recv, key] = take(n);
            let (acc_ty, add, init) = if decimal {
                (
                    TsType::named(crate::emitter::ts_types::DECIMAL_TS_TYPE),
                    call(
                        id("__bynkDecimalAdd"),
                        vec![id("__s"), applied(key, id("__x"))],
                    ),
                    decimal_lit("0", 0),
                )
            } else {
                (
                    TsType::named("number"),
                    binary(TsBinaryOp::Add, id("__s"), applied(key, id("__x"))),
                    zero(),
                )
            };
            let step = arrow(
                vec![
                    param("__s", Some(acc_ty)),
                    param("__x", Some(a.elem.clone())),
                ],
                add,
            );
            method_call(recv, "reduce", vec![step, init])
        },
        |t| {
            let [recv, key] = take(t);
            if decimal {
                format!(
                    "({recv}).reduce((__s: {}, __x: {elem_ts}) => __bynkDecimalAdd(__s, ({key})(__x)), {})",
                    crate::emitter::ts_types::DECIMAL_TS_TYPE,
                    crate::emitter::ts_types::decimal_ts_literal("0", 0)
                )
            } else {
                format!("({recv}).reduce((__s: number, __x: {elem_ts}) => __s + ({key})(__x), 0)")
            }
        },
    )
}

fn list_min_max(a: &ListArm, op: ListOp, key: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let xs_ts = &a.xs_ts;
    let cmp = if op == ListOp::Min { "<" } else { ">" };
    let cmp_op = if op == ListOp::Min {
        TsBinaryOp::LessThan
    } else {
        TsBinaryOp::GreaterThan
    };
    let key_cmp = kernel_key_comparator(&a.args[0], cx);
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    node_or_text(
        vec![recv, key],
        |n| {
            let [recv, key] = take(n);
            // The kernel over `__xs`, ordering by `better(__k, __m)`.
            let kernel = |better: &dyn Fn() -> TsExpr| {
                let take_key = TsStmt::if_stmt(better(), assign(id("__m"), id("__k")), None);
                let scan = block(vec![
                    const_("__k", None, applied(key.clone(), id("__x"))),
                    take_key,
                ]);
                vec![
                    TsStmt::if_stmt(
                        binary(TsBinaryOp::StrictEq, member(id("__xs"), "length"), zero()),
                        ret(id("None")),
                        None,
                    ),
                    let_("__m", applied(key.clone(), first_item())),
                    for_of("__x", id("__xs"), scan),
                    ret(call(id("Some"), vec![id("__m")])),
                ]
            };
            match &key_cmp {
                None => iife_block(
                    vec![xs_param(&a.elem)],
                    false,
                    kernel(&|| binary(cmp_op, id("__k"), id("__m"))),
                    vec![recv],
                ),
                Some(key_cmp) => with_comparator_node(key_cmp, |c| {
                    iife_block(
                        vec![xs_param(&a.elem)],
                        false,
                        kernel(&|| {
                            binary(cmp_op, call(c.clone(), vec![id("__k"), id("__m")]), zero())
                        }),
                        vec![recv],
                    )
                }),
            }
        },
        |t| {
            let [recv, key] = take(t);
            match &key_cmp {
                None => format!(
                    "((__xs: {xs_ts}) => {{ if (__xs.length === 0) return None; let __m = ({key})(__xs[0]); for (const __x of __xs) {{ const __k = ({key})(__x); if (__k {cmp} __m) __m = __k; }} return Some(__m); }})({recv})"
                ),
                Some(key_cmp) => with_comparator(key_cmp, |c| {
                    format!(
                        "((__xs: {xs_ts}) => {{ if (__xs.length === 0) return None; let __m = ({key})(__xs[0]); for (const __x of __xs) {{ const __k = ({key})(__x); if ({c}(__k, __m) {cmp} 0) __m = __k; }} return Some(__m); }})({recv})"
                    )
                }),
            }
        },
    )
}

fn list_average(a: &ListArm, key: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let (e, xs_ts) = (a.e, &a.xs_ts);
    let tys = cx.ty_render();
    // D3: Duration averages round to integer millis; Int/Float -> Float.
    let round = matches!(
        cx.commons().expr_ty(e.id).as_deref(),
        Some(Ty::Option(inner)) if matches!(&*tys.get(*inner), Ty::Base(BaseType::Duration))
    );
    let mean = if round {
        "globalThis.Math.round(__s / __xs.length)"
    } else {
        "__s / __xs.length"
    };
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    node_or_text(
        vec![recv, key],
        |n| {
            let [recv, key] = take(n);
            let ratio = binary(TsBinaryOp::Div, id("__s"), member(id("__xs"), "length"));
            let mean = if round {
                call(
                    member(member(id("globalThis"), "Math"), "round"),
                    vec![ratio],
                )
            } else {
                ratio
            };
            let add = expr_stmt(TsExpr::AddAssign {
                target: Box::new(id("__s")),
                value: Box::new(applied(key, id("__x"))),
            });
            let body = vec![
                TsStmt::if_stmt(
                    binary(TsBinaryOp::StrictEq, member(id("__xs"), "length"), zero()),
                    ret(id("None")),
                    None,
                ),
                let_("__s", zero()),
                for_of("__x", id("__xs"), add),
                ret(call(id("Some"), vec![mean])),
            ];
            iife_block(vec![xs_param(&a.elem)], false, body, vec![recv])
        },
        |t| {
            let [recv, key] = take(t);
            format!(
                "((__xs: {xs_ts}) => {{ if (__xs.length === 0) return None; let __s = 0; for (const __x of __xs) __s += ({key})(__x); return Some({mean}); }})({recv})"
            )
        },
    )
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
) -> Built {
    let elem_ts = &a.elem_ts;
    let recv = pre.lower_node(a.receiver, cx);
    let u = join_other_elem(a.args, cx);
    let (u_ts, u_arr) = (print_type(&u), array_ts(&u));
    let other = pre.lower_node(other, cx);
    let left = pre.lower_node(left, cx);
    let right = pre.lower_node(right, cx);
    let into = pre.lower_node(into, cx);
    node_or_text(
        vec![recv, other, left, right, into],
        |n| {
            let [recv, other, left, right, into] = take(n);
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
                const_("__k", None, string_of(applied(right, id("__u")))),
                expr_stmt(method_call_bare(paren(bucket), "push", vec![id("__u")])),
            ]);
            let matches = TsExpr::Index {
                object: Box::new(id("__h")),
                index: Box::new(string_of(applied(left, id("__t")))),
            };
            let found = const_(
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
                    vec![param("__u", Some(u.clone()))],
                    call(paren(into.clone()), vec![id("__t"), u_arg]),
                )
            };
            let result = if left_join {
                TsExpr::Conditional {
                    test: Box::new(binary(
                        TsBinaryOp::GreaterThan,
                        member(id("__m"), "length"),
                        zero(),
                    )),
                    consequent: Box::new(method_call_bare(
                        id("__m"),
                        "map",
                        vec![project(call(id("Some"), vec![id("__u")]))],
                    )),
                    alternate: Box::new(TsExpr::array(vec![call(
                        paren(into.clone()),
                        vec![id("__t"), id("None")],
                    )])),
                }
            } else {
                method_call_bare(id("__m"), "map", vec![project(id("__u"))])
            };
            let probe = block_arrow(
                vec![param("__t", Some(a.elem.clone()))],
                false,
                vec![found, ret(result)],
            );
            let body = vec![
                const_("__h", Some(record_of(&u)), create_null()),
                for_of("__u", other, fill),
                ret(method_call(recv, "flatMap", vec![probe])),
            ];
            iife_block(vec![], false, body, vec![])
        },
        |t| {
            let [recv, other, left, right, into] = take(t);
            if left_join {
                format!(
                    "(() => {{ const __h: globalThis.Record<string, {u_arr}> = globalThis.Object.create(null); for (const __u of {other}) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return ({recv}).flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.length > 0 ? __m.map((__u: {u_ts}) => ({into})(__t, Some(__u))) : [({into})(__t, None)]; }}); }})()"
                )
            } else {
                format!(
                    "(() => {{ const __h: globalThis.Record<string, {u_arr}> = globalThis.Object.create(null); for (const __u of {other}) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return ({recv}).flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.map((__u: {u_ts}) => ({into})(__t, __u)); }}); }})()"
                )
            }
        },
    )
}

/// `__h[__k]`.
fn hash_slot() -> TsExpr {
    TsExpr::Index {
        object: Box::new(id("__h")),
        index: Box::new(id("__k")),
    }
}

/// `String(x)`: the stringified hash key.
fn string_of(x: TsExpr) -> TsExpr {
    call(id("String"), vec![x])
}

/// `globalThis.Record<string, E[]>`: the hash of buckets.
fn record_of(elem: &TsType) -> TsType {
    TsType::named_with_args(
        "globalThis.Record",
        vec![TsType::named("string"), TsType::array(elem.clone())],
    )
}

/// `globalThis.Object.create(null)`.
fn create_null() -> TsExpr {
    call(
        member(member(id("globalThis"), "Object"), "create"),
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
) -> Built {
    let elem_ts = &a.elem_ts;
    let recv = pre.lower_node(a.receiver, cx);
    let u = join_other_elem(a.args, cx);
    let (u_ts, u_xs) = (print_type(&u), print_type(&xs_ty(&u)));
    let other = pre.lower_node(other, cx);
    let on = pre.lower_node(on, cx);
    let into = pre.lower_node(into, cx);
    node_or_text(
        vec![recv, other, on, into],
        |n| {
            let [recv, other, on, into] = take(n);
            let pair = |f: TsExpr| call(paren(f), vec![id("__t"), id("__u")]);
            let each_u = |body: TsExpr| arrow(vec![param("__u", Some(u.clone()))], body);
            let matched = method_call_bare(id("__b"), "filter", vec![each_u(pair(on))]);
            let joined = method_call_bare(matched, "map", vec![each_u(pair(into))]);
            let flat = method_call(
                recv,
                "flatMap",
                vec![arrow(vec![param("__t", Some(a.elem.clone()))], joined)],
            );
            let body = vec![const_("__b", Some(xs_ty(&u)), other), ret(flat)];
            iife_block(vec![], false, body, vec![])
        },
        |t| {
            let [recv, other, on, into] = take(t);
            format!(
                "(() => {{ const __b: {u_xs} = {other}; return ({recv}).flatMap((__t: {elem_ts}) => __b.filter((__u: {u_ts}) => ({on})(__t, __u)).map((__u: {u_ts}) => ({into})(__t, __u))); }})()"
            )
        },
    )
}

fn list_group_by(a: &ListArm, key: &Expr, into: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let record = record_of(&a.elem);
    let record_ts = print_type(&record);
    let recv = pre.lower_node(a.receiver, cx);
    let key = pre.lower_node(key, cx);
    let into = pre.lower_node(into, cx);
    node_or_text(
        vec![recv, key, into],
        |n| {
            let [recv, key, into] = take(n);
            let first_seen = TsStmt::if_stmt(
                TsExpr::Unary {
                    op: TsUnaryOp::Not,
                    expr: Box::new(binary(TsBinaryOp::In, id("__k"), id("__h"))),
                },
                block(vec![
                    assign(hash_slot(), TsExpr::array(vec![])),
                    expr_stmt(method_call_bare(id("__order"), "push", vec![id("__k")])),
                ]),
                None,
            );
            let partition = block(vec![
                const_("__k", None, string_of(applied(key.clone(), id("__t")))),
                first_seen,
                expr_stmt(method_call_bare(hash_slot(), "push", vec![id("__t")])),
            ]);
            let rows = TsExpr::Index {
                object: Box::new(id("__rows")),
                index: Box::new(zero()),
            };
            let emit = block_arrow(
                vec![param("__k", None)],
                false,
                vec![
                    const_("__rows", None, hash_slot()),
                    ret(call(paren(into), vec![applied(key, rows), id("__rows")])),
                ],
            );
            let body = vec![
                const_("__h", Some(record.clone()), create_null()),
                const_(
                    "__order",
                    Some(TsType::array(TsType::named("string"))),
                    TsExpr::array(vec![]),
                ),
                for_of("__t", recv, partition),
                ret(method_call_bare(id("__order"), "map", vec![emit])),
            ];
            iife_block(vec![], false, body, vec![])
        },
        |t| {
            let [recv, key, into] = take(t);
            format!(
                "(() => {{ const __h: {record_ts} = globalThis.Object.create(null); const __order: string[] = []; for (const __t of {recv}) {{ const __k = String(({key})(__t)); if (!(__k in __h)) {{ __h[__k] = []; __order.push(__k); }} __h[__k].push(__t); }} return __order.map((__k) => {{ const __rows = __h[__k]; return ({into})(({key})(__rows[0]), __rows); }}); }})()"
            )
        },
    )
}

/// v0.94 (ADR 0120): the TS element type of a join's `other` collection (its
/// `List`/`Query` element), for typing the hash-map buckets. Falls back to
/// `unknown` if the checker recorded no type (an already-diagnosed program).
pub(super) fn join_other_elem(args: &[Expr], cx: &LowerCtx) -> TsType {
    let tys = cx.ty_render();
    match cx.commons().expr_ty(args[0].id).as_deref() {
        Some(Ty::List(u) | Ty::Query(u)) => ts_ty_to_ts_type(*u, tys),
        _ => TsType::named("unknown"),
    }
}

/// [`join_other_elem`] as text, for the `Query` kernel's still-text builders.
pub(super) fn join_other_elem_ts(args: &[Expr], cx: &LowerCtx) -> String {
    print_type(&join_other_elem(args, cx))
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
        Some(BaseType::Int | BaseType::String | BaseType::Duration | BaseType::Instant)
    );
    (!plain).then(|| cx.comparator(*ret))
}

/// Constrained generics slice 2: whether a `Map` key / `Set` element of this
/// static type is a JS primitive (`string`, `number`, `boolean`), so the plain
/// `Map`/`Set` compares it by value. Every other `Key` type (`Bytes`, `Option`,
/// `List`, a record, a sum, a type parameter) takes the keyed helpers in
/// `keys.ts`; a plain-keyed site emits exactly what it did before.
pub(super) fn key_is_plain(ty: TyId, tys: &Types) -> bool {
    match &*tys.get(ty) {
        Ty::Base(b) => !matches!(b, BaseType::Bytes | BaseType::Decimal),
        Ty::Named {
            kind: NamedKind::Refined(b) | NamedKind::Opaque(b),
            ..
        } => !matches!(b, BaseType::Bytes | BaseType::Decimal),
        _ => false,
    }
}

/// The result type of a function-typed argument (a kernel's key function).
pub(super) fn key_fn_ret(key: &Expr, cx: &LowerCtx) -> Option<TyId> {
    let tys = cx.ty_render();
    let fn_ty = cx.commons().expr_types.get(&key.id)?.ty;
    match &*tys.get(fn_ty) {
        Ty::Fn { ret, .. } => Some(*ret),
        _ => None,
    }
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
            arrow(vec![param("__c", None)], body(id("__c"))),
            vec![cmp.clone()],
        ),
    }
}

/// Bind comparator expression `cmp` as `__c` around `body`. A bare identifier (a
/// runtime comparator or an in-scope witness) is substituted directly; a built
/// comparator is evaluated once, outside the loop that calls it.
///
/// 1A: `body` builds text for the `Query` kernel, which stays text until its own
/// slice; the list kernel's arms use [`with_comparator_node`]. `cmp` is matched
/// as the node it is.
pub(super) fn with_comparator(cmp: &TsExpr, body: impl FnOnce(&str) -> String) -> String {
    if bare_comparator(cmp).is_some() {
        body(&bynk_ts::print_expr(cmp))
    } else {
        format!("((__c) => {})({})", body("__c"), bynk_ts::print_expr(cmp))
    }
}
