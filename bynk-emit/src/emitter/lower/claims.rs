//! The Idempotency claim scope a service handler opens (back-half paydown 1A.35a part B, 2.2b,
//! #4087), with its tests. Moved out of `lower.rs` byte for byte.

use super::member_node;
use crate::ts_build::ident;
#[cfg(test)]
use crate::ts_build::num_lit;

#[cfg(test)]
use super::super::RuntimeUse;
#[cfg(test)]
use super::super::ctx::UnitFacts;
#[cfg(test)]
use super::super::lower_ctx::LowerCtx;
#[cfg(test)]
use super::test_support::Printed;

/// The text the scope declaration was written as, kept to check [`claim_scope_declaration`]
/// prints exactly it.
#[cfg(test)]
pub(crate) const CLAIM_SCOPE_DECLARATION: &str = "const __claims = (deps.Idempotency as unknown as { __track?: () => { note<C>(k: string, c: C, t: number): C; lease(t: number): number; settle(r: unknown, threw: boolean): globalThis.Promise<void> } }).__track?.() ?? { note: <C>(_k: string, c: C, _t: number) => c, lease: (t: number) => t, settle: async (_r: unknown, _threw: boolean) => {} };";

/// W30 (#2840): the statement that opens a service handler's claim scope, which
/// wraps the already-lowered function body (`lower_stmt::claim_scoped`) so the
/// claims it made are settled when it exits. `__claims` is the provider's own
/// scope (`Idempotency.__track()`, hidden: not in the capability), or a no-op
/// scope for a provider without one, which takes the claim for its full
/// window and never settles it. A thrown fault releases the claims; a
/// returned `Err` or a 5xx `HttpResult` does too; any other outcome confirms
/// them.
///
/// [`CLAIM_SCOPE_DECLARATION`] as a node (1A.35a part B): `const __claims = (deps.Idempotency
/// as unknown as { __track?: () => { note<C>(…): C; lease(…): number; settle(…): … } })
/// .__track?.() ?? { note: …, lease: …, settle: … };`.
pub(crate) fn claim_scope_declaration() -> bynk_ts::TsStmt {
    use bynk_ts::{
        TsArrowBody, TsBinaryOp, TsBindingName, TsExpr, TsObjectEntry, TsParam, TsStmt, TsType,
        TsTypeMember,
    };
    let named = |name: &str| TsType::named(name);
    let param = |name: &str, ty: &str| TsParam::new(name, Some(named(ty)));
    // The provider's scope: `{ note<C>(k, c, t): C; lease(t): number; settle(r, threw): Promise<void> }`.
    let scope_ty = TsType::Object(vec![
        TsTypeMember::Method {
            name: "note".to_string(),
            generics: vec!["C".to_string()],
            params: vec![param("k", "string"), param("c", "C"), param("t", "number")],
            ret: named("C"),
            doc: None,
        },
        TsTypeMember::method("lease", vec![param("t", "number")], named("number")),
        TsTypeMember::method(
            "settle",
            vec![param("r", "unknown"), param("threw", "boolean")],
            TsType::named_with_args("globalThis.Promise", vec![named("void")]),
        ),
    ]);
    let track_ty = TsType::Object(vec![TsTypeMember::optional_prop(
        "__track",
        TsType::Fn {
            rest: false,
            params: Vec::new(),
            names: Vec::new(),
            ret: Box::new(scope_ty),
        },
    )]);
    let provider = TsExpr::Paren(Box::new(TsExpr::As {
        expr: Box::new(TsExpr::As {
            expr: Box::new(member_node("deps", "Idempotency")),
            ty: named("unknown"),
        }),
        ty: track_ty,
    }));
    let tracked = TsExpr::OptionalCall {
        callee: Box::new(TsExpr::Member {
            object: Box::new(provider),
            property: "__track".to_string(),
        }),
        args: Vec::new(),
    };
    // The no-op scope: takes every claim for its full window and never settles.
    let arrow = |generics: &[&str], params: Vec<TsParam>, is_async: bool, body: TsArrowBody| {
        TsExpr::Arrow {
            params,
            is_async,
            generics: generics.iter().map(ToString::to_string).collect(),
            return_type: None,
            body: Box::new(body),
        }
    };
    let noop = TsExpr::object_entries(vec![
        TsObjectEntry::prop(
            "note",
            arrow(
                &["C"],
                vec![
                    param("_k", "string"),
                    param("c", "C"),
                    param("_t", "number"),
                ],
                false,
                TsArrowBody::Expr(Box::new(ident("c"))),
            ),
        ),
        TsObjectEntry::prop(
            "lease",
            arrow(
                &[],
                vec![param("t", "number")],
                false,
                TsArrowBody::Expr(Box::new(ident("t"))),
            ),
        ),
        TsObjectEntry::prop(
            "settle",
            arrow(
                &[],
                vec![param("_r", "unknown"), param("_threw", "boolean")],
                true,
                TsArrowBody::Block(Vec::new()),
            ),
        ),
    ]);
    TsStmt::const_stmt(
        TsBindingName::Ident("__claims".to_string()),
        None,
        TsExpr::Binary {
            op: TsBinaryOp::NullishCoalescing,
            left: Box::new(tracked),
            right: Box::new(noop),
        },
        None,
    )
}

#[cfg(test)]
mod idempotency_scoping_tests {
    use super::*;
    use crate::emitter::ctx::{BodyMode, HandlerShared, ModuleCtx};
    use crate::emitter::lower_call::platform::scope_idempotency_key;
    use bynk_check::checker::TypedCommons;

    // P6.38 (design/tracks/the-ir.md §6a): reads `TypedCommons::empty()`
    // (bynk-check) instead of hand-building an empty `Commons` here — this
    // module's own last `bynk_syntax::ast` dependency, now gone.
    fn empty_commons() -> TypedCommons {
        TypedCommons::empty()
    }

    /// #934 (Decision C): a first-party `Idempotency.dedup`/`remember` call
    /// reaching `scope_idempotency_key` with no `handler_scope` set means some
    /// emission site populated `capabilities` without also wiring the scope —
    /// a compiler bug. This must fail loudly, not silently ship an unscoped
    /// key; nothing in the fixture suite exercises a missing scope (every real
    /// site sets it), so this is the only proof the guard actually fires.
    #[test]
    #[should_panic(expected = "handler_scope")]
    fn missing_handler_scope_panics() {
        let commons = empty_commons();
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
        );
        let mut args = vec![ident("orderId")];
        scope_idempotency_key(true, "dedup", &mut args, &cx);
    }

    /// The mirror case: a real scope present, the call proceeds and the key
    /// is prefixed — confirms the panic in the test above is really guarding
    /// the `None` case and not firing unconditionally.
    #[test]
    fn present_handler_scope_prefixes_the_key() {
        let commons = empty_commons();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(
                &commons,
                &cross_context,
                &runtime_use,
                UnitFacts::detached(),
            ),
            BodyMode::ServiceHandler {
                handler: HandlerShared {
                    handler_scope: Some("shop.reserve.ordering.call".to_string()),
                    ..HandlerShared::default()
                },
                deps_identity_binder: None,
                actor_sum_binder: None,
            },
        );
        let mut args = vec![ident("orderId")];
        scope_idempotency_key(true, "dedup", &mut args, &cx);
        assert_eq!(
            args[0].clone().printed(),
            "`shop.reserve.ordering.call::${orderId}`"
        );
    }

    /// `claim` takes its key first, like `dedup` and `remember`, so it is
    /// scoped to the calling handler the same way.
    #[test]
    fn claim_is_scoped_like_dedup() {
        let commons = empty_commons();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(
                &commons,
                &cross_context,
                &runtime_use,
                UnitFacts::detached(),
            ),
            BodyMode::ServiceHandler {
                handler: HandlerShared {
                    handler_scope: Some("shop.hooks.api.POST".to_string()),
                    ..HandlerShared::default()
                },
                deps_identity_binder: None,
                actor_sum_binder: None,
            },
        );
        let mut args = vec![ident("id"), num_lit("3600000".to_string())];
        scope_idempotency_key(true, "claim", &mut args, &cx);
        assert_eq!(args[0].clone().printed(), "`shop.hooks.api.POST::${id}`");
        assert_eq!(args[1].clone().printed(), "3600000");
    }

    /// The declaration node prints the very text it replaced (1A.35a part B), token for token
    /// and in its one-line layout.
    #[test]
    fn the_claim_scope_declaration_node_prints_its_old_text() {
        assert_eq!(
            bynk_ts::print_stmt(&claim_scope_declaration(), 0),
            format!("{CLAIM_SCOPE_DECLARATION}\n")
        );
    }

    /// A provider with no `__track` gets a scope that takes a claim for its
    /// whole window, so nothing is cut to the lease with no settle to extend it.
    #[test]
    fn a_provider_without_a_scope_takes_the_full_window() {
        let before = CLAIM_SCOPE_DECLARATION;
        assert!(before.contains("lease: (t: number) => t"), "{before}");
        assert!(before.contains("__track?.() ??"), "{before}");
    }

    /// A capability merely named `"Idempotency"` from some other adapter or
    /// context (not first-party `bynk`) must pass through untouched even with
    /// no scope set — `is_first_party: false` short-circuits before the
    /// `handler_scope` check, so this must NOT panic.
    #[test]
    fn non_first_party_capability_is_not_scoped() {
        let commons = empty_commons();
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
        );
        let mut args = vec![ident("orderId")];
        scope_idempotency_key(false, "dedup", &mut args, &cx);
        assert_eq!(args[0].clone().printed(), "orderId");
    }
}
