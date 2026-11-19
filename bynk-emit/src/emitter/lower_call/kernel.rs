//! The kernel-method dispatcher and the arms it owns (back-half paydown 1A.28, #3729,
//! Decision D): `lower_kernel_method_call`, the `Effect` kernels (`within`, `race`, `detach`) and
//! the `EffectResult` combinators. Moved out of `lower.rs` byte for byte, then the inline and
//! `Effect` arms converted. The sibling kernels (scalar, container) stay in `lower.rs`.

use bynk_check::checker::{Callee, Ty, TyId};
use bynk_check::kernel_op::{EffectOp, EffectResultOp, KernelOp, TestConnectionOp};

use super::super::lower::*;
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::lower_list_kernel;
use super::super::ts_types::{ts_ty, ts_ty_to_ts_type};
use super::super::*;
use super::query::{
    QueryShape, QuerySource, lower_query_method, lower_stream_method, note_par_fan_out,
    query_key_cmp, query_keyed, query_other_elem,
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
) -> Option<String> {
    let tys = cx.ty_render();
    match op {
        // #2683: `e.within(d)` races the effect against a deadline
        // and hands the abandoned effect to the handler's execution
        // context.
        EffectOp::Within if args.len() == 1 => {
            cx.note_within();
            let recv = pre.lower(receiver, cx);
            let after = pre.lower(&args[0], cx);
            return Some(format!("__bynkWithin({recv}, {after}, {})", cx.exec_expr()));
        }
        // #2711: `a.race(b)` takes the first side to succeed and
        // hands the rest to the same execution context, uncancelled.
        EffectOp::Race if args.len() == 1 => {
            cx.note_race();
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
            let lowered: Vec<String> = operands.into_iter().map(|o| pre.lower(o, cx)).collect();
            return Some(format!(
                "__bynkRace([{}], {})",
                lowered.join(", "),
                cx.exec_expr()
            ));
        }
        // #3437: `e.detach()` hands the already-running effect to
        // the execution context and reports a failure (a fault, or
        // an `Err` when the payload is a `Result`). The static type
        // picks the route: only a `Result` payload names its error
        // type to the helper.
        EffectOp::Detach if args.is_empty() => {
            cx.note_detach();
            let recv = pre.lower(receiver, cx);
            let site = cx
                .assert_loc()
                .map(|_| expect_location(cx, e.span.start))
                .or_else(|| cx.fault_location(e.span.start))
                .map(|l| crate::emitter::escape_ts_string(&l))
                .unwrap_or_else(|| format!("offset {}", e.span.start));
            let err_type = match &recv_ty {
                Ty::Effect(inner) => match &*tys.get(*inner) {
                    Ty::Result(_, err) => Some(crate::emitter::escape_ts_string(
                        &err.display(tys.types()).to_string(),
                    )),
                    _ => None,
                },
                _ => None,
            };
            let tail = err_type.map(|t| format!(", \"{t}\"")).unwrap_or_default();
            return Some(format!(
                "__bynkDetach({recv}, {}, \"{site}\"{tail})",
                cx.exec_expr()
            ));
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
/// [`Built`] (1A.20, Decision G): the `List` kernel returns a node when its
/// operands are nodes (1A.18a), a delegated kernel's `Lowered` passes through
/// `Pre::absorb_built` unprinted, and a family that is still text is `Err`
/// (`// 1A:` at its arm below).
pub(in crate::emitter) fn lower_kernel_method_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<Built> {
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
        // the checker and the emitter have drifted.
        let mismatch = |kop: KernelOp| -> ! {
            panic!("bynk internal error (#3164): kernel op {kop:?} on a mismatched receiver type")
        };
        // 1A: every arm below but `List` is text until its own family slice (the
        // track's `1A.query`, `1A.stream`, `1A.map`, `1A.set`, `1A.option`,
        // `1A.result`, `1A.string` and `1A.numeric` rows).
        match kop {
            KernelOp::List(op) => {
                let Ty::List(elem) = &recv_ty else {
                    mismatch(kop)
                };
                if let Some(built) = lower_list_kernel(e, receiver, op, args, *elem, cx, pre) {
                    return Some(built);
                }
            }
            // v0.91 (ADR 0119): a chained op on a lazy `Query` — the source is
            // the receiver thunk, invoked (`(recv)()`).
            KernelOp::Query(op) => {
                let Ty::Query(elem) = &recv_ty else {
                    mismatch(kop)
                };
                let elem_node = ts_ty_to_ts_type(*elem, tys);
                let recv = pre.lower_node(receiver, cx);
                let a: Vec<LoweredExpr> = args.iter().map(|x| pre.lower_node(x, cx)).collect();
                let result_ty = cx.commons().expr_types.get(&e.id).map(|te| te.ty);
                note_par_fan_out(cx, method);
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
                let recv = pre.lower(receiver, cx);
                let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
                if let Some(s) = lower_stream_method(recv, op, &a) {
                    return Some(Err(s));
                }
            }
            // #3051: a case's recording `TestConnection[F]`.
            KernelOp::TestConnection(op) => {
                let recv = pre.lower(receiver, cx);
                let s = match op {
                    TestConnectionOp::Sent => format!("[...({recv}).sent]"),
                    TestConnectionOp::Closed => format!("({recv}).closed"),
                    TestConnectionOp::Connection => format!("({recv}).mint()"),
                };
                return Some(Err(s));
            }
            // v0.102: the held-resource operations on a `Connection[F]` lower to
            // method calls on the runtime `Connection` object — `send(frame)` and
            // `close()`. The linearity pass has already verified ownership.
            KernelOp::Connection(op) => {
                let recv = pre.lower(receiver, cx);
                let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
                return Some(Err(format!("({recv}).{}({})", op.name(), a.join(", "))));
            }
            KernelOp::Map(op) => {
                let Ty::Map(key, val) = &recv_ty else {
                    mismatch(kop)
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
                    mismatch(kop)
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
                let recv = pre.lower(receiver, cx);
                let args: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
                let arg = args.first().cloned().unwrap_or_default();
                let expr = match op {
                    bynk_check::kernel_op::HttpResultOp::WithCookie => format!(
                        "((__r: any) => ({{ ...__r, cookies: [...(__r.cookies ?? []), ({arg})] }}))({recv})"
                    ),
                    bynk_check::kernel_op::HttpResultOp::ClearCookie => format!(
                        "((__r: any) => ({{ ...__r, cookies: [...(__r.cookies ?? []), {{ ...({arg}), value: \"\", maxAge: Some(0), expires: Some(0) }}] }}))({recv})"
                    ),
                    bynk_check::kernel_op::HttpResultOp::Cookies => {
                        format!("((({recv}) as any).cookies ?? [])")
                    }
                };
                return Some(Err(expr));
            }
            // #2341 (decision A): an HTTP handler's `headers` is a runtime
            // object with these very methods, each answering an `Option`.
            KernelOp::RequestHeaders(_)
            | KernelOp::RequestQuery(_)
            | KernelOp::RequestCookies(_)
            | KernelOp::TokenClaims(_) => {
                let recv = pre.lower(receiver, cx);
                let args: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
                return Some(Err(format!("({recv}).{}({})", kop.name(), args.join(", "))));
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
                    mismatch(kop)
                };
                if let Some(built) =
                    pre.absorb_built(lower_option_kernel(e, receiver, op, args, *inner, cx))
                {
                    return Some(built);
                }
            }
            KernelOp::Result(op) => {
                let Ty::Result(ok, err) = &recv_ty else {
                    mismatch(kop)
                };
                if let Some(built) =
                    pre.absorb_built(lower_result_kernel(e, receiver, op, args, *ok, *err, cx))
                {
                    return Some(built);
                }
            }
            // #2683 / #2711 / #3437: `within`, `race` and `detach`, on every `Effect[T]`.
            KernelOp::Effect(op) => {
                if let Some(s) = lower_effect_kernel_call(e, receiver, op, args, &recv_ty, cx, pre)
                {
                    return Some(Err(s));
                }
            }
            // §2.8.3: the `Effect[Result[T, E]]` combinators — `mapOk`/`mapErr`/
            // `flatMapOk`/`flatMapErr`. Lowered as an `async` IIFE that awaits the
            // receiver `Promise<Result<…>>` and rebuilds the transformed Result.
            KernelOp::EffectResult(op) => {
                let Ty::Effect(inner) = &recv_ty else {
                    mismatch(kop)
                };
                if let Ty::Result(ok, err) = &*tys.get(*inner)
                    && let Some(built) = pre.absorb_built(lower_effect_result_kernel(
                        e, receiver, op, args, *ok, *err, cx,
                    ))
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
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let t = ts_ty(ok, tys);
    let et = ts_ty(err, tys);
    // The call's checked type is `Effect[Result[a, b]]`; peel `(a, b)` for the
    // transformed parameter's TS type.
    let (a_ts, b_ts) = match cx.commons().expr_ty(e.id).as_deref() {
        Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
            Ty::Result(a, b) => (ts_ty(*a, tys), ts_ty(*b, tys)),
            other => panic!(
                "bynk internal error (finding #28): emitter expected `Effect[Result[_, _]]` \
                 recorded for an effectful `Result` combinator call at {:?}, but found \
                 `Effect[{other:?}]` — the checker should already have typed it",
                e.span
            ),
        },
        other => panic!(
            "bynk internal error (finding #28): emitter expected `Effect[Result[_, _]]` \
             recorded for an effectful `Result` combinator call at {:?}, but found {other:?} \
             — the checker should already have typed it",
            e.span
        ),
    };
    let text: Option<String> = match (op, args) {
        (EffectResultOp::MapOk, [f]) => {
            // result `Effect[Result[U, E]]` — `a_ts` is `U`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (x: {t}) => {a_ts}) => {{ const __r = await __e; return __r.tag === \"Ok\" ? Ok(__f(__r.value)) : __r; }})({recv}, {f})"
            ))
        }
        (EffectResultOp::MapErr, [f]) => {
            // result `Effect[Result[T, F]]` — `b_ts` is `F`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (e: {et}) => {b_ts}) => {{ const __r = await __e; return __r.tag === \"Err\" ? Err(__f(__r.error)) : __r; }})({recv}, {f})"
            ))
        }
        (EffectResultOp::FlatMapOk, [f]) => {
            // `f: T -> Effect[Result[U, E]]`; result `Effect[Result[U, E]]`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (x: {t}) => globalThis.Promise<Result<{a_ts}, {et}>>) => {{ const __r = await __e; return __r.tag === \"Ok\" ? await __f(__r.value) : __r; }})({recv}, {f})"
            ))
        }
        (EffectResultOp::FlatMapErr, [f]) => {
            // `f: E -> Effect[Result[T, F]]`; result `Effect[Result[T, F]]`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (e: {et}) => globalThis.Promise<Result<{t}, {b_ts}>>) => {{ const __r = await __e; return __r.tag === \"Err\" ? await __f(__r.error) : __r; }})({recv}, {f})"
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}
