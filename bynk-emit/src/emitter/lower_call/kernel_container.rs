//! The container kernels (back-half paydown 1A.30, #3731): `Option`, `Result`, `Set` and `Map`.
//! Moved out of `lower.rs` byte for byte.

use bynk_check::checker::{Ty, TyId};
use bynk_check::kernel_op::{MapOp, OptionOp, ResultOp, SetOp};

use super::super::lower::{Lowered, Pre};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::key_is_plain;
use super::super::ts_types::{ts_array_elem, ts_ty};
use super::super::*;

/// v0.22a: lower a built-in `Option[T]` kernel method (ADR 0048). Typed
/// IIFEs in the v0.20b posture — no runtime imports beyond the
/// `Some`/`None`/`Ok`/`Err` constructors every module already has.
#[allow(clippy::too_many_arguments)]
pub(in crate::emitter) fn lower_option_kernel(
    e: &Expr,
    receiver: &Expr,
    op: OptionOp,
    args: &[Expr],
    inner: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let t = ts_ty(inner, tys);
    let text: Option<String> = match (op, args) {
        (OptionOp::Map, [f]) => {
            // The call's checked type is `Option[B]` — peel for B.
            let b = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Option(b)) => ts_ty(*b, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Option[_]` recorded \
                     for an `Option.map` call at {:?}, but found {other:?} — the checker \
                     should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__o: Option<{t}>, __f: (x: {t}) => {b}) => __o.tag === \"Some\" ? Some(__f(__o.value)) : None)({recv}, {f})"
            ))
        }
        (OptionOp::AndThen, [f]) => {
            let b = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Option(b)) => ts_ty(*b, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Option[_]` recorded \
                     for an `Option.andThen` call at {:?}, but found {other:?} — the checker \
                     should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__o: Option<{t}>, __f: (x: {t}) => Option<{b}>) => __o.tag === \"Some\" ? __f(__o.value) : None)({recv}, {f})"
            ))
        }
        (OptionOp::GetOrElse, [fallback]) => {
            let recv = pre.lower(receiver, cx);
            let fallback = pre.lower(fallback, cx);
            Some(format!(
                "((__o: Option<{t}>, __d: {t}) => __o.tag === \"Some\" ? __o.value : __d)({recv}, {fallback})"
            ))
        }
        (OptionOp::IsSome, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}.tag === \"Some\")"))
        }
        (OptionOp::OkOr, [error]) => {
            // The call's checked type is `Result[T, E]` — peel for E.
            let err = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Result(_, err)) => ts_ty(*err, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Result[_, _]` \
                     recorded for an `Option.okOr` call at {:?}, but found {other:?} — the \
                     checker should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let error = pre.lower(error, cx);
            Some(format!(
                "((__o: Option<{t}>, __e: {err}) => __o.tag === \"Some\" ? Ok(__o.value) : Err(__e))({recv}, {error})"
            ))
        }
        (OptionOp::IsNone, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}.tag === \"None\")"))
        }
        (OptionOp::OrElse, [f]) => {
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__o: Option<{t}>, __f: () => Option<{t}>) => __o.tag === \"Some\" ? __o : __f())({recv}, {f})"
            ))
        }
        (OptionOp::Filter, [p]) => {
            let recv = pre.lower(receiver, cx);
            let p = pre.lower(p, cx);
            Some(format!(
                "((__o: Option<{t}>, __p: (x: {t}) => boolean) => __o.tag === \"Some\" && __p(__o.value) ? __o : None)({recv}, {p})"
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.22a: lower a built-in `Result[T, E]` kernel method (ADR 0048). The
/// miss branches return the narrowed receiver — TS's discriminated-union
/// narrowing makes the `Err` arm assignable to `Result<B, E>` directly.
#[allow(clippy::too_many_arguments)]
pub(in crate::emitter) fn lower_result_kernel(
    e: &Expr,
    receiver: &Expr,
    op: ResultOp,
    args: &[Expr],
    ok: TyId,
    err: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let t = ts_ty(ok, tys);
    let et = ts_ty(err, tys);
    let text: Option<String> = match (op, args) {
        (ResultOp::Map, [f]) => {
            // The call's checked type is `Result[B, E]` — peel for B.
            let b = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Result(b, _)) => ts_ty(*b, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Result[_, _]` \
                     recorded for a `Result.map` call at {:?}, but found {other:?} — the \
                     checker should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__r: Result<{t}, {et}>, __f: (x: {t}) => {b}) => __r.tag === \"Ok\" ? Ok(__f(__r.value)) : __r)({recv}, {f})"
            ))
        }
        (ResultOp::AndThen, [f]) => {
            let b = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Result(b, _)) => ts_ty(*b, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Result[_, _]` \
                     recorded for a `Result.andThen` call at {:?}, but found {other:?} — the \
                     checker should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__r: Result<{t}, {et}>, __f: (x: {t}) => Result<{b}, {et}>) => __r.tag === \"Ok\" ? __f(__r.value) : __r)({recv}, {f})"
            ))
        }
        (ResultOp::MapErr, [f]) => {
            // The call's checked type is `Result[T, F]` — peel for F.
            let fts = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Result(_, f)) => ts_ty(*f, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Result[_, _]` \
                     recorded for a `Result.mapErr` call at {:?}, but found {other:?} — the \
                     checker should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__r: Result<{t}, {et}>, __f: (e: {et}) => {fts}) => __r.tag === \"Err\" ? Err(__f(__r.error)) : __r)({recv}, {f})"
            ))
        }
        (ResultOp::GetOrElse, [fallback]) => {
            let recv = pre.lower(receiver, cx);
            let fallback = pre.lower(fallback, cx);
            Some(format!(
                "((__r: Result<{t}, {et}>, __d: {t}) => __r.tag === \"Ok\" ? __r.value : __d)({recv}, {fallback})"
            ))
        }
        (ResultOp::IsOk, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}.tag === \"Ok\")"))
        }
        (ResultOp::IsErr, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}.tag === \"Err\")"))
        }
        (ResultOp::Ok, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!(
                "((__r: Result<{t}, {et}>) => __r.tag === \"Ok\" ? Some(__r.value) : None)({recv})"
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.20b: lower a built-in `Map` kernel method. `insert` copies — the
/// emitted `ReadonlyMap` is never mutated in place; updating an existing key
/// keeps its insertion position (JS `Map` semantics, normative in §7).
/// K6a (#2596): lower a `Set` kernel method over the `ReadonlySet`. `insert`
/// and `remove` copy, so the receiver is never mutated; a copy keeps the
/// insertion order, and inserting a present element keeps its position.
pub(in crate::emitter) fn lower_set_kernel(
    receiver: &Expr,
    op: SetOp,
    args: &[Expr],
    elem: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let elem_ts = ts_ty(elem, tys);
    let text: Option<String> = match (op, args) {
        (SetOp::Length, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).size"))
        }
        (SetOp::Contains, [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!("({recv}).has({x})"))
        }
        (SetOp::Insert, [x]) if !key_is_plain(elem, tys.types()) => {
            cx.runtime_use().note_key("__bynkSetInsert");
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!("__bynkSetInsert<{elem_ts}>({recv}, {x})"))
        }
        (SetOp::Remove, [x]) if !key_is_plain(elem, tys.types()) => {
            cx.runtime_use().note_key("__bynkSetRemove");
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!("__bynkSetRemove<{elem_ts}>({recv}, {x})"))
        }
        (SetOp::Insert, [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!(
                "(new globalThis.Set<{elem_ts}>({recv}).add({x}) as ReadonlySet<{elem_ts}>)"
            ))
        }
        (SetOp::Remove, [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!(
                "((__s: globalThis.Set<{elem_ts}>) => (__s.delete({x}), __s as ReadonlySet<{elem_ts}>))(new globalThis.Set<{elem_ts}>({recv}))"
            ))
        }
        (SetOp::ToList, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!(
                "([...({recv})] as readonly {}[])",
                ts_array_elem(&elem_ts)
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

pub(in crate::emitter) fn lower_map_kernel(
    receiver: &Expr,
    op: MapOp,
    args: &[Expr],
    key: TyId,
    val: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let key_ts = ts_ty(key, tys);
    let val_ts = ts_ty(val, tys);
    let text: Option<String> = match (op, args) {
        (MapOp::Length, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).size"))
        }
        (MapOp::Keys, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("[...({recv}).keys()]"))
        }
        // v0.149 (ADR 0173): the values in key order — the `keys()` sibling over
        // the in-memory `ReadonlyMap`.
        (MapOp::Values, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("[...({recv}).values()]"))
        }
        (MapOp::Get, [k]) => {
            let recv = pre.lower(receiver, cx);
            let k = pre.lower(k, cx);
            Some(format!(
                "((__m: ReadonlyMap<{key_ts}, {val_ts}>, __k: {key_ts}) => __m.has(__k) ? Some(__m.get(__k) as {val_ts}) : None)({recv}, {k})"
            ))
        }
        (MapOp::Insert, [k, v]) if !key_is_plain(key, tys.types()) => {
            cx.runtime_use().note_key("__bynkMapInsert");
            let recv = pre.lower(receiver, cx);
            let k = pre.lower(k, cx);
            let v = pre.lower(v, cx);
            Some(format!(
                "__bynkMapInsert<{key_ts}, {val_ts}>({recv}, {k}, {v})"
            ))
        }
        (MapOp::Insert, [k, v]) => {
            let recv = pre.lower(receiver, cx);
            let k = pre.lower(k, cx);
            let v = pre.lower(v, cx);
            Some(format!("new globalThis.Map({recv}).set({k}, {v})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}
