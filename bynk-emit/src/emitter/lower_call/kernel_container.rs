//! The container kernels (back-half paydown 1A.30, #3731, Decision D): `Option`, `Result`,
//! `Set` and `Map`. Moved out of `lower.rs` byte for byte, then converted to nodes: each arm
//! builds a node over its receiver and arguments.
//!
//! The plain `Set.remove`'s body is a comma expression, built as a `Sequence` node (1A.35a
//! part B). Every `note_key` fires before the receiver is lowered,
//! as it did, and the finding #28 panics are unchanged.

use bynk_check::checker::{Ty, TyId};
use bynk_check::kernel_op::{MapOp, OptionOp, ResultOp, SetOp};
use bynk_ts::{TsBinaryOp, TsExpr, TsParam, TsType};

use super::super::lower::{Lowered, Pre};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{fn_type, key_is_plain, take};
use super::super::ts_types::ts_ty_to_ts_type;
use super::super::*;
use super::kernel::receiver_object;
use crate::ts_build::{
    arrow, as_expr as cast, binary, call, cond_expr as cond, ident, member, new, param, paren,
    spread, str_lit as str_node, type_app,
};

/// The receiver and then each argument, lowered in order.
fn operands(pre: &mut Pre, cx: &mut LowerCtx, parts: &[&Expr]) -> Vec<bynk_ts::TsExpr> {
    parts.iter().map(|e| pre.lower_node(e, cx)).collect()
}

/// A kernel's exit: the built node, finished over the hoists collected so far.
fn finish(pre: Pre, built: Option<bynk_ts::TsExpr>) -> Option<Lowered> {
    built.map(|b| pre.finish(b))
}

/// `Option<T>`.
fn option_of(t: &TsType) -> TsType {
    TsType::named_with_args("Option", vec![t.clone()])
}

/// `Result<T, E>`.
fn result_of(t: &TsType, e: &TsType) -> TsType {
    TsType::named_with_args("Result", vec![t.clone(), e.clone()])
}

/// `globalThis.<name>`.
fn global(name: &str) -> TsExpr {
    member(ident("globalThis"), name)
}

/// `name.prop`.
fn field(name: &str, prop: &str) -> TsExpr {
    member(ident(name), prop)
}

/// `name.tag === "<tag>"`.
fn tag_is(name: &str, tag: &str) -> TsExpr {
    binary(TsBinaryOp::StrictEq, field(name, "tag"), str_node(tag))
}

/// An immediately-invoked typed arrow over the lowered operands: the printer brackets the arrow.
fn iife(params: Vec<TsParam>, body: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    call(arrow(params, body), args)
}

/// `(recv.tag === "<tag>")`: the brackets stay, as the text wrote them, so the test still reads as
/// one operand inside an unconverted text parent.
fn is_tag(recv: bynk_ts::TsExpr, tag: &'static str) -> bynk_ts::TsExpr {
    let n = vec![recv];
    let [recv] = take::<1, _>(n);
    paren(binary(
        TsBinaryOp::StrictEq,
        member(receiver_object(recv), "tag"),
        str_node(tag),
    ))
}

/// The `Option[B]` or `Result[_, _]` a combinator call was checked at, peeled with `peel`. A
/// call the checker recorded at another type is a missing fact (1A.34): it is reported at the
/// call, and `placeholder` (the receiver's own type) stands in so lowering continues. The output
/// of a unit with a fault is never used.
fn call_ty(
    e: &Expr,
    cx: &LowerCtx,
    what: &str,
    expected: &str,
    placeholder: TyId,
    peel: impl FnOnce(&Ty) -> Option<TyId>,
) -> TyId {
    match cx.commons().expr_ty(e.id).as_deref().and_then(peel) {
        Some(t) => t,
        None => {
            cx.missing_fact_ty(e.span, format_args!("`{expected}` type for {what}"));
            placeholder
        }
    }
}

/// v0.22a: lower a built-in `Option[T]` kernel method (ADR 0048). Typed
/// IIFEs in the v0.20b posture — no runtime imports beyond the
/// `Some`/`None`/`Ok`/`Err` constructors every module already has.
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
    let t = ts_ty_to_ts_type(inner, tys);
    let opt = option_of(&t);
    let built: Option<bynk_ts::TsExpr> = match (op, args) {
        (OptionOp::Map, [f]) => {
            // The call's checked type is `Option[B]` — peel for B.
            let b = call_ty(
                e,
                cx,
                "an `Option.map` call",
                "Option[_]",
                inner,
                Ty::as_option,
            );
            let b = ts_ty_to_ts_type(b, tys);
            let kids = operands(&mut pre, cx, &[receiver, f]);
            Some({
                let n = kids;
                let params = vec![
                    param("__o", Some(opt.clone())),
                    param("__f", Some(fn_type(&[("x", &t)], b))),
                ];
                let body = cond(
                    tag_is("__o", "Some"),
                    call(
                        ident("Some"),
                        vec![call(ident("__f"), vec![field("__o", "value")])],
                    ),
                    ident("None"),
                );
                iife(params, body, n)
            })
        }
        (OptionOp::AndThen, [f]) => {
            let b = call_ty(
                e,
                cx,
                "an `Option.andThen` call",
                "Option[_]",
                inner,
                Ty::as_option,
            );
            let b = ts_ty_to_ts_type(b, tys);
            let kids = operands(&mut pre, cx, &[receiver, f]);
            Some({
                let n = kids;
                let params = vec![
                    param("__o", Some(opt.clone())),
                    param("__f", Some(fn_type(&[("x", &t)], option_of(&b)))),
                ];
                let body = cond(
                    tag_is("__o", "Some"),
                    call(ident("__f"), vec![field("__o", "value")]),
                    ident("None"),
                );
                iife(params, body, n)
            })
        }
        (OptionOp::GetOrElse, [fallback]) => {
            let kids = operands(&mut pre, cx, &[receiver, fallback]);
            Some({
                let n = kids;
                let params = vec![
                    param("__o", Some(opt.clone())),
                    param("__d", Some(t.clone())),
                ];
                let body = cond(tag_is("__o", "Some"), field("__o", "value"), ident("__d"));
                iife(params, body, n)
            })
        }
        (OptionOp::IsSome, []) => {
            let recv = pre.lower_node(receiver, cx);
            Some(is_tag(recv, "Some"))
        }
        (OptionOp::OkOr, [error]) => {
            // The call's checked type is `Result[T, E]` — peel for E.
            let err = call_ty(
                e,
                cx,
                "an `Option.okOr` call",
                "Result[_, _]",
                inner,
                |ty| ty.as_result().map(|(_, err)| err),
            );
            let err = ts_ty_to_ts_type(err, tys);
            let kids = operands(&mut pre, cx, &[receiver, error]);
            Some({
                let n = kids;
                let params = vec![param("__o", Some(opt.clone())), param("__e", Some(err))];
                let body = cond(
                    tag_is("__o", "Some"),
                    call(ident("Ok"), vec![field("__o", "value")]),
                    call(ident("Err"), vec![ident("__e")]),
                );
                iife(params, body, n)
            })
        }
        (OptionOp::IsNone, []) => {
            let recv = pre.lower_node(receiver, cx);
            Some(is_tag(recv, "None"))
        }
        (OptionOp::OrElse, [f]) => {
            let kids = operands(&mut pre, cx, &[receiver, f]);
            Some({
                let n = kids;
                let params = vec![
                    param("__o", Some(opt.clone())),
                    param("__f", Some(fn_type(&[], opt.clone()))),
                ];
                let body = cond(
                    tag_is("__o", "Some"),
                    ident("__o"),
                    call(ident("__f"), vec![]),
                );
                iife(params, body, n)
            })
        }
        (OptionOp::Filter, [p]) => {
            let kids = operands(&mut pre, cx, &[receiver, p]);
            Some({
                let n = kids;
                let params = vec![
                    param("__o", Some(opt.clone())),
                    param("__p", Some(fn_type(&[("x", &t)], TsType::named("boolean")))),
                ];
                let body = cond(
                    binary(
                        TsBinaryOp::And,
                        tag_is("__o", "Some"),
                        call(ident("__p"), vec![field("__o", "value")]),
                    ),
                    ident("__o"),
                    ident("None"),
                );
                iife(params, body, n)
            })
        }
        _ => None,
    };
    finish(pre, built)
}

/// v0.22a: lower a built-in `Result[T, E]` kernel method (ADR 0048). The
/// miss branches return the narrowed receiver — TS's discriminated-union
/// narrowing makes the `Err` arm assignable to `Result<B, E>` directly.
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
    let t = ts_ty_to_ts_type(ok, tys);
    let et = ts_ty_to_ts_type(err, tys);
    let res = result_of(&t, &et);
    let built: Option<bynk_ts::TsExpr> = match (op, args) {
        (ResultOp::Map, [f]) => {
            // The call's checked type is `Result[B, E]` — peel for B.
            let b = call_ty(e, cx, "a `Result.map` call", "Result[_, _]", ok, |ty| {
                ty.as_result().map(|(ok, _)| ok)
            });
            let b = ts_ty_to_ts_type(b, tys);
            let kids = operands(&mut pre, cx, &[receiver, f]);
            Some({
                let n = kids;
                let params = vec![
                    param("__r", Some(res.clone())),
                    param("__f", Some(fn_type(&[("x", &t)], b))),
                ];
                let body = cond(
                    tag_is("__r", "Ok"),
                    call(
                        ident("Ok"),
                        vec![call(ident("__f"), vec![field("__r", "value")])],
                    ),
                    ident("__r"),
                );
                iife(params, body, n)
            })
        }
        (ResultOp::AndThen, [f]) => {
            let b = call_ty(e, cx, "a `Result.andThen` call", "Result[_, _]", ok, |ty| {
                ty.as_result().map(|(ok, _)| ok)
            });
            let b = ts_ty_to_ts_type(b, tys);
            let kids = operands(&mut pre, cx, &[receiver, f]);
            Some({
                let n = kids;
                let params = vec![
                    param("__r", Some(res.clone())),
                    param("__f", Some(fn_type(&[("x", &t)], result_of(&b, &et)))),
                ];
                let body = cond(
                    tag_is("__r", "Ok"),
                    call(ident("__f"), vec![field("__r", "value")]),
                    ident("__r"),
                );
                iife(params, body, n)
            })
        }
        (ResultOp::MapErr, [f]) => {
            // The call's checked type is `Result[T, F]` — peel for F.
            let fty = call_ty(e, cx, "a `Result.mapErr` call", "Result[_, _]", ok, |ty| {
                ty.as_result().map(|(_, err)| err)
            });
            let fnode = ts_ty_to_ts_type(fty, tys);
            let kids = operands(&mut pre, cx, &[receiver, f]);
            Some({
                let n = kids;
                let params = vec![
                    param("__r", Some(res.clone())),
                    param("__f", Some(fn_type(&[("e", &et)], fnode))),
                ];
                let body = cond(
                    tag_is("__r", "Err"),
                    call(
                        ident("Err"),
                        vec![call(ident("__f"), vec![field("__r", "error")])],
                    ),
                    ident("__r"),
                );
                iife(params, body, n)
            })
        }
        (ResultOp::GetOrElse, [fallback]) => {
            let kids = operands(&mut pre, cx, &[receiver, fallback]);
            Some({
                let n = kids;
                let params = vec![
                    param("__r", Some(res.clone())),
                    param("__d", Some(t.clone())),
                ];
                let body = cond(tag_is("__r", "Ok"), field("__r", "value"), ident("__d"));
                iife(params, body, n)
            })
        }
        (ResultOp::IsOk, []) => {
            let recv = pre.lower_node(receiver, cx);
            Some(is_tag(recv, "Ok"))
        }
        (ResultOp::IsErr, []) => {
            let recv = pre.lower_node(receiver, cx);
            Some(is_tag(recv, "Err"))
        }
        (ResultOp::Ok, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some({
                let n = kids;
                let params = vec![param("__r", Some(res.clone()))];
                let body = cond(
                    tag_is("__r", "Ok"),
                    call(ident("Some"), vec![field("__r", "value")]),
                    ident("None"),
                );
                iife(params, body, n)
            })
        }
        _ => None,
    };
    finish(pre, built)
}

/// `recv.size` / `(recv).size`-style member of a container receiver.
fn member_of(recv: bynk_ts::TsExpr, prop: &'static str) -> bynk_ts::TsExpr {
    let n = vec![recv];
    let [recv] = take::<1, _>(n);
    member(receiver_object(recv), prop)
}

/// `recv.method(args)`; the first child is the receiver.
fn method_of(kids: Vec<bynk_ts::TsExpr>, method: &'static str) -> bynk_ts::TsExpr {
    let mut n = kids;
    let recv = receiver_object(n.remove(0));
    call(member(recv, method), n)
}

/// `[...recv.<method>()]`: the iterator of a `Map` spread into an array.
fn spread_iter(recv: bynk_ts::TsExpr, method: &'static str) -> bynk_ts::TsExpr {
    let n = vec![recv];
    let [recv] = take::<1, _>(n);
    TsExpr::Array {
        items: vec![spread(call(member(receiver_object(recv), method), vec![]))],
        multiline: false,
    }
}

/// `__bynkSetInsert<T>(recv, x)`-style keyed helper call over the lowered operands.
fn keyed_helper(
    name: &'static str,
    type_args: Vec<TsType>,
    kids: Vec<bynk_ts::TsExpr>,
) -> bynk_ts::TsExpr {
    let n = kids;
    call(type_app(ident(name), type_args.clone()), n)
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
    let elem_t = ts_ty_to_ts_type(elem, tys);
    let built: Option<bynk_ts::TsExpr> = match (op, args) {
        (SetOp::Length, []) => {
            let recv = pre.lower_node(receiver, cx);
            Some(member_of(recv, "size"))
        }
        (SetOp::Contains, [x]) => {
            let kids = operands(&mut pre, cx, &[receiver, x]);
            Some(method_of(kids, "has"))
        }
        (SetOp::Insert, [x]) if !key_is_plain(elem, tys.types()) => {
            cx.runtime_use().note_key("__bynkSetInsert");
            let kids = operands(&mut pre, cx, &[receiver, x]);
            Some(keyed_helper("__bynkSetInsert", vec![elem_t], kids))
        }
        (SetOp::Remove, [x]) if !key_is_plain(elem, tys.types()) => {
            cx.runtime_use().note_key("__bynkSetRemove");
            let kids = operands(&mut pre, cx, &[receiver, x]);
            Some(keyed_helper("__bynkSetRemove", vec![elem_t], kids))
        }
        (SetOp::Insert, [x]) => {
            let kids = operands(&mut pre, cx, &[receiver, x]);
            Some({
                let n = kids;
                let [recv, x] = take::<2, _>(n);
                let set = new(type_app(global("Set"), vec![elem_t.clone()]), vec![recv]);
                paren(cast(
                    call(member(set, "add"), vec![x]),
                    TsType::named_with_args("ReadonlySet", vec![elem_t.clone()]),
                ))
            })
        }
        // Delete from a copy and return it: `(__s) => (__s.delete(x), __s as ReadonlySet<T>)`
        // applied to `new Set<T>(recv)`, the body a comma expression (1A.35a part B).
        (SetOp::Remove, [x]) => {
            let kids = operands(&mut pre, cx, &[receiver, x]);
            Some({
                let n = kids;
                let [recv, x] = take::<2, _>(n);
                let set_ty = TsType::named_with_args("globalThis.Set", vec![elem_t.clone()]);
                let remove = arrow(
                    vec![param("__s", Some(set_ty))],
                    TsExpr::Sequence(vec![
                        call(member(ident("__s"), "delete"), vec![x]),
                        cast(
                            ident("__s"),
                            TsType::named_with_args("ReadonlySet", vec![elem_t.clone()]),
                        ),
                    ]),
                );
                call(
                    remove,
                    vec![new(
                        type_app(global("Set"), vec![elem_t.clone()]),
                        vec![recv],
                    )],
                )
            })
        }
        (SetOp::ToList, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            Some({
                let n = kids;
                let [recv] = take::<1, _>(n);
                paren(cast(
                    TsExpr::Array {
                        items: vec![spread(recv)],
                        multiline: false,
                    },
                    TsType::readonly_array(elem_t.clone()),
                ))
            })
        }
        _ => None,
    };
    finish(pre, built)
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
    let key_t = ts_ty_to_ts_type(key, tys);
    let val_t = ts_ty_to_ts_type(val, tys);
    let built: Option<bynk_ts::TsExpr> = match (op, args) {
        (MapOp::Length, []) => {
            let recv = pre.lower_node(receiver, cx);
            Some(member_of(recv, "size"))
        }
        (MapOp::Keys, []) => {
            let recv = pre.lower_node(receiver, cx);
            Some(spread_iter(recv, "keys"))
        }
        // v0.149 (ADR 0173): the values in key order — the `keys()` sibling over
        // the in-memory `ReadonlyMap`.
        (MapOp::Values, []) => {
            let recv = pre.lower_node(receiver, cx);
            Some(spread_iter(recv, "values"))
        }
        (MapOp::Get, [k]) => {
            let kids = operands(&mut pre, cx, &[receiver, k]);
            Some({
                let n = kids;
                let params = vec![
                    param(
                        "__m",
                        Some(TsType::named_with_args(
                            "ReadonlyMap",
                            vec![key_t.clone(), val_t.clone()],
                        )),
                    ),
                    param("__k", Some(key_t.clone())),
                ];
                let body = cond(
                    call(field("__m", "has"), vec![ident("__k")]),
                    call(
                        ident("Some"),
                        vec![cast(
                            call(field("__m", "get"), vec![ident("__k")]),
                            val_t.clone(),
                        )],
                    ),
                    ident("None"),
                );
                iife(params, body, n)
            })
        }
        (MapOp::Insert, [k, v]) if !key_is_plain(key, tys.types()) => {
            cx.runtime_use().note_key("__bynkMapInsert");
            let kids = operands(&mut pre, cx, &[receiver, k, v]);
            Some(keyed_helper("__bynkMapInsert", vec![key_t, val_t], kids))
        }
        (MapOp::Insert, [k, v]) => {
            let kids = operands(&mut pre, cx, &[receiver, k, v]);
            Some({
                let n = kids;
                let [recv, k, v] = take::<3, _>(n);
                call(member(new(global("Map"), vec![recv]), "set"), vec![k, v])
            })
        }
        _ => None,
    };
    finish(pre, built)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emitter::ctx::{BodyMode, ModuleCtx, UnitFacts};
    use crate::emitter::runtime_use::RuntimeUse;
    use crate::testkit::emit_source;
    use bynk_check::checker::{ExprId, TypedCommons, TypedExpr};
    use bynk_syntax::span::Span;

    /// The emitted TypeScript for a one-function commons.
    fn emitted(sig: &str, body: &str) -> String {
        emit_source(&format!("commons kc\nfn f{sig} -> {body}\n"))
    }

    /// The returned expression of `export function f`.
    fn returned(ts: &str) -> String {
        let at = ts.find("export function f").expect("f");
        let rest = &ts[at..];
        let ret = rest.find("  return ").expect("a return") + "  return ".len();
        let end = rest[ret..].find(";\n}\n").expect("end");
        rest[ret..ret + end].to_string()
    }

    const OPT: &str = "((__o: Option<number>, __";
    const RES: &str = "((__r: Result<number, string>, __";

    /// Every `OptionOp` and `ResultOp` over identifier arguments: the exact print.
    #[test]
    fn every_option_and_result_op_prints_as_a_node() {
        let o = "(o: Option[Int], f: (Int) -> Int, g: (Int) -> Option[Int], h: () -> Option[Int], p: (Int) -> Bool, d: Int, e: String)";
        let r = "(r: Result[Int, String], f: (Int) -> Int, g: (Int) -> Result[Int, String], m: (String) -> Int, d: Int)";
        for (sig, body, want) in [
            (o, "Option[Int] { o.map(f) }", format!("{OPT}f: (x: number) => number) => __o.tag === \"Some\" ? Some(__f(__o.value)) : None)(o, f)")),
            (o, "Option[Int] { o.andThen(g) }", format!("{OPT}f: (x: number) => Option<number>) => __o.tag === \"Some\" ? __f(__o.value) : None)(o, g)")),
            (o, "Int { o.getOrElse(d) }", format!("{OPT}d: number) => __o.tag === \"Some\" ? __o.value : __d)(o, d)")),
            (o, "Bool { o.isSome() }", "(o.tag === \"Some\")".to_string()),
            (o, "Bool { o.isNone() }", "(o.tag === \"None\")".to_string()),
            (o, "Result[Int, String] { o.okOr(e) }", format!("{OPT}e: string) => __o.tag === \"Some\" ? Ok(__o.value) : Err(__e))(o, e)")),
            (o, "Option[Int] { o.orElse(h) }", format!("{OPT}f: () => Option<number>) => __o.tag === \"Some\" ? __o : __f())(o, h)")),
            (o, "Option[Int] { o.filter(p) }", format!("{OPT}p: (x: number) => boolean) => __o.tag === \"Some\" && __p(__o.value) ? __o : None)(o, p)")),
            (r, "Result[Int, String] { r.map(f) }", format!("{RES}f: (x: number) => number) => __r.tag === \"Ok\" ? Ok(__f(__r.value)) : __r)(r, f)")),
            (r, "Result[Int, String] { r.andThen(g) }", format!("{RES}f: (x: number) => Result<number, string>) => __r.tag === \"Ok\" ? __f(__r.value) : __r)(r, g)")),
            (r, "Result[Int, Int] { r.mapErr(m) }", "((__r: Result<number, string>, __f: (e: string) => number) => __r.tag === \"Err\" ? Err(__f(__r.error)) : __r)(r, m)".to_string()),
            (r, "Int { r.getOrElse(d) }", format!("{RES}d: number) => __r.tag === \"Ok\" ? __r.value : __d)(r, d)")),
            (r, "Option[Int] { r.ok() }", "((__r: Result<number, string>) => __r.tag === \"Ok\" ? Some(__r.value) : None)(r)".to_string()),
            (r, "Bool { r.isOk() }", "(r.tag === \"Ok\")".to_string()),
            (r, "Bool { r.isErr() }", "(r.tag === \"Err\")".to_string()),
        ] {
            let ts = emitted(sig, body);
            assert_eq!(returned(&ts), want, "{body}");
        }
    }

    /// Typed and inferred closures, a generic element type and a nested `Option[Result[..]]` receiver
    /// keep their type arguments.
    #[test]
    fn element_types_keep_their_type_arguments() {
        for (sig, body, want) in [
            (
                "(o: Option[Int])",
                "Option[Int] { o.map((x) => x + 1) }",
                "((__o: Option<number>, __f: (x: number) => number) => __o.tag === \"Some\" ? Some(__f(__o.value)) : None)(o, (x: number) => x + 1)",
            ),
            (
                "[T](o: Option[T], d: T)",
                "T { o.getOrElse(d) }",
                "((__o: Option<T>, __d: T) => __o.tag === \"Some\" ? __o.value : __d)(o, d)",
            ),
            (
                "(o: Option[Result[Int, String]], d: Result[Int, String])",
                "Result[Int, String] { o.getOrElse(d) }",
                "((__o: Option<Result<number, string>>, __d: Result<number, string>) => __o.tag === \"Some\" ? __o.value : __d)(o, d)",
            ),
            ("(s: Set[List[Int]])", "Int { s.length() }", "s.size"),
        ] {
            let ts = emitted(sig, body);
            assert_eq!(returned(&ts), want, "{body}");
        }
    }

    /// Every `SetOp` and `MapOp` over a plain and a keyed type: the exact print, the helper's import
    /// noted only on the keyed path.
    #[test]
    fn set_and_map_ops_print_plain_and_keyed_forms() {
        let s = "(s: Set[String], x: String)";
        let ks = "(s: Set[Bytes], x: Bytes)";
        let m = "(m: Map[String, Int], k: String, v: Int)";
        let km = "(m: Map[Bytes, Int], k: Bytes, v: Int)";
        for (sig, body, want, import) in [
            (s, "Int { s.length() }", "s.size", None),
            (s, "Bool { s.contains(x) }", "s.has(x)", None),
            (
                s,
                "Set[String] { s.insert(x) }",
                "(new globalThis.Set<string>(s).add(x) as ReadonlySet<string>)",
                None,
            ),
            (
                s,
                "List[String] { s.toList() }",
                "([...s] as readonly string[])",
                None,
            ),
            (
                ks,
                "Set[Bytes] { s.insert(x) }",
                "__bynkSetInsert<globalThis.Uint8Array>(s, x)",
                Some("__bynkSetInsert"),
            ),
            (
                ks,
                "Set[Bytes] { s.remove(x) }",
                "__bynkSetRemove<globalThis.Uint8Array>(s, x)",
                Some("__bynkSetRemove"),
            ),
            (m, "Int { m.length() }", "m.size", None),
            (m, "List[String] { m.keys() }", "[...m.keys()]", None),
            (m, "List[Int] { m.values() }", "[...m.values()]", None),
            (
                m,
                "Option[Int] { m.get(k) }",
                "((__m: ReadonlyMap<string, number>, __k: string) => __m.has(__k) ? Some(__m.get(__k) as number) : None)(m, k)",
                None,
            ),
            (
                m,
                "Map[String, Int] { m.insert(k, v) }",
                "new globalThis.Map(m).set(k, v)",
                None,
            ),
            (
                km,
                "Map[Bytes, Int] { m.insert(k, v) }",
                "__bynkMapInsert<globalThis.Uint8Array, number>(m, k, v)",
                Some("__bynkMapInsert"),
            ),
        ] {
            let ts = emitted(sig, body);
            assert_eq!(returned(&ts), want, "{body}");
            for helper in ["__bynkSetInsert", "__bynkSetRemove", "__bynkMapInsert"] {
                assert_eq!(
                    ts.contains(&format!("import {{ {helper}"))
                        || ts.contains(&format!(", {helper}")),
                    import == Some(helper),
                    "{body}: {helper} import\n{ts}"
                );
            }
        }
    }

    /// The plain `Set.remove` is the comma-expression arm: a `Sequence` node (1A.35a part B),
    /// which prints today's bytes.
    #[test]
    fn the_comma_expression_arm_is_a_sequence_node() {
        let ts = emitted("(s: Set[String], x: String)", "Set[String] { s.remove(x) }");
        assert_eq!(
            returned(&ts),
            "((__s: globalThis.Set<string>) => (__s.delete(x), __s as ReadonlySet<string>))(new globalThis.Set<string>(s))"
        );
    }

    /// The receiver is lowered before the argument, as before: a `?` in each hoists in that order.
    #[test]
    fn the_receiver_is_lowered_before_the_argument() {
        let ts = emitted(
            "(a: Result[Option[Int], String], b: Result[Int, String])",
            "Result[Int, String] { Ok(a?.getOrElse(b?)) }",
        );
        let first = ts.find("const __r0 = a;").expect("receiver first");
        let second = ts.find("const __r1 = b;").expect("argument second");
        assert!(first < second, "{ts}");
    }

    fn typed_as_int(commons: &mut TypedCommons) -> bynk_check::checker::TyId {
        let int = commons.tys().intern(Ty::Base(BaseType::Int));
        commons.expr_types.insert(
            ExprId(1),
            TypedExpr {
                span: Span::new(0, 0),
                ty: int,
            },
        );
        int
    }

    /// The call under test sits at bytes 7..11; every operand sits at 0..0.
    fn call_span() -> Span {
        Span::new(7, 11)
    }

    fn ident(id: u32) -> Expr {
        let span = if id == 1 {
            call_span()
        } else {
            Span::new(0, 0)
        };
        Expr {
            id: ExprId(id),
            kind: ExprKind::Ident(Ident {
                name: "x".to_string(),
                span,
            }),
            span,
        }
    }

    /// A combinator call whose checked type is not `Option[_]` is checker and emitter drift: a
    /// missing type fact (1A.34), recorded at the call's span, and lowering continues.
    #[test]
    fn an_untyped_option_call_is_a_missing_type_fact() {
        let mut commons = TypedCommons::empty();
        let int = typed_as_int(&mut commons);
        let cross = bynk_check::resolver::CrossContextInfo::default();
        let ru = RuntimeUse::default();
        let mut cx = LowerCtx::new(
            ModuleCtx::new(&commons, &cross, &ru, UnitFacts::detached()),
            BodyMode::Method,
        );
        let (e, recv, f) = (ident(1), ident(2), ident(3));
        let lowered = lower_option_kernel(&e, &recv, OptionOp::Map, &[f], int, &mut cx);
        assert!(lowered.is_some(), "lowering continues past the fault");
        let faults = ru.take_faults();
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].span, call_span());
        assert!(
            faults[0]
                .what
                .contains("`Option[_]` type for an `Option.map` call"),
            "{faults:?}"
        );
    }

    /// The same for a `Result` combinator.
    #[test]
    fn an_untyped_result_call_is_a_missing_type_fact() {
        let mut commons = TypedCommons::empty();
        let int = typed_as_int(&mut commons);
        let cross = bynk_check::resolver::CrossContextInfo::default();
        let ru = RuntimeUse::default();
        let mut cx = LowerCtx::new(
            ModuleCtx::new(&commons, &cross, &ru, UnitFacts::detached()),
            BodyMode::Method,
        );
        let (e, recv, f) = (ident(1), ident(2), ident(3));
        let lowered = lower_result_kernel(&e, &recv, ResultOp::MapErr, &[f], int, int, &mut cx);
        assert!(lowered.is_some(), "lowering continues past the fault");
        let faults = ru.take_faults();
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].span, call_span());
        assert!(
            faults[0]
                .what
                .contains("`Result[_, _]` type for a `Result.mapErr` call"),
            "{faults:?}"
        );
    }
}
