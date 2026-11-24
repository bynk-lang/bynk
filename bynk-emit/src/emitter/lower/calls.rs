//! The call dispatchers (back-half paydown 1A.6, 1A.20, 2.2e, #4087): the method-call dispatcher,
//! UFCS and cross-context calls, function calls and the `settle` helpers. Moved out of `lower.rs`
//! byte for byte.

use std::sync::Arc;

use bynk_check::actors::CallSitePrincipal;
use bynk_check::checker::{Callee, Ty, TyId, Types};
use bynk_check::kernel_op::{IntrinsicNs, StaticOp};
use bynk_syntax::ast::{Expr, ExprKind, Ident, TypeBody, TypeRef};

use super::super::internal_fault::unnameable_receiver_type_arg;
use super::super::lower_call::agents::{
    lower_agent_init, lower_inline_agent_call, lower_let_bound_agent_call,
};
use super::super::lower_call::cross_context::{
    cross_context_lowering_prefix, lower_cross_context_service_call,
};
use super::super::lower_call::kernel::lower_kernel_method_call;
use super::super::lower_call::platform::{
    lower_capability_call, lower_cookie_field_call, lower_cross_capability_call,
    lower_d1_peek_call, lower_d1_select_call, lower_events_emit_call, lower_signed_cookie_call,
};
use super::super::lower_call::statics::{
    lower_runtime_value_static_call, lower_scalar_static_call, lower_type_static_call,
};
use super::super::lower_call::store::lower_store_method_call;
use super::super::lower_call::test_calls::{
    lower_system_http_call, lower_system_schedule_call, lower_test_asset_fetch_call,
    lower_test_service_call,
};
use super::super::lower_ctx::LowerCtx;
use super::super::ts_types::ts_type_ref_to_ts_type;
use super::super::{ts_ident, ty_to_type_ref};
use super::{Lowered, Pre, expect_location};
use crate::ts_build::{call, ident, str_lit};

/// The parameter names of a capability operation, looked up from the capability
/// declarations in scope (v0.117). Used to destructure a recorded call's
/// arguments for a `with` predicate and to build `trace(Cap.op)` records.
/// P6.29 (design/tracks/the-ir.md §6a): the declaration lookup itself now reads
/// `bynk_model::capability_op_sig_from_commons` — still a by-name walk
/// under the hood (nothing IR-native replaces "find the op named `op` on the
/// capability named `cap`"), but relocated to `ir/lower.rs` (excluded from
/// `ast_importers` — the `Ast → Ir` lowering pass's own job), so this file no
/// longer spells `bynk_syntax::ast::CommonsItem` itself.
pub(super) fn cap_op_param_names(cx: &LowerCtx, cap: &str, op: &str) -> Vec<String> {
    bynk_model::capability_op_sig_from_commons(cx.commons(), cap, op)
        .map(|sig| sig.params.into_iter().map(|(name, _)| name).collect())
        .unwrap_or_default()
}

/// 1A.33: what the checker recorded for a call-site principal clause (`by Nobody`, a reserved
/// bad credential driven by `BadCredential::driver`, or an actor).
///
/// Every checker path that accepts a `by` clause records it. If one ever does
/// not, fall back to the same single classification the checker uses
/// (`CallSitePrincipal::of`) rather than treating `Nobody` or `Forged` as an
/// ordinary actor of that name, and flag the gap in debug builds.
pub(in crate::emitter) fn call_site_principal(
    cx: &LowerCtx,
    principal: &bynk_syntax::ast::CallSiteActor,
) -> CallSitePrincipal {
    match cx.commons().callee(principal.id) {
        Some(Callee::Principal(p)) => p.clone(),
        _ => {
            debug_assert!(
                false,
                "unrecorded call-site principal `{}`",
                principal.actor.name
            );
            CallSitePrincipal::of(&principal.actor.name)
        }
    }
}

/// Lower an `ExprKind::MethodCall`. This is a dispatcher: a sequence of
/// independent guard-and-`return` branches, tried in order (the order is
/// load-bearing — earlier guards take precedence), falling through to the
/// UFCS instance-call tail. The collection/numeric/string/option/result
/// kernels and the typed JSON codec delegate to dedicated helpers that
/// return `Option<String>`.
pub(super) fn lower_method_call(e: &Expr, cx: &mut LowerCtx) -> Lowered {
    let ExprKind::MethodCall {
        receiver,
        method,
        type_args,
        args,
        ..
    } = &e.kind
    else {
        unreachable!("lower_method_call is called on a method call")
    };
    let mut pre = Pre::new();
    // Each dispatcher below returns a node that ends in
    // `pre.finish` (1A.20, #3721, Decision G): every family builds nodes (the statics in
    // `lower_call/statics.rs`, the store, D1, cross-context, events, cookie, capability,
    // test-service and agent families each under `lower_call/`), every delegated kernel's node
    // passes through unprinted, and the plain instance call that ends this function is a node.
    // Persisted-store receivers: held Map, Map, Set, Cache, Log, Cell (#3167).
    if let Some(built) = lower_store_method_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    // Intrinsic statics: scalar and collection constructors, then runtime values (#3167).
    if let Some(built) = lower_scalar_static_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_runtime_value_static_call(e, receiver, args, cx, &mut pre) {
        return pre.finish(built);
    }
    // Capability, cross-context, events and static-type calls (#3167).
    if let Some(built) = lower_d1_peek_call(e, cx) {
        return pre.finish(built);
    }
    if let Some(built) = lower_d1_select_call(e, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) =
        lower_cross_context_call(e, receiver, method, type_args, args, cx, &mut pre)
    {
        return pre.finish(built);
    }
    if let Some(built) = lower_events_emit_call(e, receiver, type_args, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_signed_cookie_call(e, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_cookie_field_call(e, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_capability_call(e, receiver, method, type_args, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_type_static_call(e, receiver, method, type_args, args, cx, &mut pre)
    {
        return pre.finish(built);
    }
    // Test-case and system-tier service addresses (1A.26, #3727), then agent invocations (#3167).
    if let Some(built) = lower_test_asset_fetch_call(e, receiver, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_system_schedule_call(e, receiver, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_system_http_call(e, receiver, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_test_service_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_inline_agent_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    if let Some(built) = lower_let_bound_agent_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    // v0.20b: built-in kernel methods, resolved by the checker (#3164).
    if let Some(built) = lower_kernel_method_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish(built);
    }
    // Instance call: UFCS lowering with the receiver as first arg.
    let built = lower_ufcs_method_call(e, receiver, method, type_args, args, cx, &mut pre);
    pre.finish(built)
}

/// Lower a cross-context capability call (`B.Cap.op(…)`) or service call, or `None` when the call is not one. The
/// capability half is `lower_call/platform.rs`'s (1A.24), the service-call half `lower_call/cross_context.rs`'s (1A.25).
fn lower_cross_context_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    if let Some(built) = lower_cross_capability_call(e, receiver, method, type_args, args, cx, pre)
    {
        return Some(built);
    }
    // v0.6 cross-context service call: receiver is an alias or the
    // dotted name of a consumed context. #1961: unless the checker resolved
    // this call to something else — a local shadowing the alias
    // (`1203_cross_context_call_shadowed_by_local`) is a UFCS call, and
    // lowering it over the wire named a Service Binding the Worker, which
    // binds only what it calls, does not declare.
    if !matches!(cx.commons().callee(e.id), Some(c) if !matches!(c, Callee::Cross { .. })) {
        // #3894: `<context>.<service>.outcome(args)`.
        if let Some((prefix, service)) = outcome_service_of(e, receiver, method, cx) {
            return lower_cross_context_service_call(prefix, service, args, true, cx, pre);
        }
        return lower_cross_context_service_call(receiver, method, args, false, cx, pre);
    }
    None
}

/// #3894: the context prefix and service of `<context>.<service>.outcome(…)`, when the checker
/// resolved this call to that service (`Callee::Cross`, and `receiver` is not itself a consumed
/// context, which is a service named `outcome`).
fn outcome_service_of<'a>(
    e: &Expr,
    receiver: &'a Expr,
    method: &Ident,
    cx: &LowerCtx,
) -> Option<(&'a Expr, &'a Ident)> {
    let ExprKind::FieldAccess {
        receiver: prefix,
        field: service,
    } = &receiver.kind
    else {
        return None;
    };
    let resolved = matches!(
        cx.commons().callee(e.id),
        Some(Callee::Cross { service: s, .. }) if *s == service.name
    );
    (method.name == "outcome"
        && resolved
        && cross_context_lowering_prefix(receiver, cx).is_none()
        && cross_context_lowering_prefix(prefix, cx).is_some())
    .then_some((prefix, service))
}

/// The instance-call fallback (UFCS lowering, #3167): the receiver becomes the
/// first argument of the namespace function for the receiver's checked type. The
/// result is the call expression; hoisted statements go to `pre`.
fn lower_ufcs_method_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> bynk_ts::TsExpr {
    let tys = cx.ty_render();
    // Instance call: UFCS lowering with the receiver as first arg. The
    // namespace comes from the receiver's checked type. Every method call the
    // checker accepts has one: a test-service call's outcome is typed by its
    // handler (#1934), where it once reached a placeholder and failed only at
    // `tsc` (#1931). A receiver with no namespace is a missing checker fact
    // (1A.34), reported at the receiver.
    let Some(ns) = cx.receiver_namespace(receiver) else {
        return cx.missing_fact(receiver.span, "receiver namespace for an instance call");
    };
    let recv = pre.lower_node(receiver, cx);
    let mut all = vec![recv];
    for a in args {
        all.push(pre.lower_node(a, cx));
    }
    // Constrained generics slice 1: the call's comparator witnesses follow the
    // ordinary arguments.
    let mut needs_exec = false;
    if let Some(Callee::Method(decl, witnesses)) = cx.commons().callees.get(&e.id) {
        all.extend(cx.witness_args(decl, witnesses));
        // #2719: a method in the needs-context set takes the context last.
        if cx.commons().exec_plan.fn_needs(decl) {
            all.push(cx.exec_node());
            needs_exec = true;
        }
    }
    // #2795: explicit type arguments on a generic instance method are kept, the
    // receiver's own parameters first (the emitted `<A, U>` order), so a call
    // with no argument of type `U` still types as written. The method's own
    // arguments are what the author wrote; a receiver argument is spelled only
    // when it is a base or built-in type, which is in scope everywhere, and is
    // the named widening `unnameable_receiver_type_arg` otherwise (a type the
    // calling module may never import, or a type variable that may not be this
    // function's).
    let explicit: Vec<bynk_ts::TsType> = if type_args.is_empty() {
        Vec::new()
    } else {
        let recv_args: Option<Vec<TyId>> =
            cx.commons().expr_types.get(&receiver.id).and_then(|te| {
                if let Ty::Named { args, .. } = &*tys.get(te.ty) {
                    Some(args.clone())
                } else {
                    None
                }
            });
        match recv_args {
            Some(recv_args) => recv_args
                .iter()
                .map(|a| {
                    if ts_spelling_in_scope_everywhere(*a, tys.types()) {
                        ty_to_type_ref(*a, tys.types())
                            .map(|r| ts_type_ref_to_ts_type(&r, None))
                            .unwrap_or_else(|| {
                                cx.missing_fact_ty(
                                    receiver.span,
                                    "codec shape for a receiver type argument",
                                )
                            })
                    } else {
                        unnameable_receiver_type_arg()
                    }
                })
                .chain(type_args.iter().map(|t| ts_type_ref_to_ts_type(t, None)))
                .collect(),
            None => Vec::new(),
        }
    };
    // Every call is a node, with a `TypeApp` callee when type arguments are explicit (1A.20,
    // #3721, Decision A).
    let mut callee = bynk_ts::TsExpr::Member {
        object: Box::new(ident(&ns)),
        property: method.name.clone(),
    };
    if !explicit.is_empty() {
        callee = bynk_ts::TsExpr::TypeApp {
            callee: Box::new(callee),
            type_args: explicit,
        };
    }
    let built = call(callee, all);
    if needs_exec {
        settle_built(built, cx)
    } else {
        built
    }
}

/// #2795: whether a type's TypeScript spelling is in scope in every emitted
/// module: base types, `Unit` and the built-in containers over them.
fn ts_spelling_in_scope_everywhere(t: TyId, tys: &Arc<Types>) -> bool {
    match &*tys.get(t) {
        Ty::Base(_) | Ty::Unit => true,
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => ts_spelling_in_scope_everywhere(*a, tys),
        Ty::Result(a, b) | Ty::Map(a, b) => {
            ts_spelling_in_scope_everywhere(*a, tys) && ts_spelling_in_scope_everywhere(*b, tys)
        }
        Ty::Error
        | Ty::Never
        | Ty::Named { .. }
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
        | Ty::Actor(..)
        | Ty::ActorSum(..)
        | Ty::Fn { .. }
        | Ty::Var(..) => false,
    }
}

/// #2045: on `workers`, a variant of a consumed context's sum built inline —
/// `({ tag: "Held", reason: x } as Status)` — rather than through the callee's
/// companion (`Status.Held(x)`), which would make the callee's module a value
/// import of this Worker (#661). The companion's constructors are pure data,
/// so the literal is the same value; the cast keeps the tag narrow, through
/// the type-only import of `Status`. `None` for any other sum.
pub(in crate::emitter) fn consumed_variant_literal(
    e: &Expr,
    sum: &str,
    tag: &str,
    args: Vec<bynk_ts::TsExpr>,
    cx: &LowerCtx,
) -> Result<bynk_ts::TsExpr, Vec<bynk_ts::TsExpr>> {
    let shape = (|| {
        if !cx.is_consumed_context_type(sum) {
            return None;
        }
        let decl = cx.commons().types.get(sum)?;
        let TypeBody::Sum(s) = &decl.body else {
            return None;
        };
        let variant = s.variants.iter().find(|v| v.name.name == tag)?;
        if variant.payload.len() != args.len() {
            return None;
        }
        let ty = cx.commons().expr_types.get(&e.id)?.ty;
        let props: Vec<String> = variant
            .payload
            .iter()
            .map(|p| crate::emitter::payload_prop(&p.name.name))
            .collect();
        Some((
            crate::emitter::ts_types::ts_ty_to_ts_type(ty, cx.ty_render()),
            props,
        ))
    })();
    let Some((ty, props)) = shape else {
        return Err(args);
    };
    Ok({
        let c = args;
        let mut entries = vec![bynk_ts::TsObjectEntry::prop(
            "tag".to_string(),
            str_lit(tag),
        )];
        entries.extend(
            props
                .iter()
                .zip(c)
                .map(|(p, a)| bynk_ts::TsObjectEntry::prop(p.clone(), a)),
        );
        bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
            expr: Box::new(bynk_ts::TsExpr::object_entries(entries)),
            ty,
        }))
    })
}

/// Lower a variant call's arguments. #2341 (L4): a `RawError`'s first argument
/// is an `ErrorStatus` name the checker resolved, so it lowers to its number
/// (`Forbidden` → `403`), not to the `HttpResult` value of that name.
pub(in crate::emitter) fn lower_http_variant_args(
    e: &Expr,
    args: &[Expr],
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> Vec<bynk_ts::TsExpr> {
    // 1A.33: the variant is the one the checker resolved, not the call's spelling.
    let raw_error = matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic {
            op: StaticOp::HttpResult(v),
            ..
        }) if v.payload == bynk_syntax::ast::HttpVariantPayload::RawError
    );
    args.iter()
        .enumerate()
        .map(|(i, a)| {
            if raw_error
                && i == 0
                && let Some(status) = bynk_syntax::ast::raw_error_status(a)
            {
                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(status.to_string()))
            } else {
                pre.lower_node(a, cx)
            }
        })
        .collect()
}

/// #2719 (G): which helper a test `case` that calls a function in the
/// needs-context set directly settles the call with: on virtual time it is
/// driven (auto-advance) and then settled, otherwise the sends it handed the
/// harness's execution context are awaited, so the case's `expect`s see their
/// effects. `None` outside a test case, or when nothing needs settling.
pub(in crate::emitter) fn settle_helper(cx: &LowerCtx) -> Option<&'static str> {
    if !cx.in_test_case() {
        None
    } else if cx.runtime_use().virtual_time() {
        Some("__bynkSettleVirtual")
    } else if cx.test_settles_sends() {
        Some("__bynkSettle")
    } else {
        None
    }
}

/// [`settle_helper`] over a built call: `helper(deps, call)`.
pub(in crate::emitter) fn settle_built(call: bynk_ts::TsExpr, cx: &LowerCtx) -> bynk_ts::TsExpr {
    settle_with(settle_helper(cx), call)
}

/// [`settle_built`] with the helper already read, for a caller that lowers its
/// arguments between reading the settle state and settling (the test-case
/// service call reads it first, as it always has).
pub(in crate::emitter) fn settle_with(
    helper: Option<&'static str>,
    call: bynk_ts::TsExpr,
) -> bynk_ts::TsExpr {
    match helper {
        None => call,
        Some(helper) => bynk_ts::TsExpr::Call {
            callee: Box::new(ident(helper)),
            args: vec![ident("deps"), call],
        },
    }
}

pub(super) fn lower_call(e: &Expr, name: &Ident, args: &[Expr], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    // Bare variant constructor with payload → qualify.
    let mut args_lowered = lower_http_variant_args(e, args, &mut pre, cx);
    // #2620: `unreachable(msg)` / `todo(msg)` lower to a call of the runtime's
    // `never`-returning `__fault`, which throws — valid in any expression
    // position, and inside an async body it rejects the effect.
    if let Some(Callee::Fault { kind }) = cx.commons().callee(e.id) {
        // The location is a string-literal node: the printer escapes it with
        // the same `escape_ts_literal` the text form used.
        let location = cx
            .assert_loc()
            .map(|_| expect_location(cx, e.span.start))
            .or_else(|| cx.fault_location(e.span.start))
            .unwrap_or_else(|| format!("offset {}", e.span.start));
        args_lowered.insert(0, str_lit(*kind));
        args_lowered.push(str_lit(location));
        return pre.finish(call(ident("__fault"), args_lowered));
    }
    // #3051: `TestConnection[F]()` — a case's recording handle for a websocket
    // connection.
    if let Some(Callee::Intrinsic {
        ns: IntrinsicNs::TestConnection,
        ..
    }) = cx.commons().callee(e.id)
    {
        let frame = match cx.commons().expr_ty(e.id).as_deref() {
            Some(Ty::TestConnection(f)) => {
                crate::emitter::ts_types::ts_ty_to_ts_type(*f, cx.ty_render())
            }
            _ => cx.missing_fact_ty(
                e.span,
                "`TestConnection[_]` type for a `TestConnection()` call",
            ),
        };
        // `new TestConnection<F>()`: a `New` over a `TypeApp` (1A.20, #3721).
        return pre.finish(bynk_ts::TsExpr::New {
            callee: Box::new(bynk_ts::TsExpr::TypeApp {
                callee: Box::new(ident("TestConnection")),
                type_args: vec![frame],
            }),
            args: vec![],
        });
    }
    // v0.9: HttpResult variant call.
    //
    // P6.21/P6.23 (review of #1251): reads `Callee::Intrinsic` instead of
    // the `Ty::HttpResult`/`Ty::QueueResult` type check plus a bare
    // keyword lookup — consistency with the bare-`Ident` sibling in
    // `lower_ident`, not a new defect closure.
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic {
            ns: IntrinsicNs::HttpResult,
            ..
        })
    ) {
        let callee = bynk_ts::TsExpr::Member {
            object: Box::new(ident("HttpResult")),
            property: name.name.clone(),
        };
        return pre.finish(call(callee, args_lowered));
    }
    // v0.44: a QueueResult variant call (`Retry(reason)`) → `QueueResult.Retry(...)`.
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic {
            ns: IntrinsicNs::QueueResult,
            ..
        })
    ) {
        let callee = bynk_ts::TsExpr::Member {
            object: Box::new(ident("QueueResult")),
            property: name.name.clone(),
        };
        return pre.finish(call(callee, args_lowered));
    }
    // P6.21 (real P6.2 cutover, partial — #1143/#1140's `Callee` classification
    // read back instead of re-derived): agent instantiation `AgentName(key)`
    // lowers to the generated `__makeAgentName(key)` factory, which obtains
    // the instance for this key (lookup-or-create against the registry in
    // bundle mode, or a typed DO proxy in workers mode). Skipped when this
    // Call is the receiver of a MethodCall — that path folds construction
    // and the method invocation together. Previously guarded by
    // `cx.local_agents.contains(&name.name)`, a name-matched re-derivation
    // of what the checker's own `Callee::AgentInit` already resolved once —
    // the same defect class `block_writes_state`'s receiver-name matching
    // carried before #1196 converted it to read `Callee::Store` directly.
    if matches!(cx.commons().callee(e.id), Some(Callee::AgentInit(_))) {
        return pre.finish(lower_agent_init(name, args_lowered, cx));
    }
    // P6.21: sum-variant construction is `Callee::Ctor` — reads the sum
    // type's own name and the variant's own tag straight off the checker's
    // classification instead of `call_is_sum_variant`'s own name-matched
    // `sum_name`/`call_name` string comparison against the call's checked
    // type (the same re-derivation this whole `Callee` precedent, P6.0,
    // exists to close).
    if let Some(Callee::Ctor { sum, tag }) = cx.commons().callee(e.id) {
        let args_lowered = match consumed_variant_literal(e, &sum.name.name, tag, args_lowered, cx)
        {
            Ok(built) => return pre.finish(built),
            // Not an inline literal: the companion call below.
            Err(args) => args,
        };
        let callee = bynk_ts::TsExpr::Member {
            object: Box::new(ident(&sum.name.name)),
            property: tag.to_string(),
        };
        return pre.finish(call(callee, args_lowered));
    }
    // Constrained generics slice 1: the call's comparator witnesses follow the
    // ordinary arguments (an `Ord`-bounded callee's hidden trailing parameters).
    let mut needs_exec = false;
    if let Some(Callee::Fn(decl, witnesses)) = cx.commons().callee(e.id) {
        args_lowered.extend(cx.witness_args(decl, witnesses));
        // #2868: a function that declares `given` takes the caller's `deps`,
        // which the checker has made carry its capabilities.
        if !decl.given.is_empty() {
            args_lowered.push(cx.cap_deps_node());
        }
        // #2719: a function in the needs-context set takes the context last.
        if cx.commons().exec_plan.fn_needs(decl) {
            args_lowered.push(cx.exec_node());
            needs_exec = true;
        }
    }
    let mut built = call(ident(ts_ident(&name.name)), args_lowered);
    if needs_exec {
        built = settle_built(built, cx);
    }
    pre.finish(built)
}
