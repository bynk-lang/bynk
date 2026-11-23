//! Capabilities, derived capabilities and providers: the interface, the provider classes and
//! their dependency order (`emit_capability`, `emit_provider`, `topo_order_providers`).

use super::{cap_ref_ty, consumed_type_ns, qualified_to_ns, workers_env_ty};
use crate::emit_model::EmitProjectCtx;
use crate::emitter::ctx::{BodyMode, HandlerShared, ModuleCtx};
use crate::emitter::lower_ctx::LowerCtx;
use crate::emitter::lower_stmt::function_body_stmts;
use crate::emitter::ts_types::{ts_ty_to_ts_type, ts_type_ref_to_ts_type};
use crate::emitter::{BuildTarget, UnitFacts, ts_ident};
use bynk_check::checker::TypedCommons;
use bynk_model::{ActorSeamIr, OpSig, is_effectful_return};
use bynk_syntax::ast::{CapabilityDecl, ProviderDecl};
use std::collections::HashSet;

/// v0.12: order a context's providers so each appears after the providers of
/// the capabilities it depends on (its `given`). Used by the composition root
/// to emit `const <Cap> = new <Provider>({ deps })` bindings in dependency
/// order. Cycles are rejected by the checker, so this terminates; the marker is
/// set before recursing as a defensive guard. Keyed by capability name.
pub(crate) fn topo_order_providers(
    providers: &std::collections::HashMap<String, ProviderDecl>,
) -> Vec<String> {
    fn visit(
        node: &str,
        providers: &std::collections::HashMap<String, ProviderDecl>,
        visited: &mut HashSet<String>,
        order: &mut Vec<String>,
    ) {
        if visited.contains(node) {
            return;
        }
        visited.insert(node.to_string());
        if let Some(p) = providers.get(node) {
            let given = bynk_model::lower_provider_given_ir(p);
            let mut deps: Vec<&str> = given
                .iter()
                .filter(|d| d.context.is_none())
                .map(|d| d.name.as_str())
                .filter(|n| providers.contains_key(*n))
                .collect();
            deps.sort_unstable();
            for d in deps {
                visit(d, providers, visited, order);
            }
        }
        order.push(node.to_string());
    }
    let mut order = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut keys: Vec<&String> = providers.keys().collect();
    keys.sort();
    for k in keys {
        visit(k, providers, &mut visited, &mut order);
    }
    order
}

/// P6.x (#1193, slice 3 of #1187): reads each op's resolved
/// `params`/`return_ty` off `ops` ([`bynk_model::OpSig`], from
/// `bynk_model::lower_capability_ops_ir`) through `ts_ty`, instead of
/// walking `CapabilityOp::params`/`return_type` `TypeRef`s through
/// `ts_type_ref` directly. `c` is still needed alongside `ops` — an `OpSig`
/// carries neither `c.documentation`/`op.documentation` nor the `Arc` back
/// to the declaration #1188 could rely on for `Type` ([DECISION A]).
/// `ops`/`c.ops` are zipped by index: both are built in declaration order
/// (`lower_capability_ops_ir` maps `cap.ops` in place), never reordered by
/// either side.
///
/// Accepted divergence (review of #1194, widening #1193's own disclosure):
/// `resolve_type_ref` returns `None` — falling back to `Unit`/`void` here —
/// for two shapes `ts_type_ref` rendered directly before this slice: an
/// unresolvable type name (`void` instead of the raw bad name) and
/// `TypeRef::History` (`void` instead of `ts_type_ref`'s own `"never"`,
/// `emitter.rs:4127`). Both are reachable only through a capability op,
/// since the resolver skips `CommonsItem::Capability` outright — no program
/// the checker actually validates can reach either.
/// #1357 (R7.1): both declarations this function builds are now real
/// `bynk_ts` nodes. `export interface {Name} { op<T>(params): ret; ... }` is
/// a real [`bynk_ts::TsDecl::Interface`] over real, per-op
/// [`bynk_ts::TsTypeMember::Method`] entries — `generics`/`doc` are this
/// slice's own real gap (bare names for `generics`, matching every other
/// real generics-list precedent in this crate; `doc` mirrors
/// [`bynk_ts::TsObjectEntry::Method.doc`]'s own identical field, #1337) —
/// params route through the already-real [`ts_ty_to_ts_type`] (P7.9,
/// #1315) instead of the opaque pre-printed `String` `ts_ty` returns. The
/// injection token (`export const __{Name}Token: symbol =
/// globalThis.Symbol("{Name}");`, #1653) is a real
/// [`bynk_ts::TsDecl::ConstDecl`]. This function's own exact signature is
/// unchanged, the P7.9/step-1 pattern — it never owned a `Verbatim`
/// construction site.
/// #1478: real-node-internally already — returns its two real declarations
/// (an interface, an injection-token const) directly instead of writing into
/// `out: &mut String`.
pub(crate) fn emit_capability(
    c: &CapabilityDecl,
    ops: &[OpSig],
    commons: &TypedCommons,
    unit: UnitFacts<'_>,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    if let Some(doc) = c.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    // Review of #1194: `zip` silently truncates to the shorter side — this
    // guards the by-index pairing invariant `ops`/`c.ops` are documented
    // (not enforced) to share, so a future second caller that passes a
    // mismatched `ops` loses trailing interface methods loudly instead of
    // silently.
    debug_assert_eq!(
        c.ops.len(),
        ops.len(),
        "`ops` is zipped with `c.ops` by index — the two must be the same lowering"
    );
    let members: Vec<bynk_ts::TsTypeMember> = c
        .ops
        .iter()
        .zip(ops)
        .map(|(op, sig)| {
            // Constrained generics slice 3 (DECISION D): a parameter whose type
            // mentions a `Codable` type parameter is encoded whole at the call
            // site, so the provider receives a JSON value, never a witness.
            let codable = bynk_check::checker::codable::codable_op_params(op);
            let encoded: Vec<bool> = op
                .params
                .iter()
                .map(|p| bynk_check::checker::codable::mentions_any(&p.type_ref, &codable))
                .collect();
            let params: Vec<bynk_ts::TsParam> = sig
                .params
                .iter()
                .zip(&encoded)
                .map(|((name, ty), encoded)| {
                    bynk_ts::TsParam::new(
                        ts_ident(name),
                        Some(if *encoded {
                            bynk_ts::TsType::named("unknown")
                        } else {
                            ts_ty_to_ts_type(*ty, unit.render(commons))
                        }),
                    )
                })
                .collect();
            // #3458: a typed read's `Effect[P]` payload mentions a `Codable`
            // parameter, so the provider replies with plain JSON that the call
            // site decodes: the interface returns `unknown` there.
            let typed_read = matches!(
                &op.return_type,
                bynk_syntax::ast::TypeRef::Effect(payload, _)
                    if bynk_check::checker::codable::mentions_any(payload, &codable)
            );
            // A type parameter that only occurs in encoded parameters is not in
            // the interface (nothing would use it).
            let kept: Vec<String> = op
                .type_params
                .iter()
                .map(|tp| tp.name.name.clone())
                .filter(|tp| {
                    codable.is_empty()
                        || op
                            .params
                            .iter()
                            .zip(&encoded)
                            .filter(|(_, enc)| !**enc)
                            .any(|(p, _)| {
                                bynk_check::checker::codable::mentions_any(
                                    &p.type_ref,
                                    std::slice::from_ref(tp),
                                )
                            })
                        || (!typed_read
                            && bynk_check::checker::codable::mentions_any(
                                &op.return_type,
                                std::slice::from_ref(tp),
                            ))
                })
                .collect();
            // #926 (Decision C): a genuine generic TS interface method, no
            // monomorphisation/erasure. Reads `op.type_params` (not
            // `sig.type_params`): it only extracts each `TypeParam`'s own
            // name, no type resolution, and `OpSig::type_params` exists for
            // `lower_op_sig_ir`'s own rigid-variable scoping, not for this
            // to render from (Decision A/B, #1193).
            bynk_ts::TsTypeMember::Method {
                name: op.name.name.clone(),
                generics: kept,
                params,
                ret: if typed_read {
                    bynk_ts::TsType::named_with_args(
                        "globalThis.Promise",
                        vec![bynk_ts::TsType::named("unknown")],
                    )
                } else {
                    ts_ty_to_ts_type(sig.return_ty, unit.render(commons))
                },
                doc: op.documentation.as_deref().map(str::to_owned),
            }
        })
        .collect();
    // #1486: `no_blank_before` when this capability's own doc comment is the
    // preceding statement — see `emit_free_fn`'s own identical note. The two
    // explicit `TsStmt::blank(None)` pushes this function used to carry
    // (after `interface`, after `token_decl`) are gone for the same reason
    // `emit_refined_type`'s own note gives.
    let mut interface = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Interface {
            name: c.name.name.clone(),
            type_params: Vec::new(),
            members,
        })),
        None,
    );
    interface.no_blank_before = c.documentation.is_some();
    stmts.push(interface);

    // Injection token (symbol carrying the interface type). #1653: typed
    // `symbol`, not `unique symbol` — TypeScript only gives a bare `Symbol(…)`
    // call a unique-symbol type, and the call is qualified as
    // `globalThis.Symbol` so a user type named `Symbol` cannot hide it.
    let token_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: format!("__{}Token", c.name.name),
            ty: Some(bynk_ts::TsType::named("symbol")),
            init: bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.Symbol".to_string())),
                args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    c.name.name.clone(),
                ))],
            },
        })),
        None,
    );
    stmts.push(token_decl);
    stmts
}

/// S4a (#1966): a derived capability (`capability Sessions = Kv`) is its
/// base's interface under its own name, `export type Sessions =
/// bynk_cloudflare.Kv;`, so everything typed by the capability (a handler's
/// `deps`, the context's deps interface, a test double) has the base's ops,
/// and compose's base provider satisfies it. The injection token is the same
/// as a declared capability's.
pub(crate) fn emit_derived_capability(
    c: &CapabilityDecl,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    let Some(derived) = &c.derived else {
        return Vec::new();
    };
    let base = &derived.base.name;
    // The checker resolved the base among the flattened capabilities
    // (`bynk.capability.unknown_base`), so a miss is a broken build already
    // reported; the bare name keeps the output printable.
    let ty = match ctx.cross_context.flattened_caps.get(base) {
        Some(unit) => format!("{}.{base}", qualified_to_ns(unit)),
        None => base.clone(),
    };
    let mut stmts = Vec::new();
    if let Some(doc) = c.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let mut alias = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::TypeAlias {
            name: c.name.name.clone(),
            type_params: Vec::new(),
            ty: bynk_ts::TsType::named(ty),
        })),
        None,
    );
    alias.no_blank_before = c.documentation.is_some();
    stmts.push(alias);
    stmts.push(bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: format!("__{}Token", c.name.name),
            ty: Some(bynk_ts::TsType::named("symbol")),
            init: bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.Symbol".to_string())),
                args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    c.name.name.clone(),
                ))],
            },
        })),
        None,
    ));
    stmts
}

/// A provider's declarations (1A.32a, Decision A of #3733): its doc comment, if any, then
/// `export class P implements Cap { .. }` as a [`bynk_ts::TsDecl::Class`] over typed members, then
/// the `export const __PProvider = { token, factory }` registration. `None` for an external
/// (bodiless) provider, which the adapter's binding supplies.
///
/// Source maps come from the printer alone: each op's [`bynk_ts::TsClassMethod::span`] anchors its
/// header and prologue to its own `fn` line (#2280), the class's `end_span` anchors the closing
/// brace to the provider, and the registration carries the provider's span. The caller gives the
/// first statement the provider's span, as for every other item.
pub(crate) fn emit_provider(
    p: &ProviderDecl,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Option<Vec<bynk_ts::TsStmt>> {
    // v0.17: an external (bodiless) provider inside an adapter is supplied by
    // the adapter's binding — the compiler emits no class for it. Its symbol is
    // imported and constructed by the consumer's compose (§6.1, Phase 2).
    if p.external {
        return None;
    }
    // v0.12: a provider with `given` receives its dependencies through a
    // constructor; its bodies call them as `this.deps.<cap>`. The deps object
    // type lists exactly the provider's `given` capabilities.
    //
    // P7.2 (review of #1300): computed once here and reused below (the
    // capability-scope collection in each op, and the factory's own `deps` type).
    let given_ir = bynk_model::lower_provider_given_ir(p);
    // #2355: a provider whose ops construct a local agent hands `this.deps` to
    // the agent's methods, so it takes a `deps` even with no `given`; on
    // `workers` that `deps` also carries the Worker's `env`, which the agent
    // factory reads its Durable Object namespace from.
    let constructs_agent =
        bynk_check::project_model::provider_constructs_agent(p, |n| ctx.local_agents.contains(n));
    let takes_deps = !p.given.is_empty() || constructs_agent;
    let mut deps_members: Vec<bynk_ts::TsTypeMember> = given_ir
        .iter()
        .map(|c| bynk_ts::TsTypeMember::prop(c.name.clone(), cap_ref_ty(c, &ctx.cross_context)))
        .collect();
    if constructs_agent && matches!(ctx.target, BuildTarget::Workers) {
        let mut agents: Vec<String> = ctx.local_agents.iter().cloned().collect();
        agents.sort();
        deps_members.push(bynk_ts::TsTypeMember::prop(
            "env",
            workers_env_ty(&[], &agents),
        ));
    }
    let deps_ty = bynk_ts::TsType::Object(deps_members);
    let prov = p.provider_name.name.as_str();
    let mut class = bynk_ts::TsDecl::class(
        prov,
        Vec::new(),
        None,
        p.ops
            .iter()
            .map(|op| provider_op_method(p, op, &given_ir, takes_deps, commons, ctx))
            .collect(),
    )
    .implementing(vec![bynk_ts::TsType::named(p.capability.name.as_str())])
    .ending_at(p.span);
    if takes_deps
        && let bynk_ts::TsDecl::Class {
            fields,
            constructor,
            ..
        } = &mut class
    {
        // The field is declared separately and assigned in the constructor body
        // rather than written as a parameter property (`constructor(private deps:
        // …)`). A parameter property is the one type-directed construct pure
        // type-stripping cannot erase — Node's `--experimental-strip-types` throws
        // `ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX` on it — so this de-sugared form keeps
        // the emitter's whole surface strip-removable (the strip-only emission
        // invariant, ADR 0136). The field stays typed for the `tsc`/strict path,
        // and under ES2022 `useDefineForClassFields` the declaration defines
        // `this.deps` before the body assigns the real value; the end state is
        // correct and strips to `constructor(deps) { this.deps = deps; }`.
        fields.push(bynk_ts::TsClassField::new("deps", deps_ty.clone()).private());
        *constructor = Some(bynk_ts::TsClassCtor {
            params: vec![deps_param(&deps_ty)],
            body: vec![bynk_ts::TsStmt::assign(
                bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
                    property: "deps".to_string(),
                },
                bynk_ts::TsExpr::Ident("deps".to_string()),
                None,
            )],
        });
    }
    let mut stmts = Vec::new();
    let has_doc = p.documentation.is_some();
    if let Some(doc) = p.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let mut class_stmt = bynk_ts::TsStmt::decl(bynk_ts::TsDecl::Export(Box::new(class)), None);
    // A leading doc comment sits directly above the declaration it documents.
    class_stmt.no_blank_before = has_doc;
    stmts.push(class_stmt);
    // The registration: `token` names the capability, `factory` builds the class from
    // its dependencies. Its own line belongs to the provider, not to the last op.
    let construct = |args: Vec<bynk_ts::TsExpr>| bynk_ts::TsExpr::New {
        callee: Box::new(bynk_ts::TsExpr::Ident(prov.to_string())),
        args,
    };
    let factory = bynk_ts::TsExpr::Arrow {
        params: if takes_deps {
            vec![deps_param(&deps_ty)]
        } else {
            Vec::new()
        },
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(construct(
            if takes_deps {
                vec![bynk_ts::TsExpr::Ident("deps".to_string())]
            } else {
                Vec::new()
            },
        )))),
    };
    let token = format!("__{}Token", p.capability.name);
    stmts.push(bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: format!("__{prov}Provider"),
            ty: None,
            init: bynk_ts::TsExpr::object(vec![
                ("token".to_string(), bynk_ts::TsExpr::Ident(token)),
                ("factory".to_string(), factory),
            ]),
        })),
        Some(p.span),
    ));
    Some(stmts)
}

/// `deps: <deps type>`: the one parameter of a provider's constructor and of its factory.
fn deps_param(deps_ty: &bynk_ts::TsType) -> bynk_ts::TsParam {
    bynk_ts::TsParam {
        name: "deps".to_string(),
        ty: Some(deps_ty.clone()),
        optional: false,
        rest: false,
        array_pattern: None,
        default: None,
    }
}

/// One provider op as a class method, its body lowered through
/// [`function_body_stmts`]. #2280: the method's span anchors its header and prologue to its
/// own `fn` line rather than the previous op's last body line.
fn provider_op_method(
    p: &ProviderDecl,
    op: &bynk_syntax::ast::ProviderOp,
    given_ir: &[bynk_model::CapRefIr],
    takes_deps: bool,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> bynk_ts::TsClassMethod {
    let params: Vec<bynk_ts::TsParam> = op
        .params
        .iter()
        .map(|p| {
            bynk_ts::TsParam::new(
                ts_ident(&p.name.name),
                Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
            )
        })
        .collect();
    // P6.55 (design/tracks/the-ir.md §6b): computed once, not twice.
    let effectful = is_effectful_return(&op.return_type);
    let mut module = ModuleCtx::for_unit(commons, ctx);
    module.agent_method_givens = ctx.agent_method_givens.clone();
    module.agent_called_contexts = ctx.agent_called_contexts.clone();
    module.event_schema_versions = ctx.event_schema_versions.clone();
    module.consumed_type_ns = consumed_type_ns(ctx);
    module.consumed_type_declared = ctx.consumed_type_aliases.clone();
    module.consumed_context_types = ctx.consumed_context_types();
    module.target = ctx.target;
    module.note_unit(ctx);
    let mut cx = LowerCtx::new(
        module,
        BodyMode::ProviderOp {
            handler: HandlerShared {
                // The provider's `given` capabilities are in scope in its
                // bodies, and resolve against the injected `this.deps`.
                capabilities: given_ir.iter().map(|c| c.name.clone()).collect(),
                deps_on_this: takes_deps,
                // #934: a provider's own op body can itself call a `given`
                // capability (e.g. a provider that dedups its own work via
                // `Idempotency`).
                handler_scope: Some(format!(
                    "{}.provides.{}.{}",
                    ctx.commons_name, p.capability.name, op.name.name
                )),
                owning_context: ctx.commons_name.clone(),
                ..HandlerShared::default()
            },
        },
    );
    cx.local_agents = ctx.local_agents.clone();
    // #1653: parameters are bindings of the body's own scope, so a `let` that
    // re-binds one gets a fresh name instead of redeclaring it.
    for p in &op.params {
        cx.declare_binder(&p.name.name);
    }
    let body = function_body_stmts(&op.body, &mut cx, effectful, Some(&op.return_type));
    bynk_ts::TsClassMethod {
        span: Some(op.span),
        ..bynk_ts::TsClassMethod::new(
            op.name.name.clone(),
            effectful,
            params,
            Some(ts_type_ref_to_ts_type(&op.return_type, None)),
            body,
        )
    }
}

/// Append one field to a `deps` object type under construction —
/// `emit_service`'s own actor-seam/`__exec`/events-dispatch widening all
/// need this. Review of #1209: gathering the actor-seam widening into one
/// `match` (below) made this splice's own repetition newly visible — six
/// copies in `emit_service` alone before this helper.
///
/// Arc F, slice 4 (#1463): widening is now a plain `Vec::push`, replacing the
/// `trim_end_matches('}')` string splice this helper used before — genuinely
/// safer, not just tidier (review of #1469): `trim_end_matches` strips *all*
/// trailing braces, so the old splice only produced the right answer because
/// a nested object field always happened to end with a space before its own
/// closing brace — `__exec`'s own `{ waitUntil(...): void }` shape was one
/// stray space away from silently losing a brace. Takes the member list
/// directly (review of #1469) rather than a `TsType` that would only ever be
/// the `Object` variant — no runtime check, and no panic path, for a shape
/// mismatch that can't happen.
pub(super) fn append_deps_field(
    deps_members: &mut Vec<bynk_ts::TsTypeMember>,
    name: impl Into<String>,
    ty: bynk_ts::TsType,
) {
    deps_members.push(bynk_ts::TsTypeMember::prop(name, ty));
}

/// The type of `deps.identity` for a `Caller` or `Sender` handler. For `Sender`
/// (#3328) it is the structural type of the minted `SenderIdentity`. Structural, so the handler module needs no import of the
/// `bynk.cloudflare` declaration the checker typed `s.identity` by, and the
/// declared interface is assignable to it.
pub(super) fn seam_identity_ts_type(seam: &ActorSeamIr) -> bynk_ts::TsType {
    if !matches!(seam, ActorSeamIr::Sender(_)) {
        // `Caller`: the calling context's name.
        return bynk_ts::TsType::named("string");
    }
    bynk_ts::TsType::Object(vec![
        bynk_ts::TsTypeMember::readonly_prop("address", bynk_ts::TsType::named("string")),
        bynk_ts::TsTypeMember::readonly_prop(
            "dmarc",
            // #3397: the `DmarcVerdict` union itself, not `{ tag: string }`,
            // so `s.identity.dmarc` passes where a `DmarcVerdict` is expected.
            // Keep the variants in step with `DmarcVerdict` in
            // `bynk-std/lib/cloudflare/bynk.cloudflare.bynk` and the runtime's
            // `email.ts`.
            bynk_ts::TsType::union(
                ["DmarcPass", "DmarcFail", "DmarcNone"]
                    .into_iter()
                    .map(|v| {
                        bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::readonly_prop(
                            "tag",
                            bynk_ts::TsType::named(format!("\"{v}\"")),
                        )])
                    })
                    .collect(),
            ),
        ),
    ])
}
