//! Services and contexts: `emit_service` and the context `deps` interface, the `make` surface,
//! the cross-context capability namespaces, and the actor-sum types a service's `deps.who` uses.

use super::{
    append_deps_field, cron_handler_method_name, events_release_body, http_handler_method_name_ir,
    queue_handler_method_name, return_undefined_unless, seam_identity_ts_type,
    sender_admission_guard,
};
use crate::emit_model::EmitProjectCtx;
use crate::emitter::ctx::{BodyMode, HandlerShared, ModuleCtx};
use crate::emitter::lower_ctx::LowerCtx;
use crate::emitter::lower_stmt::{Body, block_body_with_return};
use crate::emitter::ts_types::{ts_ty_to_ts_type, ts_type_ref_to_ts_type};
use crate::emitter::{BuildTarget, UnitFacts, UnitKind, event_pattern_guard_ir, ts_ident};
use crate::ts_build::{binary, ident, member, num_lit};
use bynk_check::actors::ActorDecl;
use bynk_check::checker::TypedCommons;
use bynk_check::firstparty::FirstPartyCap;
use bynk_model::{
    ActorSeamIr, HandlerSignatureIr, IrHandlerKind, ProtocolIr, SchemaVersionIr,
    is_effectful_return, lower_actor_seam_ir, lower_handler_kind_ir,
};
use bynk_syntax::ast::{CommonsItem, Expr, ExprKind, Handler, ServiceDecl};
use std::collections::{HashMap, HashSet};

/// The runtime test a `via schema(...)` guard negates (events slice 4b, #990):
/// `===` for a literal, `>=` and/or `<=` for a range, and none for `_`, which
/// only forwards the envelope. Bounds are inclusive.
fn schema_version_test(env_binder: &str, version: SchemaVersionIr) -> Option<bynk_ts::TsExpr> {
    use crate::ts_build::{and_expr, strict_eq};
    use bynk_ts::TsBinaryOp::{GreaterThanEq, LessThanEq};
    let v = || member(ident(env_binder), "schemaVersion");
    match version {
        SchemaVersionIr::Exact(n) => Some(strict_eq(v(), num_lit(n))),
        SchemaVersionIr::Range {
            lo: Some(lo),
            hi: Some(hi),
        } => Some(and_expr(
            binary(GreaterThanEq, v(), num_lit(lo)),
            binary(LessThanEq, v(), num_lit(hi)),
        )),
        SchemaVersionIr::Range {
            lo: Some(lo),
            hi: None,
        } => Some(binary(GreaterThanEq, v(), num_lit(lo))),
        SchemaVersionIr::Range {
            lo: None,
            hi: Some(hi),
        } => Some(binary(LessThanEq, v(), num_lit(hi))),
        SchemaVersionIr::Range { lo: None, hi: None } | SchemaVersionIr::Any => None,
    }
}

/// A service's declarations (1A.32a, Decision A of #3733): its doc comment, if any, then
/// `export const {name} = { .. };` as a [`bynk_ts::TsDecl::ConstDecl`] over a multiline object
/// with one [`bynk_ts::TsObjectEntry::Method`] per handler.
///
/// Source maps come from the printer alone: each handler's entry span anchors its header and
/// prologue to its own `on call` line (#2280), and the object's `end_span` anchors the closing
/// `};` to the service. The caller gives the first statement the service's span, as for every
/// other item.
pub(crate) fn emit_service(
    s: &ServiceDecl,
    protocol: &ProtocolIr,
    signatures: &[HandlerSignatureIr],
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    let tys = UnitFacts::of(ctx).render(commons);
    let mut entries: Vec<bynk_ts::TsObjectEntry> = Vec::new();
    let mut cron_idx = 0usize;
    let mut queue_idx = 0usize;
    let ws_proto = matches!(protocol, ProtocolIr::WebSocket { .. });
    // #1187's slice 5: `s.handlers`/`signatures` are the same list in the
    // same declaration order — `signatures` is built by mapping `s.handlers`
    // 1:1 at the one call site (`emitter.rs`) — the same zip-by-index
    // precedent `emit_capability`'s own `c.ops`/`ops` pairing already
    // established (#1193).
    for (handler, (ir_params, _ir_given, ir_ret, ir_effectful)) in s.handlers.iter().zip(signatures)
    {
        // v0.104/v0.106 (real-time track slice 3b): on Workers a `from websocket`
        // lifecycle handler (`on open`/`on message`/`on close`) does not emit a
        // service-surface method — its body runs inside the hosting Durable Object
        // (`__wsOpen`/`__wsMessage`/`__wsClose`, DECISION A), driven by the edge
        // upgrade / `webSocketMessage` / `webSocketClose`, not from here. (On bundle
        // these are callable surface methods a `TestConnection` test drives.)
        let handler_kind_ir = lower_handler_kind_ir(&handler.kind);
        let is_ws_handler = matches!(handler_kind_ir, IrHandlerKind::Open | IrHandlerKind::Close)
            || (ws_proto && matches!(handler_kind_ir, IrHandlerKind::Message));
        // Events track, slice 0 (spine #936): unlike the WebSocket lifecycle
        // handlers above, `on event` has no *other* place its body could go
        // on Workers — a WS lifecycle handler's real body lives in the
        // hosting agent's DO (`emit_ws_do_method`); a subscriber service has
        // no such alternate home. So the method is still emitted here on
        // every target — real delivery (the fan-out DO calling in) reaches
        // it directly, not through `compose.ts`'s HTTP-routable surface,
        // which never exposes it regardless (no HTTP method/route to route
        // from).
        if is_ws_handler && matches!(ctx.target, BuildTarget::Workers) {
            continue;
        }
        // #1361: the doc block is no longer written directly here — it now
        // lives on the real `TsObjectEntry::Method` built below (`doc:
        // handler.documentation.clone()`), rendered by `render_multiline_
        // object_entry`'s own `Method` arm at the right depth/position.
        let kind_name = match &handler_kind_ir {
            IrHandlerKind::Call => "call".to_string(),
            IrHandlerKind::Http { method, path } => http_handler_method_name_ir(*method, path),
            IrHandlerKind::Cron { .. } => {
                let name = cron_handler_method_name(&s.name.name, cron_idx);
                cron_idx += 1;
                name
            }
            // v0.106: a `from websocket` `on message` is the inbound surface method.
            IrHandlerKind::Message if ws_proto => "message".to_string(),
            IrHandlerKind::Message => {
                let name = queue_handler_method_name(&s.name.name, queue_idx);
                queue_idx += 1;
                name
            }
            // v0.103/v0.106: the WebSocket lifecycle surface methods.
            IrHandlerKind::Open => "open".to_string(),
            IrHandlerKind::Close => "close".to_string(),
            // Events track, slice 0 (spine #936): exactly one `on event` per
            // `from Events(E)` service.
            IrHandlerKind::Event => "event".to_string(),
            // #2341 (decision C): the router's rejection renderer.
            IrHandlerKind::Rejected => "rejected".to_string(),
        };
        // For service handlers the operation name is the handler kind
        // (e.g. `call`). v0.5 has only one handler kind, so the service is a
        // single-operation object literal.
        let mut params: Vec<bynk_ts::TsParam> = ir_params
            .iter()
            .map(|(name, ty)| {
                bynk_ts::TsParam::new(ts_ident(name), Some(ts_ty_to_ts_type(*ty, tys)))
            })
            .collect();
        // v0.103/v0.106: a `from websocket` lifecycle handler receives the
        // `connection` as its first parameter (the synthetic binding the checker
        // added — the fresh socket for `on open`, the firing socket for `on
        // message`/`on close`); emit it so the lowered body's `connection` resolves.
        if is_ws_handler && let ProtocolIr::WebSocket { out_ty, .. } = protocol {
            params.insert(
                0,
                bynk_ts::TsParam::new(
                    "connection".to_string(),
                    Some(bynk_ts::TsType::named_with_args(
                        "Connection",
                        vec![ts_ty_to_ts_type(*out_ty, tys)],
                    )),
                ),
            );
        }
        // Events track, slice 4 (spine #936): a `via schema(N)` guard needs
        // `env.schemaVersion` regardless of whether the subscriber declared
        // the optional `env: EventEnvelope` parameter (slice 2) — a bare
        // `on event(e: E)` handler has no envelope in scope otherwise. When
        // missing, insert a synthetic parameter in the same position a real
        // `env` would occupy, under a name distinct from anything a user
        // could write, so it can never collide with a declared one. Every
        // envelope-forwarding call site reads `wants_envelope`, so the value
        // actually arrives here.
        // 1B.1c (Decision F of #3928): whether the envelope arrives is the
        // one rule every delivery site reads, `EventSubscriberShape::
        // wants_envelope`; this callee is the side that must line up with it.
        let envelope_shape = bynk_model::EventSubscriberShape {
            two_param_handler: ir_params.len() == 2,
            schema_dispatch: matches!(
                protocol,
                ProtocolIr::Events {
                    schema_dispatch: Some(_),
                    ..
                }
            ),
        };
        let schema_dispatch_env_binder =
            if handler_kind_ir == IrHandlerKind::Event && envelope_shape.wants_envelope() {
                match ir_params.get(1) {
                    Some((env_param_name, _)) => Some(ts_ident(env_param_name)),
                    None => {
                        params.insert(
                            1,
                            bynk_ts::TsParam::new(
                                "__bynkSchemaEnv".to_string(),
                                Some(bynk_ts::TsType::named("EventEnvelope")),
                            ),
                        );
                        Some("__bynkSchemaEnv".to_string())
                    }
                }
            } else {
                None
            };
        // Lower the body first so we can detect cross-context usage and
        // adjust the deps shape accordingly.
        // v0.52: a multi-actor sum handler's resolved actor is threaded through
        // `deps.who`; the binder ident lowers to it so the body can `match`. A
        // sum supersedes the single-actor Bearer identity path (the per-arm
        // identity comes from the match, not a single `deps.identity`).
        // v0.47/v0.151/v0.52/v0.54: a handler's own actor-verification seam —
        // Bearer/Oidc/Caller thread their identity through `deps.identity`,
        // a sum threads the resolved-actor tagged union through `deps.who`
        // (the body `match`es it). `lower_actor_seam_ir`'s own doc comment
        // (`bynk_model::ActorSeamIr`) has the full grounding for why sum is
        // tried first (it can otherwise collide with Bearer) and why the
        // other three are mutually exclusive by construction.
        let seam = lower_actor_seam_ir(handler, &ctx.actors);
        let deps_identity_binder = match &seam {
            ActorSeamIr::Caller(binder) | ActorSeamIr::Sender(binder) => Some(binder.clone()),
            ActorSeamIr::Bearer(s) => s.binder.clone(),
            ActorSeamIr::Oidc(s) => s.binder.clone(),
            ActorSeamIr::SignedCookie(s) => s.binder.clone(),
            ActorSeamIr::Sum(_) | ActorSeamIr::CallSum(_) | ActorSeamIr::None => None,
        };
        let actor_sum_binder = if matches!(seam, ActorSeamIr::Sum(_) | ActorSeamIr::CallSum(_)) {
            handler
                .by_clause
                .as_ref()
                .and_then(|by| by.binder.as_ref())
                .map(|binder| binder.name.clone())
        } else {
            None
        };
        let mut module = ModuleCtx::for_unit(commons, ctx);
        module.note_unit(ctx);
        module.agent_method_givens = ctx.agent_method_givens.clone();
        module.agent_called_contexts = ctx.agent_called_contexts.clone();
        module.event_schema_versions = ctx.event_schema_versions.clone();
        module.consumed_type_ns = consumed_type_ns(ctx);
        module.consumed_type_declared = ctx.consumed_type_aliases.clone();
        module.consumed_context_types = ctx.consumed_context_types();
        module.target = ctx.target;
        let mut cx = LowerCtx::new(
            module,
            BodyMode::ServiceHandler {
                handler: HandlerShared {
                    capabilities: bynk_model::lower_handler_given_ir(handler)
                        .into_iter()
                        .map(|c| c.name)
                        .collect::<HashSet<_>>(),
                    // #934: the qualified handler name `Idempotency.dedup`/
                    // `remember` key scoping prefixes onto the developer-supplied
                    // key.
                    handler_scope: Some(format!(
                        "{}.{}.{}",
                        ctx.commons_name, s.name.name, kind_name
                    )),
                    owning_context: ctx.commons_name.clone(),
                    ..HandlerShared::default()
                },
                deps_identity_binder,
                actor_sum_binder,
            },
        );
        cx.local_agents = ctx.local_agents.clone();
        // #1653: parameters are bindings of the body's own scope, so a `let` that
        // re-binds one gets a fresh name instead of redeclaring it.
        for p in &handler.params {
            cx.declare_binder(&p.name.name);
        }
        let async_tail = *ir_effectful;
        // 1A.15: the body is items, not text: the prologue guards, the lowered
        // body and the events wrapper reach the printer as statements, which
        // records their checkpoints.
        let lowered_body = block_body_with_return(
            &handler.body,
            &mut cx,
            async_tail,
            Some(&handler.return_type),
        );
        let mut guards = Body::new();
        // Events track, slice 1 (spine #936): a `from Events(E { ... })`
        // subscription filter is deliver-and-filter (ADR 0286, unchanged by
        // this slice) — the fan-out mechanism still delivers every emission
        // of `E` to every subscriber; this subscriber's own generated
        // handler evaluates the pattern as a guard and no-ops if it doesn't
        // match. Prologue, not a wrapper edit, so it covers all three
        // delivery paths (Cloudflare Workers, Bundle/node, Bundle/browser)
        // in one place, since they all call into this same generated method.
        // `handler.params[0]`'s type is guaranteed to equal the header's
        // event type by `check_service_protocols`'s param-type-agreement
        // check, so testing its own name (read off `ir_params`, the
        // already-lowered signature `emit_service`'s own loop already
        // holds — P6.54) here is sound.
        if handler_kind_ir == IrHandlerKind::Event
            && let ProtocolIr::Events {
                pattern: Some(pattern),
                ..
            } = protocol
            && let Some((param_name, _)) = ir_params.first()
            && let Some(guard) = event_pattern_guard_ir(&ts_ident(param_name), Some(pattern))
        {
            guards.push(return_undefined_unless(guard));
        }
        // Events track, slice 4 (spine #936): a `via schema(N)` guard,
        // independent of and in addition to the payload-pattern guard above
        // — a service may carry either, both, or neither. Same
        // prologue technique, same three-delivery-path coverage. The
        // envelope binder is either the user's own declared `env` name or
        // the synthetic one inserted above.
        if let ProtocolIr::Events {
            schema_dispatch: Some(version),
            ..
        } = protocol
            && let Some(env_binder) = &schema_dispatch_env_binder
            && let Some(test) = schema_version_test(env_binder, *version)
        {
            // Inserted ahead of the subscriber-filter guard, as it always was.
            let mut schema_guard = Body::new();
            schema_guard.push(return_undefined_unless(test));
            schema_guard.append(std::mem::replace(&mut guards, Body::new()));
            guards = schema_guard;
        }
        // #3888: a `Sender` refinement refuses mail whose DMARC verdict it does not admit before
        // the body runs, whichever tier calls the handler (the Worker, a unit case or a system
        // case), as a rejection and not a fault.
        if handler_kind_ir == IrHandlerKind::Message
            && let Some(admits) = bynk_check::actors::sender_admits_for(handler, &ctx.actors)
        {
            guards.push(sender_admission_guard(&admits));
        }
        // Append the deps parameter (may include surface field if the body
        // made cross-context calls). v0.47: a Bearer handler's deps also carries
        // the seam-minted `identity` — but only when a binder captures it
        // (v0.50: a binder-less Bearer handler verifies but mints no identity).
        let mut deps_members = build_deps_object_ty_with_surface(
            &effective_given(&bynk_model::lower_handler_given_ir(handler), &cx),
            &cx,
            &ctx.cross_context,
            ctx.target,
        );
        // v0.47/v0.151/v0.52/v0.54: widen `deps` for whichever actor seam
        // this handler resolved to — at most one arm ever fires, `seam`
        // being an enum rather than four independent optionals (a stronger
        // guarantee than the four-`if`-in-a-row shape this replaced, whose
        // mutual exclusion depended on the resolver-priority `if`s above
        // rather than the type system).
        match &seam {
            // #3386: a `SignedCookie` seam mints its identity from `sub` just the same.
            ActorSeamIr::Bearer(_) | ActorSeamIr::SignedCookie(_) => {
                if let Some(id) = seam.captured_identity_type() {
                    append_deps_field(&mut deps_members, "identity", bynk_ts::TsType::named(id));
                    // #3937: an HTTP `Bearer` binder reads the verified token's claims too.
                    if matches!(seam, ActorSeamIr::Bearer(_))
                        && matches!(protocol, ProtocolIr::Http)
                    {
                        append_deps_field(
                            &mut deps_members,
                            "claims",
                            crate::emitter::ts_types::token_claims_ts_type(),
                        );
                    }
                }
            }
            // v0.151: an `Oidc`-binding handler threads its `sub`-minted
            // identity into deps exactly like Bearer.
            ActorSeamIr::Oidc(s) if s.binder.is_some() => {
                append_deps_field(
                    &mut deps_members,
                    "identity",
                    bynk_ts::TsType::named(s.identity_type.clone()),
                );
                // #2380 (L5): and the verified token's claims.
                append_deps_field(
                    &mut deps_members,
                    "claims",
                    crate::emitter::ts_types::token_claims_ts_type(),
                );
            }
            // v0.54: a Caller-binding call handler's deps carries the
            // caller's context name as its `CallerId` identity (a `string`).
            // #3328: and a `Sender`-binding email handler's carries the minted
            // `SenderIdentity` (see [`seam_identity_ts_type`]).
            ActorSeamIr::Caller(_) | ActorSeamIr::Sender(_) => {
                append_deps_field(&mut deps_members, "identity", seam_identity_ts_type(&seam));
            }
            // v0.52: a sum handler's deps carries the resolved-actor tagged
            // union (`who`), which the body `match`es. A binder-less sum is
            // rejected by the checker, so a sum handler always captures `who`.
            //
            // Arc F, slice 4 (#1463): review of #1463 found this can't build
            // as a real `TsType::Object`/`TsType::union` — each member's own
            // `{ tag: "Name", identity: T }` shape is comma-separated
            // (matching the pre-existing hand-written text), but
            // `TsType::Object`'s own printer always joins members with `"; "`
            // (`bynk-ts/src/printer.rs`'s own hardcoded convention) —
            // confirmed by a real fixture byte-diff, not assumed. Rebuilding
            // it would silently change the punctuation, not just the
            // construction. Stays the pre-rendered text it already was,
            // wrapped as opaque `Named` — the same "an odd, one-off shape
            // stays opaque text" precedent this track uses elsewhere, not a
            // gap this slice leaves unclosed.
            ActorSeamIr::Sum(members) => {
                append_deps_field(&mut deps_members, "who", actor_sum_ts_union(members));
            }
            // I3 (#1971): a `call` sum. `Caller` carries the calling context's
            // name; a foreign caller is its actor alone.
            ActorSeamIr::CallSum(members) => {
                append_deps_field(&mut deps_members, "who", call_sum_ts_type(members));
            }
            // A binder-less Bearer/Oidc, or no seam at all — nothing to widen.
            ActorSeamIr::Oidc(_) | ActorSeamIr::None => {}
        }
        // v0.79: a handler whose body uses `detach` receives the execution context
        // (`__exec`) in its deps, so the fire-and-forget send can hand its promise
        // to `waitUntil`. Gated on the body so non-sending handlers are unchanged.
        if commons.exec_plan.handler_needs(handler) {
            append_deps_field(
                &mut deps_members,
                "__exec",
                bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::method(
                    "waitUntil",
                    vec![bynk_ts::TsParam::new(
                        "promise".to_string(),
                        Some(bynk_ts::TsType::named_with_args(
                            "globalThis.Promise",
                            vec![bynk_ts::TsType::named("unknown")],
                        )),
                    )],
                    bynk_ts::TsType::named("void"),
                )]),
            );
        }
        // Events track, slice 0 (spine #936): a handler whose body uses
        // `Events.emit` receives a compose-supplied `__eventsDispatch`
        // callback in its deps — the actual fanout delivery mechanism
        // (a Cloudflare Durable Object per publishing context, or an
        // in-process dispatch on Bundle/node) lives at the composition
        // layer, which already knows how to build any capability's
        // provider; the handler itself only needs to hand its buffered
        // `__events` to whatever composed it. Mirrors `__exec`'s threading
        // for `detach` exactly. Gated on more than this handler's own body: a
        // handler with no direct `Events.emit` call but that invokes a
        // *local agent* method which itself emits (`Ledger(id).bump(...)`)
        // still needs `__eventsDispatch` threaded through, to hand onward —
        // `cx.agent_given_caps_used` is exactly the existing mechanism that
        // already propagates an invoked agent method's other `given`
        // capabilities up to the caller (`effective_given`, just below);
        // `Events` rides the same path since it's declared as an ordinary
        // `given Events` on the agent handler.
        let needs_events_dispatch = cx.is_first_party(FirstPartyCap::Events)
            && (bynk_model::block_uses_emit(&handler.body, &commons.callees)
                || cx
                    .agent_given_caps_used()
                    .is_some_and(|m| m.contains_key("Events")));
        if needs_events_dispatch {
            // Arc F, slice 4 (#1463): a bare function type has no way to name
            // its own parameter (`TsType::Fn`'s params are anonymous,
            // positional `a0`/`a1`/… — its own doc), the same real algebra
            // gap `emit_context_deps_interface`'s own identical field already
            // named (#1453) — stays opaque `Named` text, not a new pattern.
            append_deps_field(
                &mut deps_members,
                "__eventsDispatch",
                bynk_ts::TsType::named(format!(
                    "(events: globalThis.Array<{}>) => globalThis.Promise<void>",
                    crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE
                )),
            );
        }
        // Arc F, slice 4 (#1463): `deps`'s own type is now the real
        // `TsType::Object` `build_deps_object_ty_with_surface` builds,
        // widened in place by every `append_deps_field` call above — closes
        // #1361's own named residual ("deps's own type stays one opaque
        // `TsType::named` string... real, separate, much larger work than
        // this slice's own scope").
        params.push(bynk_ts::TsParam::new(
            "deps".to_string(),
            Some(bynk_ts::TsType::Object(deps_members)),
        ));
        // Events track, slice 0 (spine #936): release-at-commit (the retired
        // events track's §3.0, `design/archive/retired-tracks.md`) needs a
        // completion boundary services never had before — the
        // body runs inside an IIFE so its own `return`s resolve `__result`
        // rather than the outer method, and only a handler that completes
        // without throwing reaches past it, then flushes `__events` to
        // whatever composed it (`deps.__eventsDispatch`, threaded above).
        // Gated on `block_uses_emit` specifically (not the broader
        // `needs_events_dispatch` above) — a handler that only *forwards*
        // `__eventsDispatch` to an agent it calls has nothing of its own to
        // buffer or flush, so it keeps byte-identical output, mirroring
        // `__exec`'s gate on `block_uses_send`.
        let body_emits_directly = bynk_model::block_uses_emit(&handler.body, &commons.callees);
        // #1361: the events-IIFE wrapper (if any) surrounds the guards and the
        // lowered body. The whole method body is statements, the wrapper an
        // arrow whose body is the guards and the lowered body (1A.ws).
        let mut body = guards;
        body.append(lowered_body);
        let body = if body_emits_directly {
            events_release_body(body)
        } else {
            body
        };
        let body = body.into_stmts();

        // #2280: anchor the method header and prologue to the handler's own
        // `on call` line rather than the previous statement's.
        entries.push(bynk_ts::TsObjectEntry::Method {
            name: kind_name,
            is_async: *ir_effectful,
            generics: Vec::new(),
            params,
            return_type: Some(ts_ty_to_ts_type(*ir_ret, tys)),
            doc: handler.documentation.as_deref().map(str::to_owned),
            inline: false,
            body,
            span: Some(handler.span),
        });
    }
    // #2280: what follows the last handler belongs to the service, not to that
    // handler's final body line.
    let mut stmts = Vec::new();
    let has_doc = s.documentation.is_some();
    if let Some(doc) = s.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let mut object = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
            name: s.name.name.clone(),
            ty: None,
            init: bynk_ts::TsExpr::multiline_object_ending_at(entries, s.span),
        })),
        None,
    );
    // A leading doc comment sits directly above the declaration it documents.
    object.no_blank_before = has_doc;
    stmts.push(object);
    stmts
}

/// v0.15: the TypeScript deps-field type for a `given` capability reference.
/// A local capability uses its bare interface name; a cross-context one is
/// qualified with the providing context's import namespace
/// (`platform_time.Clock`).
///
/// Arc F, slice 4 (#1463): a qualified capability name (`ns.Cap`) has no
/// dedicated `bynk_ts::TsType` shape (no member-access-on-a-type variant in
/// the algebra) — `Named` carrying the whole dotted string is the same
/// honest-opaque representation `workers_env_ty`'s own `ServiceBinding`/
/// `DurableObjectNamespace` entries already use for library-provided names.
pub(super) fn cap_ref_ty(
    c: &bynk_model::CapRefIr,
    info: &bynk_check::resolver::CrossContextInfo,
) -> bynk_ts::TsType {
    match c.context.as_deref().and_then(|p| info.resolve_prefix(p)) {
        Some(consumed) => {
            bynk_ts::TsType::named(format!("{}.{}", qualified_to_ns(&consumed), c.name))
        }
        // v0.17: a bare flattened capability (`consumes U { Cap }`) keeps its
        // interface in the consumed unit's module — qualify the type there.
        None => match info.flattened_caps.get(&c.name) {
            Some(unit) => bynk_ts::TsType::named(format!("{}.{}", qualified_to_ns(unit), c.name)),
            None => bynk_ts::TsType::named(c.name.clone()),
        },
    }
}

/// Events track, slice 0 (spine #936): does any service or agent handler in
/// this commons emit — the same predicate `block_uses_emit` answers per
/// handler body, rolled up to "does the context-level `__eventsDispatch`
/// deps field need to exist at all." Syntactic, like its sibling: a false
/// positive here just adds an unused interface field, not a miscompile.
pub(crate) fn commons_uses_emit(commons: &TypedCommons) -> bool {
    commons.commons.items.iter().any(|item| {
        item.handlers()
            .iter()
            .any(|h| bynk_model::block_uses_emit(&h.body, &commons.callees))
    })
}

/// v0.15: collect the cross-context capabilities a context's **handlers**
/// (service + agent) reference via `given B.Cap`, as `(deps_key,
/// consumed_context)` pairs, deduplicated by key and sorted. These become
/// top-level deps fields (handlers access them as `deps.<key>`). Capabilities
/// used only by a provider are injected into that provider's constructor
/// instead, so they are excluded here.
pub(crate) fn cross_context_caps_used(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<(String, String)> {
    let info = &ctx.cross_context;
    let mut seen: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let givens = commons
        .commons
        .items
        .iter()
        .flat_map(CommonsItem::handlers)
        .map(bynk_model::lower_handler_given_ir)
        .chain(std::iter::once(sibling_agent_givens(commons, ctx)));
    for given in givens {
        for c in given {
            // Events track, slice 0 (spine #936): `Events.emit` is
            // intercepted entirely at the call site (release-at-commit
            // buffering) and never calls through a constructed provider
            // — unlike every other capability, there is no
            // `EventsProvider` for compose to build, so the first-party
            // `Events` must not appear in any context's deps interface.
            if bynk_check::firstparty::resolve(&c.name, &ctx.commons_name, &info.flattened_caps)
                == Some(FirstPartyCap::Events)
            {
                continue;
            }
            if let Some(prefix) = &c.context {
                if let Some(consumed) = info.resolve_prefix(prefix) {
                    seen.entry(c.name.clone()).or_insert(consumed);
                }
            } else if let Some(unit) = info.flattened_caps.get(&c.name) {
                // v0.17: a bare flattened capability is a cross-unit dep too.
                seen.entry(c.name.clone()).or_insert_with(|| unit.clone());
            }
        }
    }
    seen.into_iter().collect()
}

/// v0.15: the set of consumed contexts whose capabilities this context
/// references anywhere (handlers *or* providers), so their namespaces are
/// imported for the capability interface types.
pub(crate) fn cross_context_cap_namespaces(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> std::collections::BTreeSet<String> {
    let info = &ctx.cross_context;
    let mut out = std::collections::BTreeSet::new();
    let mut collect = |given: Vec<bynk_model::CapRefIr>| {
        for c in given {
            if let Some(prefix) = &c.context
                && let Some(consumed) = info.resolve_prefix(prefix)
            {
                out.insert(consumed);
            } else if c.context.is_none()
                // v0.17: a bare flattened capability imports its interface from
                // the consumed unit's module.
                && let Some(unit) = info.flattened_caps.get(&c.name)
            {
                out.insert(unit.clone());
            }
        }
    };
    for item in &commons.commons.items {
        match item {
            CommonsItem::Service(s) => s
                .handlers
                .iter()
                .for_each(|h| collect(bynk_model::lower_handler_given_ir(h))),
            CommonsItem::Agent(a) => a
                .handlers
                .iter()
                .for_each(|h| collect(bynk_model::lower_handler_given_ir(h))),
            CommonsItem::Provider(p) => collect(bynk_model::lower_provider_given_ir(p)),
            // #2912: a function's `given` names its capability in `deps` too.
            CommonsItem::Fn(f) => collect(bynk_model::lower_fn_given_ir(f)),
            CommonsItem::Type(_)
            | CommonsItem::Capability(_)
            | CommonsItem::Actor(_)
            | CommonsItem::Messages(_)
            | CommonsItem::Event(_)
            | CommonsItem::Asset(_)
            | CommonsItem::Embed(_)
            | CommonsItem::Const(_) => {}
        }
    }
    collect(sibling_agent_givens(commons, ctx));
    // S4a (#1966): a derived capability's alias names its base's interface in
    // the base's module.
    out.extend(
        commons
            .commons
            .items
            .iter()
            .filter_map(CommonsItem::as_capability)
            .filter_map(|c| c.derived.as_ref())
            .filter_map(|d| info.flattened_caps.get(&d.base.name).cloned()),
    );
    out
}

/// #1820: the `given` capabilities of every agent this file constructs that
/// a sibling file of its context declares. A handler here forwards its `deps`
/// to that agent (`effective_given`), so its `deps` name them although no
/// declaration in this file does. Empty unless the context is split across
/// files: an agent this file declares is walked with the file's own items.
fn sibling_agent_givens(commons: &TypedCommons, ctx: &EmitProjectCtx) -> Vec<bynk_model::CapRefIr> {
    let declared_here: HashSet<&str> = commons
        .commons
        .items
        .iter()
        .filter_map(CommonsItem::as_agent)
        .map(|a| a.name.name.as_str())
        .collect();
    let constructed: std::collections::BTreeSet<&str> = commons
        .callees
        .values()
        .filter_map(|c| {
            if let bynk_check::checker::Callee::AgentInit(a) = c
                && !declared_here.contains(a.as_str())
            {
                Some(a.as_str())
            } else {
                None
            }
        })
        .collect();
    let mut out = Vec::new();
    for agent in constructed {
        let Some(methods) = ctx.agent_method_givens.get(agent) else {
            continue;
        };
        let mut methods: Vec<_> = methods.iter().collect();
        methods.sort_by_key(|(m, _)| *m);
        for (_, caps) in methods {
            out.extend(caps.iter().cloned());
        }
    }
    out
}

/// #527: a handler's *effective* `given` list — its declared capabilities
/// plus those required by the local-agent methods its body calls (recorded
/// during lowering). Compose always instantiates the full capability union
/// into the runtime deps value, so this widening only brings the deps *type*
/// in line with the value; without it, forwarding `deps` to an agent method
/// with `given` failed `tsc --strict` (and under-documented the dependency).
pub(super) fn effective_given(
    declared: &[bynk_model::CapRefIr],
    cx: &LowerCtx<'_>,
) -> Vec<bynk_model::CapRefIr> {
    let mut out = declared.to_vec();
    let have: HashSet<String> = declared.iter().map(|c| c.name.clone()).collect();
    for (key, cap) in cx.agent_given_caps_used().into_iter().flatten() {
        if !have.contains(key) {
            out.push(cap.clone());
        }
    }
    // Events track, slice 0 (spine #936): `Events.emit` is intercepted
    // entirely at the call site (release-at-commit buffering; see the
    // `Events`/`emit` special case in `lower.rs`) and never calls through a
    // constructed provider — unlike every other capability, there is no
    // `EventsProvider` for compose to build. So `Events` must not appear in
    // `deps` at all, or compose would need to construct a provider that
    // doesn't exist. Filtered the same way #934 distinguishes a genuine
    // first-party `Events` from an unrelated same-named capability.
    out.retain(|c| cx.first_party_cap(&c.name) != Some(FirstPartyCap::Events));
    out
}

/// Review of #1469: returns the bare member list, not `TsType::Object(members)`
/// — every real caller immediately widens it further (`append_deps_field`)
/// before wrapping it as the `deps` parameter's own type, and threading
/// `Vec<TsTypeMember>` end to end lets that widening take `&mut Vec<...>`
/// directly, with no `TsType::Object` match arm (and no panic path for the
/// "found something else" case that can't actually happen) in between.
pub(super) fn build_deps_object_ty_with_surface(
    given: &[bynk_model::CapRefIr],
    cx: &LowerCtx<'_>,
    cross_context: &bynk_check::resolver::CrossContextInfo,
    target: BuildTarget,
) -> Vec<bynk_ts::TsTypeMember> {
    let mut members: Vec<bynk_ts::TsTypeMember> = given
        .iter()
        .map(|c| bynk_ts::TsTypeMember::prop(c.name.clone(), cap_ref_ty(c, cross_context)))
        .collect();
    match target {
        BuildTarget::Bundle => {
            // #1960: an agent method this body calls may itself call a
            // consumed service, reading the `surface` this body hands it —
            // the bundle counterpart of the whole `env` a Workers body
            // threads whenever it constructs an agent.
            if cx.cross_context_used()
                || (cx.agents_instantiated() && has_consumed_service(cross_context))
            {
                members.push(bynk_ts::TsTypeMember::prop(
                    "surface",
                    surface_ty(cross_context),
                ));
            }
        }
        BuildTarget::Workers => {
            // v0.9.2: in workers mode `env` carries both consumed-context
            // Service Bindings and the local agents' Durable Object namespaces.
            // It is threaded into deps whenever the handler makes a
            // cross-context call or instantiates an agent.
            if cx.cross_context_used() || cx.agents_instantiated() {
                let agents = if cx.agents_instantiated() {
                    sorted_local_agents(cx)
                } else {
                    Vec::new()
                };
                members.push(bynk_ts::TsTypeMember::prop(
                    "env",
                    workers_env_ty(&cx.cross_contexts_called(), &agents),
                ));
            }
        }
    }
    members
}

/// Local agent names in this commons, sorted — the DO bindings `env` exposes
/// in workers mode. #1820: every file's of the context (`cx.local_agents`),
/// not only this file's: a handler constructing an agent declared in a
/// sibling file passes the Worker's `env` to that agent's factory.
fn sorted_local_agents(cx: &LowerCtx<'_>) -> Vec<String> {
    let names: std::collections::BTreeSet<String> = cx
        .commons()
        .commons
        .items
        .iter()
        .filter_map(CommonsItem::as_agent)
        .map(|a| a.name.name.clone())
        .chain(cx.local_agents.iter().cloned())
        .collect();
    names.into_iter().collect()
}

/// Workers-mode deps.env shape: the bindings handlers reach
/// ([`crate::emitter::bindings::handler_bindings`], #1950) — one Service
/// Binding per consumed context the handler calls and one Durable Object
/// namespace per local agent.
pub(super) fn workers_env_ty(called: &[String], agents: &[String]) -> bynk_ts::TsType {
    // #1961: only a consumed context this handler **calls**
    // (`lower_workers_cross_context_call`), the one thing a handler reaches
    // through `env`. The Worker binds exactly the contexts some body of it
    // calls (`WorkerBindings`), a superset of any one
    // handler's, so `compose`'s `env` always satisfies this type. A unit
    // consumed for capabilities, types or events only has no binding at all —
    // an adapter is not a Worker, and a context whose services go uncalled
    // need not be bound (binding it made an event subscriber and its
    // publisher bind each other, which no deploy order satisfies).
    let members = crate::emitter::bindings::handler_bindings(called, agents)
        .into_iter()
        .map(|b| {
            let ty = crate::emitter::wrangler::binding_ts_type(&b.source);
            bynk_ts::TsTypeMember::prop(b.name, bynk_ts::TsType::named(ty))
        })
        .collect();
    bynk_ts::TsType::Object(members)
}

/// v0.15: true when at least one consumed context exposes services (and thus
/// a `__makeSurface`). A context may now consume another purely for its
/// capabilities, in which case there is no surface to thread.
fn has_consumed_service(cross_context: &bynk_check::resolver::CrossContextInfo) -> bool {
    cross_context
        .consumed_services
        .values()
        .any(|svcs| !svcs.is_empty())
}

/// Build the TS type for the `surface` field in deps, naming each consumed
/// context by its surface key plus the consumed context's __makeSurface type.
/// Only service-bearing consumed contexts contribute (a capability-only
/// consumed context has no `__makeSurface`).
///
/// Arc F, slice 4 (#1463): `ReturnType<T>` itself is representable generically
/// (`TsType::named_with_args`, review of #1469) — the real gap is its own
/// type argument here, `typeof {ns}.__makeSurface`, whose `typeof` operand
/// isn't an ordinary type at all and has no algebra member of its own.
/// `Named` carries only that opaque operand text now, one level narrower
/// than the whole expression this used to wrap as one string.
fn surface_ty(cross_context: &bynk_check::resolver::CrossContextInfo) -> bynk_ts::TsType {
    let mut entries: Vec<(String, String)> = Vec::new();
    // Use alias if present, else the last segment of the qualified name.
    // Order: stable (sorted) so the diff is deterministic.
    let mut consumed_sorted: Vec<String> = cross_context
        .consumed_services
        .iter()
        .filter(|(_, svcs)| !svcs.is_empty())
        .map(|(q, _)| q.clone())
        .collect();
    consumed_sorted.sort();
    // Reverse lookup: consumed-context qualified name → alias.
    let mut alias_for: HashMap<String, String> = HashMap::new();
    for (alias, target) in &cross_context.aliases {
        alias_for.insert(target.clone(), alias.clone());
    }
    for q in &consumed_sorted {
        let key = alias_for
            .get(q)
            .cloned()
            .unwrap_or_else(|| q.rsplit('.').next().unwrap_or(q.as_str()).to_string());
        let ns = qualified_to_ns(q);
        entries.push((key, format!("typeof {ns}.__makeSurface")));
    }
    bynk_ts::TsType::Object(
        entries
            .into_iter()
            .map(|(k, v)| {
                bynk_ts::TsTypeMember::prop(
                    k,
                    bynk_ts::TsType::named_with_args(
                        "globalThis.ReturnType",
                        vec![bynk_ts::TsType::named(v)],
                    ),
                )
            })
            .collect(),
    )
}

/// #2058: each type name this unit reaches from a consumed *context*, mapped
/// to the namespace the module names that context's types under
/// (`ev_store`). A literal admitted to such a refined type is cast to
/// `ev_store.Code`, not a bare `Code` nothing imports.
pub(crate) fn consumed_type_ns(ctx: &EmitProjectCtx) -> HashMap<String, String> {
    ctx.imported_from_kind
        .iter()
        .filter(|(_, kind)| **kind == UnitKind::Context)
        .filter_map(|(name, _)| {
            let owner = ctx.imported_from.get(name)?;
            Some((name.clone(), qualified_to_ns(owner)))
        })
        .collect()
}

/// Turn a qualified context name (e.g. `commerce.payment`) into the JS
/// namespace ident used in `import * as <ns>` (`commerce_payment`).
pub(crate) fn qualified_to_ns(q: &str) -> String {
    q.replace('.', "_")
}

/// #1759: the identifier a generated **test** module imports a unit as
/// (`import * as __ns_greet from …`). A test module destructures a unit's
/// values out of this namespace (`const { greet } = __ns_greet;`), so the
/// namespace can't take `qualified_to_ns`'s bare name: a context `greet`
/// declaring `fn greet` would shadow its own import and read `greet` in its
/// temporal dead zone. The reserved `__ns_` prefix can't be a user identifier.
/// Every test-module namespace reference goes through this, in `tests_emit`
/// and in lowering that runs `in_test_scaffold()`.
pub fn test_scaffold_ns(q: &str) -> String {
    format!("__ns_{}", qualified_to_ns(q))
}

/// The PascalCase name a context uses for its generated `Deps` interface:
/// `shortener.links` → `ShortenerLinks`.
pub fn context_pascal(name: &str) -> String {
    name.split('.')
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// Emit the per-context `<Ctx>Deps` interface (v0.9.2 §4): the providers the
/// context contributes plus the surfaces of any consumed contexts. Replaces
/// the fragile `Parameters<typeof svc.call>[1]` indexing, which only resolved
/// correctly for single-argument service operations.
/// Arc F slice 3 (#1453): builds a real `bynk_ts::TsDecl::Interface`, the
/// same `readonly`-member shape `emit_agent`'s own state interface already
/// establishes (`emit_agent.rs:4436-4444`) — return contract stays
/// the interface's own name too (used as a plain `TsType::named(deps_name)`
/// type reference by `emit_make_surface`, its one caller); since 1A.32c-1 it
/// returns the declaration as a node, not printed text.
///
/// Two fields stay opaque `TsType::named(...)` text, deliberately, not
/// guessed: `surface`'s type comes from `surface_ty`, itself still
/// `String`-returning (#1449's own bucket-4 residual, out of this slice's
/// scope) — wrapping its output opaquely is the established "carry an
/// unconverted sibling's own text" pattern. `__eventsDispatch`'s function
/// type needs a *named* parameter (`events`); `bynk_ts::TsType::Fn` only
/// supports positional parameter names (`a0`, `a1`, …, confirmed against its
/// own printer tests) — the same real, already-named algebra gap
/// `ts_type_ref_to_ts_type`'s own `TypeRef::Query` arm (`emitter.rs`)
/// resolved by pre-rendering and wrapping as opaque `Named` text rather than
/// widening `TsType::Fn` for one call site.
fn emit_context_deps_interface(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> (bynk_ts::TsStmt, String) {
    let deps_name = format!("__{}Deps", context_pascal(&commons.commons.name.joined()));
    let mut members: Vec<bynk_ts::TsTypeMember> = commons
        .commons
        .items
        .iter()
        .filter_map(CommonsItem::as_capability)
        .map(|c| {
            bynk_ts::TsTypeMember::readonly_prop(
                c.name.name.clone(),
                bynk_ts::TsType::named(c.name.name.clone()),
            )
        })
        .collect();
    // #1820: and the capabilities a sibling file of a split context declares.
    // A service here may call an agent there whose methods are `given` one,
    // and its handler's `deps` then names it (`effective_given`).
    let mut sibling_caps: Vec<&String> = ctx
        .file_decl_index
        .capabilities
        .iter()
        .filter(|(_, path)| **path != ctx.source_path)
        .map(|(cap, _)| cap)
        .collect();
    sibling_caps.sort();
    for cap in sibling_caps {
        members.push(bynk_ts::TsTypeMember::readonly_prop(
            cap.clone(),
            bynk_ts::TsType::named(cap.clone()),
        ));
    }
    // v0.15: cross-context capabilities the context consumes appear in deps,
    // typed against the providing context's namespace.
    for (key, consumed) in cross_context_caps_used(commons, ctx) {
        let ty = bynk_ts::TsType::named(format!("{}.{key}", qualified_to_ns(&consumed)));
        members.push(bynk_ts::TsTypeMember::readonly_prop(key, ty));
    }
    if !ctx.cross_context.consumed_contexts.is_empty() && has_consumed_service(&ctx.cross_context) {
        // Arc F, slice 4 (#1463): `surface_ty` now returns a real `TsType`
        // directly — closes the residual Arc F slice 3 (#1453) named here
        // ("kept as opaque `TsType::named(...)` text... because its own
        // source, `surface_ty`, is still `String`-returning").
        members.push(bynk_ts::TsTypeMember::readonly_prop(
            "surface",
            surface_ty(&ctx.cross_context),
        ));
    }
    // Events track, slice 0 (spine #936): a context with any handler that
    // emits needs `__eventsDispatch` threaded all the way from `__makeSurface`
    // down to that handler's own `deps` param (`emit_service`/`emit_agent`'s
    // matching field) — otherwise `svc.call(args, deps)` inside `__makeSurface`
    // fails to typecheck, since `deps`'s static type there is this interface,
    // not whatever compose actually constructs.
    let is_first_party_events = bynk_check::firstparty::resolve(
        FirstPartyCap::Events.name(),
        &ctx.commons_name,
        &ctx.cross_context.flattened_caps,
    ) == Some(FirstPartyCap::Events);
    if is_first_party_events && commons_uses_emit(commons) {
        members.push(bynk_ts::TsTypeMember::readonly_prop(
            "__eventsDispatch",
            bynk_ts::TsType::named(format!(
                "(events: globalThis.Array<{}>) => globalThis.Promise<void>",
                crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE
            )),
        ));
    }
    // #2683: a surface `on call` handler that calls `within` (or sends with
    // `detach`) forwards `deps` to a handler that requires `__exec`, so the
    // surface's `deps` carry it; the compose root supplies it.
    if commons.commons.items.iter().any(|i| {
        i.as_service().is_some_and(|s| {
            s.handlers
                .iter()
                .any(|h| crate::emitter::call_handler_needs_exec(&commons.exec_plan, h))
        })
    }) {
        members.push(bynk_ts::TsTypeMember::readonly_prop(
            "__exec",
            bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::method(
                "waitUntil",
                vec![bynk_ts::TsParam::new(
                    "promise".to_string(),
                    Some(bynk_ts::TsType::named_with_args(
                        "globalThis.Promise",
                        vec![bynk_ts::TsType::named("unknown")],
                    )),
                )],
                bynk_ts::TsType::named("void"),
            )]),
        ));
    }
    let interface_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Interface {
            name: deps_name.clone(),
            type_params: Vec::new(),
            members,
        })),
        None,
    );
    // #1486: no trailing blank baked in here — the printer's automatic
    // top-level spacing policy supplies the single blank before whatever
    // follows this interface.
    (interface_decl, deps_name)
}

/// v0.54 (#655): whether any of `services` declares an `on call … by c: Caller`
/// handler, whose emitted `deps` carries the calling context's qualified name as
/// its `CallerId` identity (ADR 0092). This is the *single* predicate both
/// bundle caller-identity seams consult — `emit_make_surface` (which adds the
/// `__caller` parameter) and the compose root (which passes the name) — so the
/// two can never disagree on which providers thread a caller. `caller_binder_for`
/// self-guards `HandlerKind::Call`, so no kind filter is needed or wanted here.
pub fn any_service_binds_caller<'a>(
    services: impl IntoIterator<Item = &'a ServiceDecl>,
    actors: &HashMap<String, ActorDecl>,
) -> bool {
    // I3 (#1971): a `call` sum names its caller too (`who`), and in bundle
    // mode every caller is a context.
    services.into_iter().any(|s| {
        s.handlers.iter().any(|h| {
            bynk_check::actors::caller_binder_for(h, actors).is_some()
                || bynk_check::actors::call_admission_for(h, actors)
                    .is_some_and(|a| a.sum.is_some())
        })
    })
}

/// Emit the `__makeSurface(deps)` function for a context that exposes
/// services to other contexts (v0.6 §6.3 / §6.4).
///
/// Arc C, slice 18 (#1364): closes the `emit_make_surface` half of step (8)
/// of the design pass's own decomposition order (#1331) — the "cross-context
/// lowering cluster" the same step also names stays a separate, deferred
/// remainder (a genuinely different, harder shape: real cross-module codec
/// lowering, not top-level structure construction), the same "split when a
/// named remainder is harder" precedent steps (4)/(6) already established.
/// Converts fully — no opaque carve-out: every shape this function needs
/// (`bynk_ts::TsDecl::Function`, `TsObjectEntry::Method`/`Spread`,
/// `TsStmt::Return`, `TsExpr::Call`/`multiline_object_entries`) already
/// exists in `bynk_ts`, and this function never lowers an expression through
/// `LowerCtx` (no `source_map` parameter reaches it), so there is no
/// sub-builder/merge machinery to get right here. `emit_context_deps_interface`
/// converted later (Arc F slice 3, #1453) — it still returns `String` (just
/// the interface's own name, used below as a plain `TsType::named` type
/// reference), but the interface declaration itself is a real
/// `bynk_ts::TsDecl::Interface` now, not `writeln!`-built text.
/// #1478: returns real [`bynk_ts::TsStmt`]s (was `out: &mut String`).
/// `emit_context_deps_interface` returns the interface declaration as a node
/// (1A.32c-1), so it is the surface's first statement.
pub(crate) fn emit_make_surface(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    let services: Vec<&ServiceDecl> = commons
        .commons
        .items
        .iter()
        .filter_map(CommonsItem::as_service)
        .collect();
    if services.is_empty() {
        return Vec::new();
    }
    let (deps_decl, deps_name) = emit_context_deps_interface(commons, ctx);
    let mut stmts = vec![deps_decl];
    // v0.54 (#655): an `on call … by c: Caller` handler reads a live `CallerId`
    // (the calling context's qualified name) threaded through `deps.identity`.
    // In bundle mode the compose root supplies that name to `__makeSurface` as a
    // second `__caller` argument — the analogue of the `X-Bynk-Caller` header a
    // Worker reads at its entry (ADR 0092). Only a context with such a handler
    // takes the extra parameter, so a caller-free surface is byte-unchanged. The
    // shared `any_service_binds_caller` predicate is what keeps this seam and the
    // compose root in lockstep on which providers get the extra argument.
    let binds_caller =
        |h: &Handler| bynk_check::actors::caller_binder_for(h, &ctx.actors).is_some();
    let any_caller = any_service_binds_caller(services.iter().copied(), &ctx.actors);
    let mut params = vec![bynk_ts::TsParam::new(
        "deps".to_string(),
        Some(bynk_ts::TsType::named(deps_name)),
    )];
    if any_caller {
        params.push(bynk_ts::TsParam::new(
            "__caller".to_string(),
            Some(bynk_ts::TsType::named("string")),
        ));
    }
    let mut entries: Vec<bynk_ts::TsObjectEntry> = Vec::new();
    for s in &services {
        // For each handler kind currently only `call`. We bind it as a
        // method on the surface with the deps captured.
        let handler = s
            .handlers
            .iter()
            .find(|h| matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Call));
        let Some(h) = handler else { continue };
        let is_async = is_effectful_return(&h.return_type);
        // #2696: the method body reads `<service>` after the parameters bind, so a
        // user parameter spelled like the service would shadow it. Rename the
        // user's binder (hygienically), never the generated name, so every other
        // surface stays byte-identical. `deps` needs no check here: `ts_ident`
        // already maps it to `__id_deps`. `__caller` needs none either: a
        // `__`-prefixed name can't be a user identifier.
        let surface_param = |name: &str| -> String {
            let id = ts_ident(name);
            if id == s.name.name {
                format!("__id_{id}")
            } else {
                id
            }
        };
        let method_params: Vec<bynk_ts::TsParam> = h
            .params
            .iter()
            .map(|p| {
                bynk_ts::TsParam::new(
                    surface_param(&p.name.name),
                    Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
                )
            })
            .collect();
        let mut call_args: Vec<bynk_ts::TsExpr> = h
            .params
            .iter()
            .map(|p| bynk_ts::TsExpr::Ident(surface_param(&p.name.name)))
            .collect();
        // A Caller-binding handler's `deps.identity` is the caller name the
        // compose root threaded in; every other handler forwards `deps` verbatim.
        //
        // I3 (#1971): in bundle mode every caller is a context, never a
        // foreign caller, so a `call` sum resolves to `Caller` with that name,
        // and a handler that admits only foreign callers is never admitted.
        let admission = bynk_check::actors::call_admission_for(h, &ctx.actors);
        if admission.as_ref().is_some_and(|a| !a.contexts) {
            entries.push(bynk_ts::TsObjectEntry::Method {
                name: s.name.name.clone(),
                is_async,
                generics: Vec::new(),
                params: method_params,
                return_type: Some(ts_type_ref_to_ts_type(&h.return_type, None)),
                doc: None,
                inline: false,
                body: vec![bynk_ts::TsStmt::throw_stmt(
                    bynk_ts::TsExpr::New {
                        callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.Error".to_string())),
                        args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(format!(
                            "`{}` admits only foreign callers, and a context is not one",
                            s.name.name
                        )))],
                    },
                    None,
                )],
                span: None,
            });
            continue;
        }
        let internal_tag = admission
            .as_ref()
            .map_or_else(|| "Caller".to_string(), |a| a.internal_tag().to_string());
        let deps_arg = if admission.is_some_and(|a| a.sum.is_some()) {
            bynk_ts::TsExpr::object_entries(vec![
                bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident("deps".to_string())),
                bynk_ts::TsObjectEntry::prop(
                    "who".to_string(),
                    bynk_ts::TsExpr::object(vec![
                        (
                            "tag".to_string(),
                            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(internal_tag)),
                        ),
                        (
                            "identity".to_string(),
                            bynk_ts::TsExpr::Ident("__caller".to_string()),
                        ),
                    ]),
                ),
            ])
        } else if binds_caller(h) {
            bynk_ts::TsExpr::object_entries(vec![
                bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident("deps".to_string())),
                bynk_ts::TsObjectEntry::prop(
                    "identity".to_string(),
                    bynk_ts::TsExpr::Ident("__caller".to_string()),
                ),
            ])
        } else {
            bynk_ts::TsExpr::Ident("deps".to_string())
        };
        call_args.push(deps_arg);
        let call = bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident(s.name.name.clone())),
                property: "call".to_string(),
            }),
            args: call_args,
        };
        entries.push(bynk_ts::TsObjectEntry::Method {
            name: s.name.name.clone(),
            is_async,
            generics: Vec::new(),
            params: method_params,
            return_type: Some(ts_type_ref_to_ts_type(&h.return_type, None)),
            doc: None,
            inline: false,
            body: vec![bynk_ts::TsStmt::return_stmt(Some(call), None)],
            span: None,
        });
    }
    let func_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
            name: "__makeSurface".to_string(),
            generics: Vec::new(),
            params,
            return_type: None,
            body: vec![bynk_ts::TsStmt::return_stmt(
                Some(bynk_ts::TsExpr::multiline_object_entries(entries)),
                None,
            )],
            is_async: false,
            inline: false,
        })),
        None,
    );
    stmts.push(func_decl);
    // #1486: no explicit trailing blank stmt here — the printer's automatic
    // top-level spacing policy supplies the single blank before whatever
    // follows (never import-kind at this function's own call site, so
    // never exempted from it).
    stmts
}

pub(crate) fn flatten_emit_ident_chain(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(id) => Some(id.name.clone()),
        ExprKind::FieldAccess { receiver, field } => {
            let head = flatten_emit_ident_chain(receiver)?;
            Some(format!("{head}.{}", field.name))
        }
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::DecimalLit { .. }
        | ExprKind::DurationLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::InterpStr(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Call { .. }
        | ExprKind::Lambda(_)
        | ExprKind::BinOp(..)
        | ExprKind::UnaryOp(..)
        | ExprKind::Paren(_)
        | ExprKind::Block(_)
        | ExprKind::If { .. }
        | ExprKind::Ok(_)
        | ExprKind::Err(_)
        | ExprKind::Question(_)
        | ExprKind::RecordConstruction { .. }
        | ExprKind::MethodCall { .. }
        | ExprKind::Match { .. }
        | ExprKind::Is { .. }
        | ExprKind::Some(_)
        | ExprKind::None
        | ExprKind::UnitLit
        | ExprKind::RecordSpread { .. }
        | ExprKind::EffectPure(_)
        | ExprKind::Expect(_)
        | ExprKind::Val { .. }
        | ExprKind::Wire(_)
        | ExprKind::ListLit(..)
        | ExprKind::Observation(_)
        | ExprKind::Faults(_)
        | ExprKind::Trace { .. } => None,
    }
}

/// The TS type of a resolved actor sum (`deps.who`): each member's tagged
/// shape, `{ tag: "User", identity: UserId }` or `{ tag: "Visitor" }`, joined
/// as a union. The HTTP wrapper builds this value and the WebSocket Durable
/// Object rebuilds it, so both read the one shape.
pub(super) fn actor_sum_ts_union(members: &[bynk_check::actors::SumMember]) -> bynk_ts::TsType {
    bynk_ts::TsType::union(
        members
            .iter()
            .map(|m| match m.identity_type() {
                Some(id) => tag_member(&m.actor_name, Some(bynk_ts::TsType::named(id))),
                None => tag_member(&m.actor_name, None),
            })
            .collect(),
    )
}

/// One member of an actor sum: `{ tag: "<name>"; identity: <ty> }`, or `{ tag: "<name>" }`.
fn tag_member(tag: &str, identity: Option<bynk_ts::TsType>) -> bynk_ts::TsType {
    let mut props = vec![bynk_ts::TsTypeMember::prop(
        "tag",
        bynk_ts::TsType::string_literal(tag),
    )];
    props.extend(identity.map(|id| bynk_ts::TsTypeMember::prop("identity", id)));
    bynk_ts::TsType::Object(props)
}

/// I3 (#1971): the TypeScript type of a `call` handler's resolved actor sum
/// (`deps.who`): `Caller` carries the calling context's name, a foreign
/// caller is its actor alone. A `TsType` node, as the HTTP sum's union is.
pub(crate) fn call_sum_ts_type(members: &[bynk_check::actors::CallSumMember]) -> bynk_ts::TsType {
    bynk_ts::TsType::union(
        members
            .iter()
            .map(|m| match m {
                bynk_check::actors::CallSumMember::Caller => {
                    tag_member("Caller", Some(bynk_ts::TsType::named("string")))
                }
                bynk_check::actors::CallSumMember::Foreign(f) => tag_member(&f.actor, None),
                bynk_check::actors::CallSumMember::Refined(name, _) => {
                    tag_member(name, Some(bynk_ts::TsType::named("string")))
                }
            })
            .collect(),
    )
}
