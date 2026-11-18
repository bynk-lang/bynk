//! The `List` kernel's lowering: `lower_list_kernel` and the helpers its arms
//! share. Split out of `lower.rs` (back-half paydown 1A.18a, #3719). The
//! dispatcher matches the checker's typed [`ListOp`]; each operation is its own
//! builder, and the pieces the sibling kernels will want (comparator binding,
//! keyed-helper choice, key-function types) are plain functions here.

use bynk_check::checker::{Ty, TyId, Types};
use bynk_check::kernel_op::ListOp;
use bynk_syntax::ast::{BaseType, Expr};

use super::lower::{Lowered, Pre};
use super::lower_ctx::LowerCtx;
use super::ts_types::{TyRender, ts_array_elem, ts_ty};
use super::*;

/// v0.150 (ADR 0174): peel a `traverseTry`/`parTraverseTry` call's checked type
/// `Effect[Result[List[U], E]]` to the `U` TS — the short-circuit collect's
/// output-array element annotation. Falls back to `any` if the shape is absent.
pub(super) fn list_ok_elem_ts(call_ty: Option<TyId>, tys: TyRender<'_>) -> String {
    match call_ty.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
            Ty::Result(ok, _) => match &*tys.get(*ok) {
                Ty::List(u) => ts_ty(*u, tys),
                _ => ts_ty(*ok, tys),
            },
            _ => ts_ty(*inner, tys),
        },
        _ => "any".to_string(),
    }
}

/// What every arm reads: the call, its receiver and arguments, and the element
/// type spelled once as a type (`elem_ts`) and as an array element (`elem_el`).
struct ListArm<'a> {
    e: &'a Expr,
    receiver: &'a Expr,
    args: &'a [Expr],
    elem_ts: String,
    elem_el: String,
}

/// v0.20b: lower a built-in `List` kernel method. Returns None for a method
/// name the kernel doesn't own (the checker has already rejected it; this
/// keeps the dispatch defensive). All forms are pure expressions; `foldEff`
/// returns a Promise that the surrounding `<-` bind awaits.
pub(super) fn lower_list_kernel(
    e: &Expr,
    receiver: &Expr,
    op: ListOp,
    args: &[Expr],
    elem: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let elem_ts = ts_ty(elem, tys);
    let a = ListArm {
        e,
        receiver,
        args,
        elem_el: ts_array_elem(&elem_ts),
        elem_ts,
    };
    let p = &mut pre;
    let text: Option<String> = match (op, args) {
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
    text.map(|expr| pre.finish_text(expr))
}

fn list_length(a: &ListArm, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let recv = pre.lower(a.receiver, cx);
    format!("({recv}).length")
}

fn list_get(a: &ListArm, index: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (elem_el, elem_ts) = (&a.elem_el, &a.elem_ts);
    let recv = pre.lower(a.receiver, cx);
    let idx = pre.lower(index, cx);
    format!(
        "((__xs: readonly {elem_el}[], __i: number) => __i >= 0 && __i < __xs.length ? Some(__xs[__i] as {elem_ts}) : None)({recv}, {idx})"
    )
}

fn list_prepend(a: &ListArm, head: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let head = pre.lower(head, cx);
    let recv = pre.lower(a.receiver, cx);
    format!("[{head}, ...{recv}]")
}

// #1889: the receiver lowers before the argument, as written, so any
// hoisted effects keep source order.
fn list_append(a: &ListArm, item: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let recv = pre.lower(a.receiver, cx);
    let item = pre.lower(item, cx);
    format!("[...{recv}, {item}]")
}

fn list_concat(a: &ListArm, other: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let recv = pre.lower(a.receiver, cx);
    let other = pre.lower(other, cx);
    format!("[...{recv}, ...{other}]")
}

fn list_fold(a: &ListArm, init: &Expr, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (e, elem_el, elem_ts) = (a.e, &a.elem_el, &a.elem_ts);
    let tys = cx.ty_render();
    // The call's checked type is the accumulator type.
    let acc_ts = cx
        .commons()
        .expr_types
        .get(&e.id)
        .map(|te| ts_ty(te.ty, tys))
        .unwrap_or_else(|| {
            panic!(
                "bynk internal error (finding #28): emitter has no recorded type for a \
                     `fold` call at {:?} — the checker should already have typed it",
                e.span
            )
        });
    let recv = pre.lower(a.receiver, cx);
    let init = pre.lower(init, cx);
    let f = pre.lower(f, cx);
    format!(
        "((__xs: readonly {elem_el}[], __acc: {acc_ts}, __f: (acc: {acc_ts}, x: {elem_ts}) => {acc_ts}) => {{ for (const __x of __xs) __acc = __f(__acc, __x); return __acc; }})({recv}, {init}, {f})"
    )
}

fn list_fold_eff(a: &ListArm, init: &Expr, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (e, elem_el, elem_ts) = (a.e, &a.elem_el, &a.elem_ts);
    let tys = cx.ty_render();
    // The call's checked type is `Effect[Acc]` — peel for the TS
    // accumulator annotation.
    let acc_ts = match cx.commons().expr_ty(e.id).as_deref() {
        Some(Ty::Effect(acc)) => ts_ty(*acc, tys),
        Some(_) => ts_ty(cx.commons().expr_types[&e.id].ty, tys),
        None => panic!(
            "bynk internal error (finding #28): emitter has no recorded type for a \
                     `foldEff` call at {:?} — the checker should already have typed it",
            e.span
        ),
    };
    let recv = pre.lower(a.receiver, cx);
    let init = pre.lower(init, cx);
    let f = pre.lower(f, cx);
    format!(
        "(async (__xs: readonly {elem_el}[], __acc: {acc_ts}, __f: (acc: {acc_ts}, x: {elem_ts}) => globalThis.Promise<{acc_ts}>) => {{ for (const __x of __xs) __acc = await __f(__acc, __x); return __acc; }})({recv}, {init}, {f})"
    )
}

// v0.146 (ADR 0170): `forEach` — run an effectful step per element in
// order, awaiting each; yields `Promise<void>`. The eager `List`
// analogue of the `Query.forEach` terminal, emitted inline.
fn list_for_each(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let elem_el = &a.elem_el;
    let recv = pre.lower(a.receiver, cx);
    let f = pre.lower(f, cx);
    format!(
        "(async (__xs: readonly {elem_el}[]) => {{ for (const __x of __xs) {{ await ({f})(__x); }} }})({recv})"
    )
}

// v0.147 (ADR 0171): `parTraverse` — issue the effectful fn over every
// element concurrently and await them together, so one slow element does
// not head-of-line-block the rest. The eager `List` analogue of the
// `Query.parTraverse` terminal, emitted inline.
fn list_par_traverse(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (elem_el, elem_ts) = (&a.elem_el, &a.elem_ts);
    cx.note_settle_all();
    let recv = pre.lower(a.receiver, cx);
    let f = pre.lower(f, cx);
    format!(
        "(async (__xs: readonly {elem_el}[]) => {{ await __bynkSettleAll(__xs.map((__x: {elem_ts}) => () => ({f})(__x))); }})({recv})"
    )
}

// v0.148 (ADR 0172): the collect-all iterators — every element's
// `Result` outcome is gathered (an `Err` is a value, so nothing rejects).
// `traverseAll` awaits each in order into a typed array; `parTraverseAll`
// issues all at once and `Promise.all`s the resolved `Result`s. Both
// yield `Promise<Result<…>[]>`.
fn list_traverse_all(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (e, elem_el) = (a.e, &a.elem_el);
    let tys = cx.ty_render();
    // The call's checked type is `Effect[List[Result[B, E]]]` — peel to
    // the `Result[B, E]` TS for the output-array annotation.
    let res_ts = match cx.commons().expr_ty(e.id).as_deref() {
        Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
            Ty::List(el) => ts_ty(*el, tys),
            _ => ts_ty(*inner, tys),
        },
        other => panic!(
            "bynk internal error (finding #28): emitter expected `Effect[List[_]]` \
                     recorded for a `traverseAll` call at {:?}, but found {other:?} — the \
                     checker should already have typed it",
            e.span
        ),
    };
    let res_el = ts_array_elem(&res_ts);
    let recv = pre.lower(a.receiver, cx);
    let f = pre.lower(f, cx);
    format!(
        "(async (__xs: readonly {elem_el}[]) => {{ const __out: {res_el}[] = []; for (const __x of __xs) {{ __out.push(await ({f})(__x)); }} return __out; }})({recv})"
    )
}

fn list_par_traverse_all(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (elem_el, elem_ts) = (&a.elem_el, &a.elem_ts);
    cx.note_settle_all();
    let recv = pre.lower(a.receiver, cx);
    let f = pre.lower(f, cx);
    format!(
        "(async (__xs: readonly {elem_el}[]) => await __bynkSettleAll(__xs.map((__x: {elem_ts}) => () => ({f})(__x))))({recv})"
    )
}

// #2806: pure `Result` accumulation. The call's checked type is
// `Result[List[T], E']`; `T` and `E` (the element's own `Err` type) type
// the accumulators.
fn list_sequence(a: &ListArm, name: ListOp, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (e, elem_el) = (a.e, &a.elem_el);
    let tys = cx.ty_render();
    let (ok_ts, err_ts) = match cx.commons().expr_ty(e.id).as_deref() {
        Some(Ty::Result(ok, err)) => {
            let ok_ts = match &*tys.get(*ok) {
                Ty::List(t) => ts_ty(*t, tys),
                _ => ts_ty(*ok, tys),
            };
            let err_ts = match &*tys.get(*err) {
                Ty::List(t) if name == ListOp::SequenceAll => ts_ty(*t, tys),
                _ => ts_ty(*err, tys),
            };
            (ok_ts, err_ts)
        }
        _ => ("any".to_string(), "any".to_string()),
    };
    let ok_el = ts_array_elem(&ok_ts);
    let err_el = ts_array_elem(&err_ts);
    let recv = pre.lower(a.receiver, cx);
    if name == ListOp::SequenceAll {
        format!(
            "((__xs: readonly {elem_el}[]) => {{ const __ok: {ok_el}[] = []; const __errs: {err_el}[] = []; for (const __r of __xs) {{ if (__r.tag === \"Err\") {{ __errs.push(__r.error); }} else {{ __ok.push(__r.value); }} }} return __errs.length > 0 ? Err(__errs) : Ok(__ok); }})({recv})"
        )
    } else {
        format!(
            "((__xs: readonly {elem_el}[]) => {{ const __ok: {ok_el}[] = []; for (const __r of __xs) {{ if (__r.tag === \"Err\") {{ return Err(__r.error); }} __ok.push(__r.value); }} return Ok(__ok); }})({recv})"
        )
    }
}

// v0.150 (ADR 0174): the short-circuit collect iterators — stop at the
// first `Err` and return it (`Effect[Result[List[U], E]]`). `traverseTry`
// awaits each in order, bailing on the first `Err`; `parTraverseTry`
// issues all at once, then scans the resolved `Result`s in input order.
fn list_traverse_try(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (e, elem_el) = (a.e, &a.elem_el);
    let tys = cx.ty_render();
    let u_ts = list_ok_elem_ts(cx.commons().expr_types.get(&e.id).map(|te| te.ty), tys);
    let u_el = ts_array_elem(&u_ts);
    let recv = pre.lower(a.receiver, cx);
    let f = pre.lower(f, cx);
    format!(
        "(async (__xs: readonly {elem_el}[]) => {{ const __out: {u_el}[] = []; for (const __x of __xs) {{ const __r = await ({f})(__x); if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})({recv})"
    )
}

fn list_par_traverse_try(a: &ListArm, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (e, elem_el, elem_ts) = (a.e, &a.elem_el, &a.elem_ts);
    let tys = cx.ty_render();
    cx.note_settle_all();
    let u_ts = list_ok_elem_ts(cx.commons().expr_types.get(&e.id).map(|te| te.ty), tys);
    let u_el = ts_array_elem(&u_ts);
    let recv = pre.lower(a.receiver, cx);
    let f = pre.lower(f, cx);
    format!(
        "(async (__xs: readonly {elem_el}[]) => {{ const __rs = await __bynkSettleAll(__xs.map((__x: {elem_ts}) => () => ({f})(__x))); const __out: {u_el}[] = []; for (const __r of __rs) {{ if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})({recv})"
    )
}

// v0.88 (ADR 0116): the eager builder/terminal vocabulary. Most lower
// to native array methods; callbacks are wrapped in a single-arg arrow
// so the array index/array extra args never reach a Bynk one-param fn.
// `map`, `filter`, `flatMap`, `any` (`some`) and `all` (`every`) share this.
fn list_callback(a: &ListArm, method: &str, f: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let elem_ts = &a.elem_ts;
    let recv = pre.lower(a.receiver, cx);
    let f = pre.lower(f, cx);
    format!("({recv}).{method}((__x: {elem_ts}) => ({f})(__x))")
}

fn list_take(a: &ListArm, n: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let recv = pre.lower(a.receiver, cx);
    let n = pre.lower(n, cx);
    format!("({recv}).slice(0, globalThis.Math.max(0, {n}))")
}

fn list_skip(a: &ListArm, n: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let recv = pre.lower(a.receiver, cx);
    let n = pre.lower(n, cx);
    format!("({recv}).slice(globalThis.Math.max(0, {n}))")
}

// v0.119 (ADR 0155, DECISION C-a): `run.upTo(step)` — the driven history
// strictly before `step`. Steps are distinct object instances in the run
// array, so `indexOf` is reference identity. An IIFE avoids re-evaluating
// the receiver.
fn list_up_to(a: &ListArm, step: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (elem_el, elem_ts) = (&a.elem_el, &a.elem_ts);
    let recv = pre.lower(a.receiver, cx);
    let step = pre.lower(step, cx);
    format!(
        "((__xs: readonly {elem_el}[], __s: {elem_ts}) => __xs.slice(0, __xs.indexOf(__s)))({recv}, {step})"
    )
}

fn list_first(a: &ListArm, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let elem_el = &a.elem_el;
    let recv = pre.lower(a.receiver, cx);
    format!("((__xs: readonly {elem_el}[]) => __xs.length > 0 ? Some(__xs[0]) : None)({recv})")
}

fn list_first_or_else(a: &ListArm, default: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (elem_el, elem_ts) = (&a.elem_el, &a.elem_ts);
    let recv = pre.lower(a.receiver, cx);
    let default = pre.lower(default, cx);
    format!(
        "((__xs: readonly {elem_el}[], __d: {elem_ts}) => __xs.length > 0 ? __xs[0] : __d)({recv}, {default})"
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
) -> String {
    let (elem_el, elem_ts) = (&a.elem_el, &a.elem_ts);
    let cmp = kernel_key_comparator(&a.args[0], cx);
    let recv = pre.lower(a.receiver, cx);
    let key = pre.lower(key, cx);
    // Descending swaps the comparator's arguments (and flips the bare form's signs).
    let (lo, hi, first, second) = if descending {
        ("1", "-1", "__b", "__a")
    } else {
        ("-1", "1", "__a", "__b")
    };
    match cmp {
        // A plain base key: the bare operators, as before.
        None => format!(
            "[...{recv}].sort((__a: {elem_ts}, __b: {elem_ts}) => {{ const __ka = ({key})(__a), __kb = ({key})(__b); return __ka < __kb ? {lo} : __ka > __kb ? {hi} : 0; }})"
        ),
        // Constrained generics slice 1 (DECISION H): `Float` (total
        // order, DECISION G), `Bytes`, and every other `Ord` key.
        // `recv` is an argument, not part of the arrow's body, so it
        // lowers exactly where it did before.
        Some(cmp) if cmp.chars().all(|c| c.is_alphanumeric() || c == '_') => format!(
            "[...{recv}].sort((__a: {elem_ts}, __b: {elem_ts}) => {cmp}(({key})({first}), ({key})({second})))"
        ),
        Some(cmp) => format!(
            "((__c, __r: readonly {elem_el}[]) => [...__r].sort((__a: {elem_ts}, __b: {elem_ts}) => __c(({key})({first}), ({key})({second}))))({cmp}, {recv})"
        ),
    }
}

/// `distinct`, `distinctBy`, `joinOn`, `leftJoin` and `groupBy` over a key that
/// is not a JS primitive: the `keys` runtime helper `helper` takes the
/// receiver, then each remaining argument in order. `note_key` fires before
/// anything lowers.
fn list_keyed(a: &ListArm, helper: &'static str, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    cx.runtime_use().note_key(helper);
    let recv = pre.lower(a.receiver, cx);
    let rest: Vec<String> = a.args.iter().map(|x| pre.lower(x, cx)).collect();
    format!(
        "{helper}({recv}{})",
        rest.iter().map(|r| format!(", {r}")).collect::<String>()
    )
}

fn list_distinct(a: &ListArm, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let recv = pre.lower(a.receiver, cx);
    format!("[...new globalThis.Set({recv})]")
}

fn list_distinct_by(a: &ListArm, key: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let elem_el = &a.elem_el;
    let recv = pre.lower(a.receiver, cx);
    let key = pre.lower(key, cx);
    format!(
        "((__xs: readonly {elem_el}[]) => {{ const __seen = new globalThis.Set(); const __out: {elem_el}[] = []; for (const __x of __xs) {{ const __k = ({key})(__x); if (!__seen.has(__k)) {{ __seen.add(__k); __out.push(__x); }} }} return __out; }})({recv})"
    )
}

fn list_sum(a: &ListArm, key: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let elem_ts = &a.elem_ts;
    let tys = cx.ty_render();
    if key_fn_ret(key, cx).and_then(|t| t.base(tys.types())) == Some(BaseType::Decimal) {
        // #2622: an exact sum, from the zero decimal.
        let recv = pre.lower(a.receiver, cx);
        let key = pre.lower(key, cx);
        cx.note_decimal("__bynkDecimalAdd");
        format!(
            "({recv}).reduce((__s: {}, __x: {elem_ts}) => __bynkDecimalAdd(__s, ({key})(__x)), {})",
            crate::emitter::ts_types::DECIMAL_TS_TYPE,
            crate::emitter::ts_types::decimal_ts_literal("0", 0)
        )
    } else {
        let recv = pre.lower(a.receiver, cx);
        let key = pre.lower(key, cx);
        format!("({recv}).reduce((__s: number, __x: {elem_ts}) => __s + ({key})(__x), 0)")
    }
}

fn list_min_max(a: &ListArm, op: ListOp, key: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let elem_el = &a.elem_el;
    let cmp = if op == ListOp::Min { "<" } else { ">" };
    let key_cmp = kernel_key_comparator(&a.args[0], cx);
    let recv = pre.lower(a.receiver, cx);
    let key = pre.lower(key, cx);
    match key_cmp {
        None => format!(
            "((__xs: readonly {elem_el}[]) => {{ if (__xs.length === 0) return None; let __m = ({key})(__xs[0]); for (const __x of __xs) {{ const __k = ({key})(__x); if (__k {cmp} __m) __m = __k; }} return Some(__m); }})({recv})"
        ),
        Some(key_cmp) => with_comparator(&key_cmp, |c| {
            format!(
                "((__xs: readonly {elem_el}[]) => {{ if (__xs.length === 0) return None; let __m = ({key})(__xs[0]); for (const __x of __xs) {{ const __k = ({key})(__x); if ({c}(__k, __m) {cmp} 0) __m = __k; }} return Some(__m); }})({recv})"
            )
        }),
    }
}

fn list_average(a: &ListArm, key: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let (e, elem_el) = (a.e, &a.elem_el);
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
    let recv = pre.lower(a.receiver, cx);
    let key = pre.lower(key, cx);
    format!(
        "((__xs: readonly {elem_el}[]) => {{ if (__xs.length === 0) return None; let __s = 0; for (const __x of __xs) __s += ({key})(__x); return Some({mean}); }})({recv})"
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
) -> String {
    let elem_ts = &a.elem_ts;
    let recv = pre.lower(a.receiver, cx);
    let u_ts = join_other_elem_ts(a.args, cx);
    let u_el = ts_array_elem(&u_ts);
    let other = pre.lower(other, cx);
    let left = pre.lower(left, cx);
    let right = pre.lower(right, cx);
    let into = pre.lower(into, cx);
    if left_join {
        format!(
            "(() => {{ const __h: globalThis.Record<string, {u_el}[]> = globalThis.Object.create(null); for (const __u of {other}) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return ({recv}).flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.length > 0 ? __m.map((__u: {u_ts}) => ({into})(__t, Some(__u))) : [({into})(__t, None)]; }}); }})()"
        )
    } else {
        format!(
            "(() => {{ const __h: globalThis.Record<string, {u_el}[]> = globalThis.Object.create(null); for (const __u of {other}) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return ({recv}).flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.map((__u: {u_ts}) => ({into})(__t, __u)); }}); }})()"
        )
    }
}

fn list_join(
    a: &ListArm,
    other: &Expr,
    on: &Expr,
    into: &Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> String {
    let elem_ts = &a.elem_ts;
    let recv = pre.lower(a.receiver, cx);
    let u_ts = join_other_elem_ts(a.args, cx);
    let u_el = ts_array_elem(&u_ts);
    let other = pre.lower(other, cx);
    let on = pre.lower(on, cx);
    let into = pre.lower(into, cx);
    format!(
        "(() => {{ const __b: readonly {u_el}[] = {other}; return ({recv}).flatMap((__t: {elem_ts}) => __b.filter((__u: {u_ts}) => ({on})(__t, __u)).map((__u: {u_ts}) => ({into})(__t, __u))); }})()"
    )
}

fn list_group_by(a: &ListArm, key: &Expr, into: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> String {
    let elem_el = &a.elem_el;
    let recv = pre.lower(a.receiver, cx);
    let key = pre.lower(key, cx);
    let into = pre.lower(into, cx);
    format!(
        "(() => {{ const __h: globalThis.Record<string, {elem_el}[]> = globalThis.Object.create(null); const __order: string[] = []; for (const __t of {recv}) {{ const __k = String(({key})(__t)); if (!(__k in __h)) {{ __h[__k] = []; __order.push(__k); }} __h[__k].push(__t); }} return __order.map((__k) => {{ const __rows = __h[__k]; return ({into})(({key})(__rows[0]), __rows); }}); }})()"
    )
}

/// v0.94 (ADR 0120): the TS element type of a join's `other` collection (its
/// `List`/`Query` element), for typing the hash-map buckets. Falls back to
/// `unknown` if the checker recorded no type (an already-diagnosed program).
pub(super) fn join_other_elem_ts(args: &[Expr], cx: &LowerCtx) -> String {
    let tys = cx.ty_render();
    match cx.commons().expr_ty(args[0].id).as_deref() {
        Some(Ty::List(u) | Ty::Query(u)) => ts_ty(*u, tys),
        _ => "unknown".to_string(),
    }
}

/// The comparator an ordering kernel (`sortBy`/`min`/`max`) needs for its key
/// function's result type, or `None` when the key is a plain base type that keeps
/// the bare `<`/`>` (constrained-generics slice 1, DECISION H). `Float` is not
/// plain: the kernels use its total order (DECISION G), so `NaN` sorts last
/// instead of making the sort inconsistent. A key type the checker did not record
/// keeps the bare operators.
pub(super) fn kernel_key_comparator(key: &Expr, cx: &LowerCtx) -> Option<String> {
    let tys = cx.ty_render();
    let fn_ty = cx.commons().expr_types.get(&key.id)?.ty;
    let Ty::Fn { ret, .. } = &*tys.get(fn_ty) else {
        return None;
    };
    let plain = matches!(
        ret.base(tys.types()),
        Some(BaseType::Int | BaseType::String | BaseType::Duration | BaseType::Instant)
    );
    // 1A: witness printed for a text caller
    (!plain).then(|| bynk_ts::print_expr(&cx.comparator(*ret)))
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

/// Bind comparator expression `cmp` as `__c` around `body`. A bare identifier (a
/// runtime comparator or an in-scope witness) is substituted directly; a built
/// comparator is evaluated once, outside the loop that calls it.
pub(super) fn with_comparator(cmp: &str, body: impl FnOnce(&str) -> String) -> String {
    if cmp.chars().all(|c| c.is_alphanumeric() || c == '_') {
        body(cmp)
    } else {
        format!("((__c) => {})({cmp})", body("__c"))
    }
}
