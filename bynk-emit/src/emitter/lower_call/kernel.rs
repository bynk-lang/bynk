//! The kernel-method dispatcher and the arms it owns (back-half paydown 1A.28, #3729,
//! Decision D): `lower_kernel_method_call`, the `Effect` kernels (`within`, `race`, `detach`) and
//! the `EffectResult` combinators. Moved out of `lower.rs` byte for byte, then the inline and
//! `Effect` arms converted. The scalar kernels live in `kernel_scalar.rs` (1A.29); the container
//! kernels stay in `lower.rs`.

use bynk_check::checker::{Callee, Ty, TyId};
use bynk_check::kernel_op::{EffectOp, EffectResultOp, HttpResultOp, KernelOp, TestConnectionOp};
use bynk_ts::{TsArrowBody, TsBinaryOp, TsExpr, TsLit, TsObjectEntry, TsType};

use super::super::lower::{Pre, expect_location};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{any_ty, fn_type, lower_list_kernel, promise, take};
use super::super::ts_types::ts_ty_to_ts_type;
use super::super::*;
use super::kernel_container::{
    lower_map_kernel, lower_option_kernel, lower_result_kernel, lower_set_kernel,
};
use super::kernel_scalar::{
    lower_bytes_kernel, lower_decimal_kernel, lower_duration_kernel, lower_float_kernel,
    lower_instant_kernel, lower_int_kernel, lower_string_kernel,
};
use super::query::{
    QueryShape, QuerySource, lower_query_method, lower_stream_method, query_key_cmp, query_keyed,
    query_other_elem,
};
use crate::ts_build::{
    arrow, binary, call, const_typed as const_, ident as id, member, param, paren, ret, spread,
    str_lit as str_node,
};

/// `within`, `race` and `detach` on an `Effect[T]` (#2683, #2711, #3437). `None`
/// when the call's shape is not one the kernel method lowers (the caller falls
/// through to the instance-call lowering).
fn lower_effect_kernel_call(
    e: &Expr,
    receiver: &Expr,
    op: EffectOp,
    args: &[Expr],
    recv_ty: &Ty,
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    let tys = cx.ty_render();
    match op {
        // #2683: `e.within(d)` races the effect against a deadline
        // and hands the abandoned effect to the handler's execution
        // context.
        EffectOp::Within if args.len() == 1 => {
            let recv = pre.lower_node(receiver, cx);
            let after = pre.lower_node(&args[0], cx);
            let exec = cx.exec_node();
            return Some({
                let n = vec![recv, after, exec];
                call(id("__bynkWithin"), n)
            });
        }
        // #2711: `a.race(b)` takes the first side to succeed and
        // hands the rest to the same execution context, uncancelled.
        EffectOp::Race if args.len() == 1 => {
            // A chain `a.race(b).race(c)` lowers to one
            // `__bynkRace([a, b, c], …)`: nesting would let `c` beat
            // `a` in a same-instant tie, as the inner race settles a
            // microtask later, and ties go to source order.
            let mut operands: Vec<&Expr> = vec![&args[0]];
            let mut head: &Expr = receiver;
            while let ExprKind::MethodCall {
                receiver: inner,
                args: a,
                ..
            } = &head.kind
                && matches!(
                    cx.commons().callee(head.id),
                    Some(Callee::Kernel {
                        op: KernelOp::Effect(EffectOp::Race),
                        ..
                    })
                )
                && a.len() == 1
                && matches!(
                    cx.commons().expr_ty(inner.id).as_deref(),
                    Some(Ty::Effect(_))
                )
            {
                operands.push(&a[0]);
                head = inner;
            }
            operands.push(head);
            operands.reverse();
            let mut children: Vec<bynk_ts::TsExpr> = operands
                .into_iter()
                .map(|o| pre.lower_node(o, cx))
                .collect();
            children.push(cx.exec_node());
            return Some({
                let mut n = children;
                let exec = n.pop().expect("the execution context is the last child");
                call(id("__bynkRace"), vec![TsExpr::array(n), exec])
            });
        }
        // #3437: `e.detach()` hands the already-running effect to
        // the execution context and reports a failure (a fault, or
        // an `Err` when the payload is a `Result`). The static type
        // picks the route: only a `Result` payload names its error
        // type to the helper.
        EffectOp::Detach if args.is_empty() => {
            cx.note_detach();
            let recv = pre.lower_node(receiver, cx);
            // The site is the assertion location in a test body, else the fault location, else
            // the offset. `expect_location` has already escaped the assertion location once;
            // the literal node's own escape is the second.
            let site = if cx.assert_loc().is_some() {
                str_node(expect_location(cx, e.span.start))
            } else {
                str_node(
                    cx.fault_location(e.span.start)
                        .unwrap_or_else(|| format!("offset {}", e.span.start)),
                )
            };
            let err_raw = recv_ty
                .as_effect()
                .and_then(|inner| tys.get(inner).as_result())
                .map(|(_, err)| err.display(tys.types()).to_string());
            let mut children = vec![recv, cx.exec_node(), site];
            if let Some(raw) = &err_raw {
                children.push(str_node(raw.as_str()));
            }
            return Some({
                let n = children;
                call(id("__bynkDetach"), n)
            });
        }
        EffectOp::Within | EffectOp::Race | EffectOp::Detach => {}
    }
    None
}

/// A built-in method call the checker resolved to a typed [`KernelOp`] (#3164):
/// the one place the emitter turns a kernel operation into TypeScript, matching
/// on the receiver kind and the operation. `None` when the operation has no
/// inline lowering here, so the caller falls through to the UFCS lowering; any
/// hoisted statements the attempt produced stay in `pre`. The result is a
/// [`bynk_ts::TsExpr`] (1A.20, Decision G): the `List` kernel returns a node when its
/// operands are nodes (1A.18a), a delegated kernel's `Lowered` passes through
/// `Pre::absorb_built`, and the `Stream` generators are nodes too (1A.35a part B).
pub(in crate::emitter) fn lower_kernel_method_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    let tys = cx.ty_render();
    // v0.20b: built-in kernel methods on the collection types, dispatched on the
    // receiver's checked type. Emitted inline (typed IIFEs / spreads) — no runtime
    // imports, so files that never touch collections emit byte-identically to v0.20a.
    // The checker resolved this call to a typed `KernelOp` (#3164), so each arm
    // matches on the resolved operation, not on the method's spelling, and the
    // receiver's type arguments are read off its checked type.
    if let Some(Callee::Kernel {
        op: kop,
        recv: kernel_recv,
    }) = cx.commons().callee(e.id).cloned()
    {
        let recv_ty = (*tys.get(kernel_recv)).clone();
        // The receiver's checked type disagrees with the resolved op only if
        // the checker and the emitter have drifted: a missing fact (1A.34),
        // reported at the call, with no text fallback.
        let site = cx.fault_site(e.span);
        let mismatch = move |kop: KernelOp| -> Option<bynk_ts::TsExpr> {
            Some(site.missing_fact(format_args!(
                "receiver type matching the resolved kernel op {kop:?}"
            )))
        };
        match kop {
            KernelOp::List(op) => {
                let Ty::List(elem) = &recv_ty else {
                    return mismatch(kop);
                };
                if let Some(built) = lower_list_kernel(e, receiver, op, args, *elem, cx, pre) {
                    return Some(built);
                }
            }
            // v0.91 (ADR 0119): a chained op on a lazy `Query` — the source is
            // the receiver thunk, invoked (`(recv)()`).
            KernelOp::Query(op) => {
                let Ty::Query(elem) = &recv_ty else {
                    return mismatch(kop);
                };
                let elem_node = ts_ty_to_ts_type(*elem, tys);
                let recv = pre.lower_node(receiver, cx);
                let a: Vec<bynk_ts::TsExpr> = args.iter().map(|x| pre.lower_node(x, cx)).collect();
                let result_ty = cx.commons().expr_types.get(&e.id).map(|te| te.ty);
                let other_elem = query_other_elem(args, cx);
                if let Some(built) = lower_query_method(
                    QuerySource::Thunk(recv),
                    op,
                    a,
                    result_ty,
                    &QueryShape {
                        elem: &elem_node,
                        other_elem: &other_elem,
                        keyed: query_keyed(e, method, args, cx),
                        key_cmp: query_key_cmp(method, args, cx),
                        runtime_use: cx.runtime_use(),
                        span: e.span,
                    },
                    tys,
                ) {
                    return Some(built);
                }
            }
            // v0.100: a chained op on a `Stream` — the receiver already *is* an
            // async iterable, so it is the source directly. Emitted inline as
            // async-generator IIFEs (builders) / an async drain (`collect`).
            KernelOp::Stream(op) => {
                let recv = pre.lower_node(receiver, cx);
                let a: Vec<bynk_ts::TsExpr> = args.iter().map(|x| pre.lower_node(x, cx)).collect();
                if let Some(built) = lower_stream_method(recv, op, a) {
                    return Some(built);
                }
            }
            // #3051: a case's recording `TestConnection[F]`.
            KernelOp::TestConnection(op) => {
                let recv = pre.lower_node(receiver, cx);
                return Some(lower_test_connection(op, recv));
            }
            // v0.102: the held-resource operations on a `Connection[F]` lower to
            // method calls on the runtime `Connection` object — `send(frame)` and
            // `close()`. The linearity pass has already verified ownership.
            KernelOp::Connection(op) => {
                return Some(lower_receiver_call(op.name(), receiver, args, cx, pre));
            }
            KernelOp::Map(op) => {
                let Ty::Map(key, val) = &recv_ty else {
                    return mismatch(kop);
                };
                if let Some(built) =
                    pre.absorb_built(lower_map_kernel(receiver, op, args, *key, *val, cx))
                {
                    return Some(built);
                }
            }
            // K6a (#2596): the `Set` kernel.
            KernelOp::Set(op) => {
                let Ty::Set(elem) = &recv_ty else {
                    return mismatch(kop);
                };
                if let Some(built) =
                    pre.absorb_built(lower_set_kernel(receiver, op, args, *elem, cx))
                {
                    return Some(built);
                }
            }
            // v0.21: the numeric kernel. `toFloat` is the identity
            // at runtime (the Int/Float distinction is erased);
            // the four `Float -> Int` roundings map onto `Math.*`.
            // v0.22a extends it (abs/min/max/clamp, isNaN/isFinite).
            KernelOp::Int(op) => {
                if let Some(built) = pre.absorb_built(lower_int_kernel(receiver, op, args, cx)) {
                    return Some(built);
                }
            }
            KernelOp::Float(op) => {
                if let Some(built) = pre.absorb_built(lower_float_kernel(receiver, op, args, cx)) {
                    return Some(built);
                }
            }
            // v0.86 (ADR 0112): the `Duration` kernel. `toMillis` is the identity
            // at runtime (a `Duration` already *is* its milliseconds); `toString`
            // renders the number.
            KernelOp::Duration(op) => {
                if let Some(built) = pre.absorb_built(lower_duration_kernel(receiver, op, args, cx))
                {
                    return Some(built);
                }
            }
            // v0.90 (ADR 0114): the `Instant` kernel. `toEpochMillis` is the
            // identity (an `Instant` lowers to its epoch millis); `toString`
            // renders the number.
            KernelOp::Instant(op) => {
                if let Some(built) = pre.absorb_built(lower_instant_kernel(receiver, op, args, cx))
                {
                    return Some(built);
                }
            }
            // #3277: a cookie rides beside an `HttpResult` as a `cookies` list the
            // router turns into one `Set-Cookie` each. `clearCookie` is the cookie
            // sent again, empty and already expired, under the same name, `Domain`
            // and `Path`, which is what removes it.
            KernelOp::HttpResult(op) => {
                cx.note_http_result_cookie();
                return Some(lower_http_result(op, receiver, args, cx, pre));
            }
            // #2341 (decision A): an HTTP handler's `headers` is a runtime
            // object with these very methods, each answering an `Option`.
            KernelOp::RequestHeaders(_)
            | KernelOp::RequestQuery(_)
            | KernelOp::RequestCookies(_)
            | KernelOp::TokenClaims(_) => {
                return Some(lower_receiver_call(kop.name(), receiver, args, cx, pre));
            }
            // v0.110 (ADR 0142): the `Bytes` kernel. `length` is the octet
            // count; `toBase64` encodes; `decodeUtf8` is a guarded UTF-8 decode
            // returning `Option`.
            KernelOp::Bytes(op) => {
                if let Some(built) = pre.absorb_built(lower_bytes_kernel(receiver, op, args, cx)) {
                    return Some(built);
                }
            }
            // #2622: the `Decimal` kernel.
            KernelOp::Decimal(op) => {
                if let Some(built) = pre.absorb_built(lower_decimal_kernel(receiver, op, args, cx))
                {
                    return Some(built);
                }
            }
            // v0.22a: the string kernel (ADR 0046).
            KernelOp::String(op) => {
                if let Some(built) = pre.absorb_built(lower_string_kernel(receiver, op, args, cx)) {
                    return Some(built);
                }
            }
            // v0.22a: Option/Result combinators (ADR 0048).
            KernelOp::Option(op) => {
                let Ty::Option(inner) = &recv_ty else {
                    return mismatch(kop);
                };
                if let Some(built) =
                    pre.absorb_built(lower_option_kernel(e, receiver, op, args, *inner, cx))
                {
                    return Some(built);
                }
            }
            KernelOp::Result(op) => {
                let Ty::Result(ok, err) = &recv_ty else {
                    return mismatch(kop);
                };
                if let Some(built) =
                    pre.absorb_built(lower_result_kernel(e, receiver, op, args, *ok, *err, cx))
                {
                    return Some(built);
                }
            }
            // #2683 / #2711 / #3437: `within`, `race` and `detach`, on every `Effect[T]`.
            KernelOp::Effect(op) => {
                if let Some(built) =
                    lower_effect_kernel_call(e, receiver, op, args, &recv_ty, cx, pre)
                {
                    return Some(built);
                }
            }
            // §2.8.3: the `Effect[Result[T, E]]` combinators — `mapOk`/`mapErr`/
            // `flatMapOk`/`flatMapErr`. Lowered as an `async` IIFE that awaits the
            // receiver `Promise<Result<…>>` and rebuilds the transformed Result.
            KernelOp::EffectResult(op) => {
                let Ty::Effect(inner) = &recv_ty else {
                    return mismatch(kop);
                };
                if let Ty::Result(ok, err) = &*tys.get(*inner)
                    && let Some(built) =
                        lower_effect_result_kernel(e, receiver, op, args, *ok, *err, cx, pre)
                {
                    return Some(built);
                }
            }
        }
    }
    None
}

/// §2.8.3: lower an `Effect[Result[T, E]]` combinator. `Effect[T]` is
/// `Promise<T>` at runtime, so each method is an `async` IIFE that `await`s the
/// receiver Promise and rebuilds the transformed `Result` — the exact
/// desugaring the spec gives (`mapOk` ≡ `map(r => r.map(f))`, etc.), inlined so
/// no runtime helper is needed. `ok`/`err` are the receiver's peeled `Result`
/// parameters; the call's checked type supplies the transformed parameter for
/// the TS annotation.
#[allow(clippy::too_many_arguments)]
fn lower_effect_result_kernel(
    e: &Expr,
    receiver: &Expr,
    op: EffectResultOp,
    args: &[Expr],
    ok: TyId,
    err: TyId,
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    let tys = cx.ty_render();
    let t = ts_ty_to_ts_type(ok, tys);
    let et = ts_ty_to_ts_type(err, tys);
    // The call's checked type is `Effect[Result[a, b]]`; peel `(a, b)` for the
    // transformed parameter's TS type.
    let peeled = cx
        .commons()
        .expr_ty(e.id)
        .as_deref()
        .and_then(Ty::as_effect)
        .and_then(|inner| tys.get(inner).as_result());
    let (a_ts, b_ts) = match peeled {
        Some((a, b)) => (ts_ty_to_ts_type(a, tys), ts_ty_to_ts_type(b, tys)),
        // A missing fact (1A.34); the placeholder keeps lowering going.
        None => {
            let missing = cx.missing_fact_ty(
                e.span,
                "`Effect[Result[_, _]]` type for an effectful `Result` combinator call",
            );
            (missing.clone(), missing)
        }
    };
    let [f] = args else {
        return None;
    };
    // `__e: Promise<Result<T, E>>`, then `__f: <the callback's type>`; the body
    // awaits the receiver and rebuilds the `Result`.
    let result_of = |ok: &TsType, err: &TsType| {
        TsType::named_with_args("Result", vec![ok.clone(), err.clone()])
    };
    let effect_of = |ok: &TsType, err: &TsType| promise(result_of(ok, err));
    let cb = |param: &str, from: &TsType, to: TsType| fn_type(&[(param, from)], to);
    // (callback type, the tag tested, the field read, whether the callback is
    // awaited, the constructor wrapping an unawaited result).
    let (cb_ty, tag, field, awaited, wrap) = match op {
        // result `Effect[Result[U, E]]` — `a_ts` is `U`.
        EffectResultOp::MapOk => (cb("x", &t, a_ts), "Ok", "value", false, Some("Ok")),
        // result `Effect[Result[T, F]]` — `b_ts` is `F`.
        EffectResultOp::MapErr => (cb("e", &et, b_ts), "Err", "error", false, Some("Err")),
        // `f: T -> Effect[Result[U, E]]`; result `Effect[Result[U, E]]`.
        EffectResultOp::FlatMapOk => (
            cb("x", &t, effect_of(&a_ts, &et)),
            "Ok",
            "value",
            true,
            None,
        ),
        // `f: E -> Effect[Result[T, F]]`; result `Effect[Result[T, F]]`.
        EffectResultOp::FlatMapErr => (
            cb("e", &et, effect_of(&t, &b_ts)),
            "Err",
            "error",
            true,
            None,
        ),
    };
    let recv = pre.lower_node(receiver, cx);
    let f = pre.lower_node(f, cx);
    let params = vec![
        param("__e", Some(effect_of(&t, &et))),
        param("__f", Some(cb_ty)),
    ];
    let applied = call(id("__f"), vec![member(id("__r"), field)]);
    let hit = match (awaited, wrap) {
        (true, _) => TsExpr::Await(Box::new(applied)),
        (false, Some(ctor)) => call(id(ctor), vec![applied]),
        (false, None) => applied,
    };
    let body = vec![
        const_("__r", None, TsExpr::Await(Box::new(id("__e")))),
        ret(TsExpr::Conditional {
            test: Box::new(binary(
                TsBinaryOp::StrictEq,
                member(id("__r"), "tag"),
                TsExpr::Lit(TsLit::Str(tag.to_string())),
            )),
            consequent: Box::new(hit),
            alternate: Box::new(id("__r")),
        }),
    ];
    let async_arrow = TsExpr::Arrow {
        params,
        is_async: true,
        generics: vec![],
        return_type: None,
        body: Box::new(TsArrowBody::Block(body)),
    };
    Some({
        let n = vec![recv, f];
        call(paren(async_arrow), n)
    })
}

/// A receiver as the object of a member access. The printer brackets a binary, a cast, an arrow, a
/// conditional or an assignment there, but not an `await`, a unary operator or a number literal
/// (`Num` or `Raw`), which would then read as `await f().op()` or `1.op()`: those keep the brackets
/// the text form always wrote (1A.28, #3729, Decision B). A `Spread` is bracketed too, defensively:
/// it is legal only as an array item, so a receiver is never one today. Shared with the scalar
/// kernels (1A.29).
pub(super) fn receiver_object(recv: TsExpr) -> TsExpr {
    if recv.needs_receiver_brackets() {
        paren(recv)
    } else {
        recv
    }
}

/// `recv.method(args)` for a runtime object that has the method (`Connection.send`, the request
/// views): the receiver is bracketed only where precedence needs it.
fn lower_receiver_call(
    method: &str,
    receiver: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> bynk_ts::TsExpr {
    let mut children = vec![pre.lower_node(receiver, cx)];
    children.extend(args.iter().map(|a| pre.lower_node(a, cx)));
    {
        let mut n = children;
        let recv = receiver_object(n.remove(0));
        call(member(recv, method), n)
    }
}

/// A case's recording `TestConnection[F]`: `[...recv.sent]`, `recv.closed`, `recv.mint()`.
fn lower_test_connection(op: TestConnectionOp, recv: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    let n = vec![recv];
    let [recv] = take::<1, _>(n);
    let recv = receiver_object(recv);
    match op {
        TestConnectionOp::Sent => TsExpr::array(vec![spread(member(recv, "sent"))]),
        TestConnectionOp::Closed => member(recv, "closed"),
        TestConnectionOp::Connection => call(member(recv, "mint"), vec![]),
    }
}

/// `withCookie`, `clearCookie` and `cookies` on an `HttpResult` (#3277).
fn lower_http_result(
    op: HttpResultOp,
    receiver: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> bynk_ts::TsExpr {
    let recv = pre.lower_node(receiver, cx);
    let lowered: Vec<bynk_ts::TsExpr> = args.iter().map(|a| pre.lower_node(a, cx)).collect();
    if matches!(op, HttpResultOp::Cookies) {
        // The `cookies` list of the receiver, read through a cast, or empty.
        return {
            let n = vec![recv];
            let [recv] = take::<1, _>(n);
            let ty = any_ty();
            let cast = TsExpr::As {
                expr: Box::new(recv),
                ty,
            };
            paren(binary(
                TsBinaryOp::NullishCoalescing,
                member(paren(cast), "cookies"),
                TsExpr::array(vec![]),
            ))
        };
    }
    // The cookie argument; absent only for a program that failed checking.
    let arg = lowered.into_iter().next().unwrap_or_else(|| {
        cx.missing_fact(receiver.span, "cookie argument of an `HttpResult` call")
    });
    let clear = matches!(op, HttpResultOp::ClearCookie);
    {
        let n = vec![arg, recv];
        let [arg, recv] = take::<2, _>(n);
        // `[...(__r.cookies ?? []), <added>]`
        let held = spread(paren(binary(
            TsBinaryOp::NullishCoalescing,
            member(id("__r"), "cookies"),
            TsExpr::array(vec![]),
        )));
        let added = if clear {
            TsExpr::object_entries(vec![
                TsObjectEntry::Spread(paren(arg)),
                TsObjectEntry::prop("value".to_string(), str_node("")),
                TsObjectEntry::prop(
                    "maxAge".to_string(),
                    call(id("Some"), vec![TsExpr::Lit(TsLit::Num("0".to_string()))]),
                ),
                TsObjectEntry::prop(
                    "expires".to_string(),
                    call(id("Some"), vec![TsExpr::Lit(TsLit::Num("0".to_string()))]),
                ),
            ])
        } else {
            paren(arg)
        };
        let record = TsExpr::object_entries(vec![
            TsObjectEntry::Spread(id("__r")),
            TsObjectEntry::prop("cookies".to_string(), TsExpr::array(vec![held, added])),
        ]);
        call(
            paren(arrow(vec![param("__r", Some(any_ty()))], paren(record))),
            vec![recv],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emitter::ctx::{BodyMode, ModuleCtx, UnitFacts};
    use crate::emitter::runtime_use::RuntimeUse;
    use crate::ts_build::ident as ident_node;
    use bynk_check::checker::{ExprId, TypedCommons, TypedExpr};
    use bynk_syntax::span::Span;

    fn printed(e: TsExpr) -> String {
        bynk_ts::print_expr(&e)
    }

    fn node(s: &str) -> bynk_ts::TsExpr {
        ident_node(s)
    }

    /// A receiver is bracketed only where precedence needs it: a plain identifier or call is not, a
    /// binary, a conditional, an `await`, a unary operator or a number literal is.
    #[test]
    fn a_receiver_is_bracketed_only_where_precedence_needs_it() {
        let a = || ident_node("a");
        let b = || ident_node("b");
        let sum = TsExpr::Binary {
            op: TsBinaryOp::Add,
            left: Box::new(a()),
            right: Box::new(b()),
        };
        let awaited = TsExpr::Await(Box::new(call(id("f"), vec![])));
        let cond = TsExpr::Conditional {
            test: Box::new(id("c")),
            consequent: Box::new(id("x")),
            alternate: Box::new(id("y")),
        };
        let not = TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(id("z")),
        };
        let on = |recv: TsExpr| printed(call(member(receiver_object(recv), "op"), vec![]));
        assert_eq!(on(a()), "a.op()");
        assert_eq!(on(call(id("f"), vec![])), "f().op()");
        assert_eq!(on(member(id("deps"), "claims")), "deps.claims.op()");
        assert_eq!(on(sum), "(a + b).op()");
        assert_eq!(on(awaited), "(await f()).op()");
        assert_eq!(on(cond), "(c ? x : y).op()");
        assert_eq!(on(not), "(!z).op()");
        assert_eq!(on(TsExpr::Lit(TsLit::Num("1".to_string()))), "(1).op()");
    }

    /// The `TestConnection` arms: the receiver is bracketed only where precedence needs it.
    #[test]
    fn the_test_connection_arms_print_their_forms() {
        for (op, want) in [
            (TestConnectionOp::Sent, "[...c.sent]"),
            (TestConnectionOp::Closed, "c.closed"),
            (TestConnectionOp::Connection, "c.mint()"),
        ] {
            assert_eq!(printed(lower_test_connection(op, node("c"))), want);
        }
        // A receiver that needs brackets keeps them as a node.
        let sum = TsExpr::Binary {
            op: TsBinaryOp::Add,
            left: Box::new(id("a")),
            right: Box::new(id("b")),
        };
        assert_eq!(
            printed(lower_test_connection(TestConnectionOp::Closed, sum)),
            "(a + b).closed"
        );
    }

    /// `detach`'s site is a `Str` node, and the printer's escape is `escape_ts_string`: the
    /// single escape (a fault location) and the doubly escaped assertion location (the node
    /// over an already escaped string) both print the bytes the text path used to write.
    #[test]
    fn the_site_string_prints_the_escapes_the_text_path_wrote() {
        use crate::emitter::escape_ts_string;
        for raw in [
            "src/shapes.bynk:47:20",
            "tests/a.test.bynk:3:7",
            "offset 120",
            "src/caf\u{e9}/\u{4e2d}\u{6587}.bynk:1:1",
            "dir with space/x.bynk:2:3",
            "a\"b.bynk:1:1",
            "a\\b.bynk:1:1",
            "tab\there.bynk:1:1",
            "line\u{2028}sep.bynk:1:1",
        ] {
            let once = escape_ts_string(raw);
            assert_eq!(printed(str_node(raw)), format!("\"{once}\""));
            let twice = escape_ts_string(&once);
            assert_eq!(printed(str_node(once)), format!("\"{twice}\""), "{raw}");
        }
    }

    fn expr(id: u32) -> Expr {
        Expr {
            id: ExprId(id),
            kind: ExprKind::Ident(Ident {
                name: "x".to_string(),
                span: Span::new(0, 0),
            }),
            span: Span::new(0, 0),
        }
    }

    /// A kernel op on a receiver whose checked type is not the op's own receiver kind is checker and
    /// emitter drift (#3164): the dispatcher records a missing fact at the call (1A.34), with no
    /// fallback, and lowering continues.
    #[test]
    fn a_mismatched_receiver_type_is_a_missing_fact() {
        let mut commons = TypedCommons::empty();
        let int = commons.tys().intern(Ty::Base(BaseType::Int));
        commons.callees.insert(
            ExprId(1),
            Callee::Kernel {
                op: KernelOp::EffectResult(EffectResultOp::MapOk),
                recv: int,
            },
        );
        let cross = bynk_check::resolver::CrossContextInfo::default();
        let ru = RuntimeUse::default();
        let mut cx = LowerCtx::new(
            ModuleCtx::new(&commons, &cross, &ru, UnitFacts::detached()),
            BodyMode::Method,
        );
        let method = Ident {
            name: "mapOk".to_string(),
            span: Span::new(0, 0),
        };
        let (e, recv) = (expr(1), expr(2));
        let built = lower_kernel_method_call(&e, &recv, &method, &[], &mut cx, &mut Pre::new());
        assert!(built.is_some(), "lowering continues past the fault");
        let faults = ru.take_faults();
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].span, e.span);
        assert!(faults[0].what.contains("EffectResult(MapOk)"), "{faults:?}");
    }

    /// An `Effect[Result]` combinator call the checker recorded no `Effect[Result[_, _]]` type for
    /// is a missing type fact (1A.34), and lowering continues.
    #[test]
    fn an_untyped_effect_result_call_is_a_missing_type_fact() {
        let mut commons = TypedCommons::empty();
        let int = commons.tys().intern(Ty::Base(BaseType::Int));
        let cross = bynk_check::resolver::CrossContextInfo::default();
        let ru = RuntimeUse::default();
        // The call is typed `Int`, not `Effect[Result[_, _]]`.
        commons.expr_types.insert(
            ExprId(1),
            TypedExpr {
                span: Span::new(0, 0),
                ty: int,
            },
        );
        let mut cx = LowerCtx::new(
            ModuleCtx::new(&commons, &cross, &ru, UnitFacts::detached()),
            BodyMode::Method,
        );
        let (e, recv) = (expr(1), expr(2));
        let _ = lower_effect_result_kernel(
            &e,
            &recv,
            EffectResultOp::MapOk,
            &[],
            int,
            int,
            &mut cx,
            &mut Pre::new(),
        );
        let faults = ru.take_faults();
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].span, e.span);
        assert!(
            faults[0].what.contains("`Effect[Result[_, _]]` type"),
            "{faults:?}"
        );
    }
}
