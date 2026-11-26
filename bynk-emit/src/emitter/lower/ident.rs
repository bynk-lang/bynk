//! Identifiers and field access (back-half paydown 1A.4, 2.2d, #4087): identifier lowering, field
//! access and map-key decoding, with their tests. Moved out of `lower.rs` byte for byte.

use bynk_check::builtin_names::types::*;
use bynk_check::checker::{Callee, NamedKind, Ty, TyId};
use bynk_check::kernel_op::IntrinsicNs;
use bynk_syntax::ast::{BaseType, Expr, ExprKind, Ident, TypeBody};

use super::super::internal_fault::FaultSite;
use super::super::lower_call::query::{held_values_source, path_values_source};
use super::super::lower_ctx::LowerCtx;
use super::super::ts_types::{TyRender, ts_base, ts_ty_to_ts_type};
use super::super::{RAW, map_query, ts_ident};
use super::{Lowered, Pre, consumed_variant_literal, member_node, one, type_member_node};
use crate::ts_build::ident;

/// `(() => values)`: a store read used as a value, the deferred `Query` thunk (1A.23, #3724).
fn store_thunk(values: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Arrow {
        params: vec![],
        is_async: false,
        generics: vec![],
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(values))),
    }))
}

/// Slice 1A.4 (#3701): the identifier lowering, as a [`bynk_ts::TsExpr`] (a node, or the
/// text of a branch that still builds a closure or thunk). The branch order is
/// deliberate and unchanged: constants, `Ord`/needs closures, transition
/// records, invariant fields, store cells, the held/store-`Map` and `Log`
/// thunks, `HttpResult`/`QueueResult` constructors, sum variants, the actor-sum
/// binder, then a plain local.
pub(super) fn lower_ident(e: &Expr, id: &Ident, cx: &mut LowerCtx) -> bynk_ts::TsExpr {
    // #2617: a constant is a hoisted, cached function (`emit_consts`), so a
    // name the checker resolved to one reads it with a call. First, since the
    // checker has already ruled out a local or any other meaning of the name.
    if let Some(name) = cx.const_ref(e) {
        return bynk_ts::TsExpr::Call {
            callee: Box::new(ident(ts_ident(name))),
            args: Vec::new(),
        };
    }
    // Constrained generics slice 1 (DECISION E): a generic function with `Ord`
    // bounds used as a value is instantiated at the reference, as a closure that
    // captures the comparator witnesses.
    if let Some(Callee::FnValue(decl, witnesses)) = cx.commons().callee(e.id)
        && (!witnesses.is_empty()
            || !decl.given.is_empty()
            || cx.commons().exec_plan.fn_needs(decl))
    {
        let tys = cx.ty_render();
        let params: Vec<bynk_ts::TsParam> = match cx
            .commons()
            .expr_types
            .get(&e.id)
            .map(|te| (*tys.get(te.ty)).clone())
        {
            Some(Ty::Fn { params, .. }) => params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    bynk_ts::TsParam::new(format!("__a{i}"), Some(ts_ty_to_ts_type(*p, tys)))
                })
                .collect(),
            _ => (0..decl.params.len())
                .map(|i| {
                    bynk_ts::TsParam::new(format!("__a{i}"), Some(super::lower_list::any_ty()))
                })
                .collect(),
        };
        let mut args: Vec<bynk_ts::TsExpr> = params.iter().map(|p| ident(&p.name)).collect();
        args.extend(cx.witness_args(decl, witnesses));
        // #2868: a `given` function closes over the `deps` in scope where it is
        // named.
        if !decl.given.is_empty() {
            args.push(cx.cap_deps_node());
        }
        // #2719: a function in the needs-context set closes over the context in
        // scope where it is named.
        if cx.commons().exec_plan.fn_needs(decl) {
            args.push(cx.exec_node());
        }
        return bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Arrow {
            params,
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(
                bynk_ts::TsExpr::Call {
                    callee: Box::new(ident(ts_ident(&id.name))),
                    args,
                },
            ))),
        }));
    }
    // v0.116: inside a `transition` predicate, the contextual `old`/`new` idents
    // are the pre-/post-commit state records, lowered to their JS names (`new` is
    // reserved, so both are renamed). Field access `old.<f>` reads off the record.
    // Checked first so `old`/`new` never collide with the heuristics below.
    if let Some((old_var, new_var)) = cx.transition_states() {
        if id.name == "old" {
            return ident(old_var);
        }
        if id.name == "new" {
            return ident(new_var);
        }
    }
    // v0.80: inside an invariant predicate, a bare ident naming a state field
    // reads it off the proposed-state value (`s.<field>`). Checked first so a
    // field never collides with the variant-constructor heuristics below.
    if let Some((var, fields)) = cx.invariant_state()
        && fields.contains(&id.name)
    {
        return member_node(var, &id.name);
    }
    // v0.81: inside a `store`-agent handler, a bare ident naming a `Cell` field
    // reads it off the mutable working state (`__state.<cell>`), so a read after
    // a `:=` write in the same handler sees the written value (read-your-writes).
    if let Some((var, cells)) = cx.agent_store_cells()
        && cells.contains(&id.name)
    {
        return member_node(var, &id.name);
    }
    // v0.104/v0.105 (real-time track slice 3b): a bare held-`Map` ident used as a
    // value is a lazy `Query` over its **resolved** connections — the persisted
    // `Record<K, connId>` mapped through `resolveConnection`, keeping the present
    // ones. Checked before the persisted-`Map` branch (held maps are excluded from
    // `agent_store_maps`).
    if !cx.is_local(&id.name)
        && let Some(frame) = cx.agent_held_map_frame(&id.name).cloned()
    {
        let recv = member_node(cx.agent_store_var(), &id.name);
        return store_thunk(held_values_source(recv, &frame));
    }
    // v0.94 (ADR 0120): a bare `store Map` ident used as a **value** — not a
    // method receiver (those are handled in the method dispatch) — is a lazy
    // `Query` over the whole map, e.g. the `other` side of a join. It lowers to
    // the same deferred thunk a query builder yields: `() => Object.values(map)`.
    if !cx.is_local(&id.name) && cx.is_agent_store_map(&id.name) {
        let recv = member_node(cx.agent_store_var(), &id.name);
        return store_thunk(path_values_source(recv));
    }
    // v0.95 (ADR 0121): a bare `store Log` ident used as a value is a lazy
    // `Query` over its entry values — `() => log.map((__e) => __e.v)`.
    if !cx.is_local(&id.name) && cx.is_agent_store_log(&id.name) {
        let recv = member_node(cx.agent_store_var(), &id.name);
        return store_thunk(path_values_source(recv));
    }
    // v0.9: a nullary HttpResult variant (whose checker type is
    // `HttpResult[_]`) constructs an HttpResult.<Variant>.
    //
    // P6.21/P6.23 (review of #1251): reads `Callee::Intrinsic` instead of
    // the `Ty::HttpResult`/`Ty::QueueResult` type check plus a bare
    // `http_variant`/`queue_variant` keyword lookup — consistency with
    // every other static-constructor branch in this module, not a new
    // defect closure: the resolved-type guard here was already immune to
    // name-matching (a shadowing local never types as `Ty::HttpResult`).
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic {
            ns: IntrinsicNs::HttpResult,
            ..
        })
    ) {
        return member_node("HttpResult", &id.name);
    }
    // v0.44: a nullary QueueResult variant (`Ack`) constructs `QueueResult.Ack`.
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic {
            ns: IntrinsicNs::QueueResult,
            ..
        })
    ) {
        return member_node("QueueResult", &id.name);
    }
    // A bare ident whose name matches a declared variant of a sum
    // type (and whose checker type is that sum) is a nullary
    // variant constructor reference. Qualify it as `Type.Variant`.
    // Otherwise (locals, params, `self`) emit the identifier as-is.
    if let Some(Ty::Named {
        kind: NamedKind::Sum,
        name: type_name,
        ..
    }) = cx.commons().expr_ty(e.id).as_deref()
        && let Some(decl) = cx.commons().types.get(type_name)
        && let TypeBody::Sum(s) = &decl.body
        && s.variants.iter().any(|v| v.name.name == id.name)
    {
        if let Ok(built) = consumed_variant_literal(e, type_name, &id.name, Vec::new(), cx) {
            return built;
        }
        return type_member_node(type_name, &id.name);
    }
    // v0.52: the multi-actor sum binder is not a runtime local — the resolved
    // actor is threaded through `deps.who` at the boundary wrapper, so the
    // binder ident lowers to it (the tagged union the body `match`es).
    if cx.actor_sum_binder() == Some(id.name.as_str())
        && matches!(cx.commons().expr_ty(e.id).as_deref(), Some(Ty::ActorSum(_)))
    {
        return member_node("deps", "who");
    }
    // #908: a re-`let` of this name (in this or an enclosing block) renamed
    // its emitted binding — resolve to it rather than the natural
    // (now-shadowed) `ts_ident` name.
    ident(
        cx.resolved_local_name(&id.name)
            .unwrap_or_else(|| ts_ident(&id.name)),
    )
}

pub(super) fn lower_field_access(
    e: &Expr,
    receiver: &Expr,
    field: &Ident,
    cx: &mut LowerCtx,
) -> Lowered {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    // v0.158 (ADR 0184): `<map>.entries` / `.keys` / `.values` on a `store
    // Map[K, V]` field — the key-exposing lazy queries. Like every store-map
    // query, they lower to a deferred thunk over the working record. `.values`
    // scans `Object.values` (the existing value lift); `.keys` scans
    // `Object.keys`; `.entries` zips both into `{ key, value }` records
    // (`MapEntry[K, V]`). A persisted key is a JS object key (a string), so an
    // `Int`-typed key is decoded back with `Number(...)`; a `String`-typed key
    // is already correct; a refined or opaque key is asserted to its branded
    // type (#1982).
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.is_agent_store_map(&id.name)
        && !cx.is_agent_held_map(&id.name)
        && matches!(
            field.name.as_str(),
            map_query::ENTRIES | map_query::KEYS | map_query::VALUES
        )
    {
        let m = member_node(cx.agent_store_var(), &id.name);
        let key = |cx: &LowerCtx| {
            decode_map_key(map_key_ty(e, cx), ident("__k"), tys, cx.fault_site(e.span))
        };
        let is_identity =
            |k: &bynk_ts::TsExpr| matches!(k, bynk_ts::TsExpr::Ident(n) if n == "__k");
        let values = |call: &str| bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(m.clone()),
                property: call.to_string(),
            }),
            args: vec![],
        };
        let arrow = |params: Vec<&str>, body: bynk_ts::TsExpr| bynk_ts::TsExpr::Arrow {
            params: params
                .into_iter()
                .map(|p| bynk_ts::TsParam::new(p.to_string(), None))
                .collect(),
            is_async: false,
            generics: vec![],
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(body))),
        };
        let call = |f: &str, args: Vec<bynk_ts::TsExpr>| bynk_ts::TsExpr::Call {
            callee: Box::new(ident(f)),
            args,
        };
        let built: bynk_ts::TsExpr = match field.name.as_str() {
            map_query::VALUES => store_thunk(values("values")),
            map_query::KEYS => {
                let decoded = key(cx);
                if is_identity(&decoded) {
                    store_thunk(values("keys"))
                } else {
                    let map = bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Member {
                            object: Box::new(ident("__ks")),
                            property: "map".to_string(),
                        }),
                        args: vec![arrow(vec!["__k"], decoded)],
                    };
                    store_thunk(call(
                        "__bynkThen",
                        vec![values("keys"), arrow(vec!["__ks"], map)],
                    ))
                }
            }
            // entries: each `[__k, __v]` pair zipped into a `{ key, value }` record.
            _ => {
                let record = bynk_ts::TsExpr::Object {
                    entries: vec![
                        bynk_ts::TsObjectEntry::prop("key", key(cx)),
                        bynk_ts::TsObjectEntry::prop("value", ident("__v")),
                    ],
                    multiline: false,
                    end_span: None,
                };
                let pair = bynk_ts::TsExpr::Arrow {
                    params: vec![bynk_ts::TsParam::array_pattern(
                        vec!["__k".to_string(), "__v".to_string()],
                        None,
                    )],
                    is_async: false,
                    generics: vec![],
                    return_type: None,
                    body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(record))),
                };
                let zipped = bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(ident("__es")),
                        property: "map".to_string(),
                    }),
                    args: vec![pair],
                };
                store_thunk(call(
                    "__bynkThen",
                    vec![values("entries"), arrow(vec!["__es"], zipped)],
                ))
            }
        };
        return pre.finish(built);
    }
    // v0.9: `HttpResult.Variant` (nullary).
    //
    // P6.21/P6.23 (review of #1251): reads `Callee::Intrinsic` instead of
    // `id.name == HTTP_RESULT` — see the `MethodCall` sibling above (this
    // module's own qualified-with-args `HttpResult.Variant(args)` branch)
    // for the full reasoning.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic {
                ns: IntrinsicNs::HttpResult,
                ..
            })
        )
    {
        return pre.finish(member_node("HttpResult", &field.name));
    }
    // #2073 (with #2045): a qualified nullary variant of a consumed context's
    // sum (`Region.Domestic`), on `workers`, is built inline as its bare form
    // is, so the callee's module stays a type-only import.
    if let ExprKind::Ident(id) = &receiver.kind
        && !cx.is_local(&id.name)
        && let Ok(built) = consumed_variant_literal(e, &id.name, &field.name, Vec::new(), cx)
    {
        return pre.finish(built);
    }
    // Agent-handler `self.<key>` rewrite.
    if cx.in_agent_handler()
        && let ExprKind::Ident(id) = &receiver.kind
        && id.name == "self"
        && let Some(k) = cx.agent_key_field()
        && field.name == *k
    {
        // #1818: the key this instance was addressed by. It used to read
        // `this.state.id.toString()` (the Durable Object's hex id on workers,
        // "[object Object]" on bundle) cast to the key's *name*.
        return pre.finish(member_node("this", "__key"));
    }
    // v0.45: `<binder>.identity` on a verified actor binding. The binder is not
    // a runtime value; the identity is minted at the verification seam. For the
    // zero-crypto schemes the sealed identity carries no payload, so it lowers
    // to the unit value (`undefined`). Authenticated identities (Bearer/
    // Signature) and the calling-context value arrive with their later slices.
    //
    // v0.47: a Bearer handler's identity is minted at the verification seam and
    // threaded through `deps`, so `<binder>.identity` reads `deps.identity`.
    // Other (unit) identities — `None`/`Internal` actors — carry no payload and
    // stay the unit value `undefined`.
    if field.name == "identity"
        && matches!(
            cx.commons().expr_ty(receiver.id).as_deref(),
            Some(Ty::Actor(_))
        )
    {
        if let (Some(binder), ExprKind::Ident(id)) = (cx.deps_identity_binder(), &receiver.kind)
            && id.name == binder
        {
            return pre.finish(member_node("deps", "identity"));
        }
        return pre.finish(ident("undefined"));
    }
    // #2380 (L5): an `Oidc` binder's `.claims` reads `deps.claims`, which its
    // seam filled from the verified token.
    if field.name == "claims"
        && matches!(
            cx.commons().expr_ty(receiver.id).as_deref(),
            Some(Ty::Actor(_))
        )
        && let (Some(binder), ExprKind::Ident(id)) = (cx.deps_identity_binder(), &receiver.kind)
        && id.name == binder
    {
        return pre.finish(member_node("deps", "claims"));
    }
    let r = pre.lower_node(receiver, cx);
    // `.raw` on an opaque value compiles to a TypeScript type
    // assertion back to the base type. The checker has already
    // verified that the receiver is opaque and the call site is
    // inside the defining commons.
    if field.name == RAW
        && let Some(Ty::Named {
            kind: NamedKind::Opaque(base),
            ..
        }) = cx.commons().expr_ty(receiver.id).as_deref()
    {
        let base = ts_base(*base);
        let built = {
            let c = vec![r];
            bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
                expr: Box::new(one(c)),
                ty: bynk_ts::TsType::named(base),
            }))
        };
        return pre.finish(built);
    }
    let built = {
        let c = vec![r];
        bynk_ts::TsExpr::Member {
            object: Box::new(one(c)),
            property: field.name.clone(),
        }
    };
    pre.finish(built)
}

/// v0.158 (ADR 0184): the key type `K` of a map `.entries`/`.keys` query, read
/// off the field-access result type — `Query[MapEntry[K, V]]` for `.entries`,
/// `Query[K]` for `.keys`. Used to decode persisted string object-keys back to
/// `K`. `None` only when the accessor's type was not recorded — which the
/// checker never leaves unset for a well-typed `.entries`/`.keys` (it stamps the
/// result type at `e.span`), so a `None` here is a checker/emitter invariant
/// break, not a runtime input. The `debug_assert!` in [`decode_map_key`] pins
/// that: the fallback identity decode must never be reached for an `Int` key,
/// which would silently emit a string where a `number` is expected.
fn map_key_ty(e: &Expr, cx: &LowerCtx) -> Option<TyId> {
    let tys = cx.ty_render();
    let Ty::Query(inner) = &*tys.get(cx.commons().expr_types.get(&e.id).map(|te| te.ty)?) else {
        return None;
    };
    if let Ty::Named { name, args, .. } = &*tys.get(*inner)
        && name == MAP_ENTRY
        && !args.is_empty()
    {
        Some(args[0])
    } else {
        Some(*inner)
    }
}

/// v0.158 (ADR 0184): decode a persisted map key (a JS object key, always a
/// string) back to its bynk type. Value-keyable keys are `Int`/`String` and
/// refinements/opaques over them (ADR 0110 D5); an `Int`-based key erases to
/// `number`, so it is parsed with `Number(...)`, while a `String`-based key is
/// already correct and passes through unchanged. A refined or opaque key is
/// then asserted back to its branded type (#1982).
///
/// A key type the checker did not record, or one that is not value-keyable, is a
/// missing fact (1A.34): the identity pass-through that used to stand in would
/// silently emit a string for an `Int` key.
fn decode_map_key(
    k: Option<TyId>,
    raw: bynk_ts::TsExpr,
    tys: TyRender<'_>,
    at: FaultSite<'_>,
) -> bynk_ts::TsExpr {
    let (base, branded) = match k.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Base(b)) => (Some(*b), false),
        Some(Ty::Named {
            kind: NamedKind::Refined(b) | NamedKind::Opaque(b),
            ..
        }) => (Some(*b), true),
        _ => (None, false),
    };
    // #1982: a refined or opaque key is a branded TS type (`Sku = string & {
    // readonly __brand: "Sku" }`), which neither the raw string nor its
    // `Number(...)` is. The key was that type when it was put, and its storage
    // is the base value, so asserting it back is sound.
    let brand = |decoded: bynk_ts::TsExpr| match (branded, k) {
        (true, Some(k)) => bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::As {
            expr: Box::new(decoded),
            ty: ts_ty_to_ts_type(k, tys),
        })),
        _ => decoded,
    };
    match base {
        Some(BaseType::Int) => brand(bynk_ts::TsExpr::Call {
            callee: Box::new(member_node("globalThis", "Number")),
            args: vec![raw],
        }),
        Some(BaseType::String) => brand(raw),
        // #70/#71 review: `resolver.rs`'s `check_map_key_keyable` and
        // `validate.rs`'s `@indexed`-field check both gate value-keyability
        // (ADR 0110 D5) on exactly `String`/`Int` — the same pair this match
        // now names explicitly. If either of those independent copies ever
        // widens the accepted set without this one being taught the new
        // type's wire shape, silently falling through to bare `raw` would
        // *miscompile* rather than reject — e.g. a `Bool` key needs
        // `raw === "true"`, not the raw string. Fail loudly instead.
        // Fail loudly instead (1A.34: report it).
        Some(other) => at.missing_fact(format_args!(
            "value-keyable map key type: {other:?} is not (ADR 0110 D5)"
        )),
        None => at.missing_fact("value-keyable map key type for `.entries`/`.keys`"),
    }
}

#[cfg(test)]
mod decode_map_key_tests {
    use super::super::super::RuntimeUse;
    use super::*;
    use crate::emitter::ts_types::no_spellings;
    use bynk_check::checker::Types;
    use std::sync::Arc;

    /// One table for this module's fixtures — `Types` is `Send + Sync`
    /// (T3.6b), so a `LazyLock` static serves every test without threading.
    static TYS: std::sync::LazyLock<Arc<Types>> =
        std::sync::LazyLock::new(|| Arc::new(Types::new()));

    fn decoded(k: Option<TyId>) -> (String, Vec<crate::emitter::InternalFault>) {
        let ru = RuntimeUse::default();
        let at = FaultSite {
            sink: &ru,
            span: bynk_syntax::span::Span::new(3, 9),
        };
        let node = decode_map_key(k, ident("k"), TyRender::new(&TYS, no_spellings()), at);
        (bynk_ts::print_expr(&node), ru.take_faults())
    }

    fn print_decoded(k: Option<TyId>) -> String {
        let (text, faults) = decoded(k);
        assert!(faults.is_empty(), "{faults:?}");
        text
    }

    #[test]
    fn int_key_is_parsed_with_number() {
        let int = TYS.intern(Ty::Base(BaseType::Int));
        assert_eq!(print_decoded(Some(int)), "globalThis.Number(k)");
    }

    #[test]
    fn string_key_passes_through() {
        let string = TYS.intern(Ty::Base(BaseType::String));
        assert_eq!(print_decoded(Some(string)), "k");
    }

    #[test]
    fn refined_int_key_is_parsed_with_number() {
        let refined = TYS.intern(Ty::Named {
            name: "UserId".to_string(),
            kind: NamedKind::Refined(BaseType::Int),
            args: Vec::new(),
        });
        assert_eq!(
            print_decoded(Some(refined)),
            "(globalThis.Number(k) as UserId)"
        );
    }

    #[test]
    fn opaque_string_key_is_asserted_to_its_brand() {
        // #1982: `Object.keys` yields `string`, not the branded `Slug`.
        let opaque = TYS.intern(Ty::Named {
            name: "Slug".to_string(),
            kind: NamedKind::Opaque(BaseType::String),
            args: Vec::new(),
        });
        assert_eq!(print_decoded(Some(opaque)), "(k as Slug)");
    }

    // #70/#71 review: a non-`String`/`Int` base reaching here means the
    // value-keyable set widened elsewhere (`resolver.rs`/`validate.rs`)
    // without teaching this decode the new type's wire shape — it must be
    // reported, not silently emit a wrong pass-through (1A.34).
    #[test]
    fn a_widened_non_string_int_base_is_reported_instead_of_silently_passing_through() {
        let b = TYS.intern(Ty::Base(BaseType::Bool));
        let (text, faults) = decoded(Some(b));
        assert_eq!(text, "undefined");
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert!(
            faults[0].what.contains("is not (ADR 0110 D5)"),
            "{faults:?}"
        );
    }

    /// The key type is always recorded for a well-typed `.entries`/`.keys`; one that is not is a
    /// missing fact at the call's span, and no identity decode stands in for it.
    #[test]
    fn an_unrecorded_key_type_is_reported_at_the_call() {
        let (text, faults) = decoded(None);
        assert_eq!(text, "undefined");
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].span, bynk_syntax::span::Span::new(3, 9));
    }
}
