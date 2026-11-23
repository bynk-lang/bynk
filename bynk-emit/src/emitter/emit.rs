//! Per-declaration emission — the functions `emit_project` drives to render
//! each top-level Bynk declaration into TypeScript. Split out of `emitter.rs`
//! (ADR 0060); the codec/reference/import/header helpers and the
//! `ts_*`/`LowerCtx` core stay in the parent and are reached via `use super::*`.
//!
//! The declaration kinds that have moved into their own files (back-half
//! paydown 2.1, #4086) are re-exported here, so every `emitter::emit::X` and
//! `emitter::X` path still resolves:
//!
//! - `types`: type, refined, record and sum declarations and their checks;
//! - `functions`: attached methods, free functions, constants and contract guards;
//! - `handlers`: handler method names and labels, and the handler-body items services, agents and
//!   free functions share;
//! - `messages`: `messages` bundles and ICU placeholders;
//! - `capability`: capabilities, derived capabilities and providers;
//! - `service`: services, contexts, the `make` surface and the actor-sum types;
//! - `websocket`: the Durable Object methods and `fetch` upgrade branch of a `from websocket`
//!   service;
//! - `agent_store`: an agent's store layout, lazy-read specs, and key and wire codecs;
//! - `agent_migrate`: the `migrate from N` step functions and their dispatcher;
//! - `agent_state`: the `loadState` and `commitState` functions, hydration and the persistence guards;
//! - `agent_history`: the history-property driver.
//!
//! What has not moved yet — the agent class, dispatch and handlers, and the suite declarations — still
//! lives below.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::internal_fault::FaultSite;
use super::zero_value::zero_expr as zero_value_expr;
use crate::emit_model::EmitProjectCtx;
use bynk_check::checker::{TyId, TypedCommons};
use bynk_check::firstparty::FirstPartyCap;
use bynk_syntax::ast::{
    AgentDecl, ConstDecl, Expr, FnDecl, RecordField, Refinement, TypeDecl, TypeRef,
};

use bynk_model::{IrHandlerKind, StoreFieldIr, StoreKindIr};
use bynk_model::{body_writes_state, is_effectful_return, lower_handler_kind_ir};

use super::ctx::{AgentStoreState, BodyMode, HandlerShared, ModuleCtx};
use super::lower_ctx::LowerCtx;
use super::lower_stmt::{Body, Item, block_body_with_return, claim_scoped};
use super::ts_types::{ts_ty_to_ts_type, ts_type_ref_to_ts_type};
use super::*;
use crate::ts_build::{call, ident, member, str_lit};

mod agent_history;
mod agent_migrate;
mod agent_state;
mod agent_store;
mod capability;
mod functions;
mod handlers;
mod messages;
mod service;
mod types;
mod websocket;

use agent_history::*;
use agent_migrate::*;
use agent_state::*;
pub(crate) use agent_store::*;
pub(crate) use capability::*;
pub(crate) use functions::*;
pub use handlers::*;
pub(crate) use messages::*;
pub use service::*;
pub(crate) use types::*;
pub(crate) use websocket::*;

/// A state field's zero value. The checker admits a field with no initialiser
/// only when its type has a zero (`bynk.agents.non_zeroable_state_field`), so none is a missing
/// fact (1A.34), reported at the field.
fn state_field_zero(
    f: &RecordField,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> bynk_ts::TsExpr {
    zero_value_ts(&f.type_ref, f.refinement.as_ref(), &commons.types).unwrap_or_else(|| {
        let site = FaultSite {
            sink: &ctx.runtime_use,
            span: f.span,
        };
        site.missing_fact("zero value for a state field")
    })
}

/// The TypeScript zero-value expression for a state field, or `None` if its
/// type is not zeroable (#3165: the checker decides, `zero_value` renders).
fn zero_value_ts(
    type_ref: &TypeRef,
    inline: Option<&Refinement>,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<bynk_ts::TsExpr> {
    bynk_check::checker::zero_value(type_ref, inline, types).map(|z| zero_value_expr(&z))
}

/// #2834: a suite-local helper `fn` — `emit_free_fn` without the `export`,
/// since it is declared inside a test runner and called from its body.
pub fn emit_suite_fn(
    f: &FnDecl,
    commons: &TypedCommons,
    runtime_use: &RuntimeUse,
) -> Vec<bynk_ts::TsStmt> {
    // A suite is only ever built for `bynk test`, which guards every contract.
    // A suite helper cannot declare `given`, so it has no capability types to name.
    let empty = bynk_check::resolver::CrossContextInfo::default();
    emit_free_fn_in(
        f,
        commons,
        true,
        runtime_use,
        &empty,
        false,
        true,
        UnitFacts::detached(),
    )
}

// -- v0.5 emission --

/// An agent class's handler methods (#3167): one per `on` handler, each lowered
/// to a class method and written onto `out`, in declaration order.
fn emit_agent_handlers(
    methods: &mut Vec<bynk_ts::TsClassMethod>,
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) {
    let is_store_agent = true;
    let AgentStoreLayout {
        effective_fields,
        held_maps_ts,
        map_names,
        map_value_ts,
        store_map_indexes,
        set_names,
        cache_ttls,
        log_retains,
        log_value_ts,
        ..
    } = layout;
    // 3) Handlers.
    let cell_names: HashSet<String> = effective_fields
        .iter()
        .map(|f| f.name.name.clone())
        .collect();
    for h in &a.handlers {
        // Arc C, slice 23 (#1375): closes step (9)'s own fourth sub-slice —
        // each handler converts to a real `bynk_ts::TsClassMethod` fragment,
        // the same "params/return-type real, whole body one opaque
        // `TsStmt::Raw`, two-level offset merge" shape `emit_service`
        // (#1361) already established for its own near-identical prologue/
        // body/epilogue wrapping — printed through `print_class_method`
        // (#1359) instead of `print_object_entry`, since a handler here is
        // a CLASS method, not an object-literal entry. The old standalone
        // `emit_doc_block` call is removed — `doc` on the method now
        // carries it, the same doc-duplication bug #1361's own review
        // caught fixed proactively here rather than rediscovered.
        let mut params: Vec<bynk_ts::TsParam> = h
            .params
            .iter()
            .map(|p| {
                bynk_ts::TsParam::new(
                    ts_ident(&p.name.name),
                    Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
                )
            })
            .collect();
        // Lower body into a buffer so we can detect cross-context usage and
        // shape the deps type accordingly.
        // v0.81: a store-agent handler reads/writes cells over a mutable working
        // record `__state`; a state-record handler uses `currentState`/`self.state`.
        // A store handler that performs any `:=` wraps its body in a closure so an
        // implicit commit runs at handler end on every (success) return path.
        // #1196/R6.5: write detection reads the checker's own resolved
        // `Callee::Store` classification (`bynk_model::body_writes_state`)
        // rather than matching a bare-identifier receiver name against this
        // agent's own field-name sets — a locally-shadowed field name (a
        // handler param, say) can no longer false-positive into an
        // unnecessary implicit-commit wrapper the way the deleted
        // `block_writes_state` could (`1196_agent_write_detection_via_
        // resolved_callee`'s own fixture pins the fix). A held `Map[K,
        // Connection]`'s own `put`/`remove` still triggers the commit flush
        // (v0.105, slice 3b-ii) with no extra name-set union needed here —
        // a held map is an ordinary `store Map` field as far as the checker's
        // own `Callee::Store` resolution is concerned, "held" being purely
        // this emitter's own downstream connection-resolution concern.
        let writes_state = is_store_agent && body_writes_state(&h.body, commons);
        let store = is_store_agent.then(|| {
            Box::new(AgentStoreState {
                state: ("__state".to_string(), cell_names.clone()),
                maps: map_names.clone(),
                sets: set_names.clone(),
                caches: cache_ttls.clone(),
                logs: log_retains.clone(),
                indexes: store_map_indexes.clone(),
                held_maps: held_maps_ts.clone(),
                map_values: map_value_ts.clone(),
                connection_maps: layout
                    .state
                    .iter()
                    .filter(|f| match &f.kind {
                        StoreKindIr::Map(_, v) => {
                            matches!(*commons.tys().get(*v), Ty::Connection(_))
                        }
                        StoreKindIr::Cell(_)
                        | StoreKindIr::Set(_)
                        | StoreKindIr::Cache(..)
                        | StoreKindIr::Log(..) => false,
                    })
                    .map(|f| f.field.clone())
                    .collect(),
                log_values: log_value_ts.clone(),
            })
        });
        // #934: mirrors the `method` name resolved below (all non-`method_name`
        // agent handler kinds resolve to `"call"`), computed here so it's
        // available before the body lowers.
        let scope_method = h
            .method_name
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| "call".to_string());
        let mut module = ModuleCtx::for_unit(commons, ctx);
        module.note_unit(ctx);
        // #1960: the body lowers for the build target, as a service body
        // does, so a cross-context call reads the Service Binding its `deps`
        // type names.
        module.target = ctx.target;
        module.agent_method_givens = ctx.agent_method_givens.clone();
        module.agent_called_contexts = ctx.agent_called_contexts.clone();
        module.event_schema_versions = ctx.event_schema_versions.clone();
        module.consumed_type_ns = consumed_type_ns(ctx);
        module.consumed_type_declared = ctx.consumed_type_aliases.clone();
        module.consumed_context_types = ctx.consumed_context_types();
        let mut cx = LowerCtx::new(
            module,
            BodyMode::AgentHandler {
                handler: HandlerShared {
                    capabilities: bynk_model::lower_handler_given_ir(h)
                        .into_iter()
                        .map(|c| c.name)
                        .collect::<HashSet<_>>(),
                    handler_scope: Some(format!(
                        "{}.{}.{}",
                        ctx.commons_name, a.name.name, scope_method
                    )),
                    owning_context: ctx.commons_name.clone(),
                    ..HandlerShared::default()
                },
                in_agent_handler: true,
                agent_key_field: Some(a.key_name.name.clone()),
                store,
            },
        );
        cx.local_agents = ctx.local_agents.clone();
        // A handler param is emitted at its natural `ts_ident` name (above), so
        // it must be declared into scope the same way: otherwise a param that
        // shares a name with a `store Map`/`Set`/`Cache`/`Log` field is
        // invisible to `LowerCtx::is_local`, and the store-field dispatch
        // below silently wins over the parameter.
        for p in &h.params {
            cx.declare_binder(&p.name.name);
        }
        // P6.54 (design/tracks/the-ir.md §6b): computed once, not twice —
        // `async_kw` below used to call `is_effectful_return(&h.return_type)`
        // again for the identical value.
        let effectful = is_effectful_return(&h.return_type);
        let async_tail = effectful;
        // A writing store handler's body sits one level deeper, inside the
        // implicit-commit closure.
        let lowered_body =
            block_body_with_return(&h.body, &mut cx, async_tail, Some(&h.return_type));
        let mut deps_members = build_deps_object_ty_with_surface(
            &effective_given(&bynk_model::lower_handler_given_ir(h), &cx),
            &cx,
            &ctx.cross_context,
            ctx.target,
        );
        // Events track, slice 0 (spine #936): see the matching field on the
        // service path (`emit_service`) — an agent handler that emits, or
        // that calls another local agent method which itself emits, gets
        // the same compose-supplied `__eventsDispatch` callback.
        let needs_events_dispatch = cx.is_first_party(FirstPartyCap::Events)
            && (bynk_model::block_uses_emit(&h.body, &commons.callees)
                || cx
                    .agent_given_caps_used()
                    .is_some_and(|m| m.contains_key("Events")));
        if needs_events_dispatch {
            // Arc F, slice 4 (#1463): stays opaque `Named` text — the same
            // `TsType::Fn`-has-no-named-params gap `emit_service`'s own
            // identical field already names.
            append_deps_field(
                &mut deps_members,
                "__eventsDispatch",
                bynk_ts::TsType::named(format!(
                    "(events: globalThis.Array<{}>) => globalThis.Promise<void>",
                    crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE
                )),
            );
        }
        params.push(bynk_ts::TsParam::new(
            "deps".to_string(),
            Some(bynk_ts::TsType::Object(deps_members)),
        ));
        let ret = ts_type_ref_to_ts_type(&h.return_type, None);
        let method = h
            .method_name
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| match lower_handler_kind_ir(&h.kind) {
                IrHandlerKind::Call => "call".to_string(),
                // HTTP/cron/queue/open handlers are service-only (rejected in
                // agents by the parser); these arms are defensive and unreachable
                // here.
                IrHandlerKind::Http { .. }
                | IrHandlerKind::Cron { .. }
                | IrHandlerKind::Message
                | IrHandlerKind::Open
                | IrHandlerKind::Close
                | IrHandlerKind::Event
                | IrHandlerKind::Rejected => "call".to_string(),
            });
        // Load state at entry. A state-record handler binds `currentState` and
        // commits explicitly via `commit`. A store handler binds a mutable
        // working record `__state`; reads/writes go through it, and (if it writes)
        // the body is wrapped so `commitState` runs once at handler end — the
        // implicit, atomic commit (ADR 0109). A fault before that flush persists
        // nothing; the invariant gate inside `commitState` runs before the write.
        //
        // 1A.15: the whole prologue + body + epilogue is statements, which the
        // printer records the checkpoints of (it was one raw text blob with
        // a source map of its own, merged at a computed offset).
        let body_emits_directly = bynk_model::block_uses_emit(&h.body, &commons.callees);
        let mut body = assemble_agent_body(
            lowered_body,
            is_store_agent,
            writes_state,
            body_emits_directly,
            history_handler(ctx, &scope_method),
        );
        // #3465: an agent handler that claimed settles its claims after the state
        // commit and the events flush, so a failure of either releases them.
        if cx.claim_scope_used {
            body = claim_scoped(body);
        }
        // #2270: anchor the method header and prologue to the handler's own
        // `on call` line. Without this they inherit the previous checkpoint,
        // so an uncalled handler's header leaked onto the preceding body's
        // last line in coverage.
        methods.push(bynk_ts::TsClassMethod {
            doc: h.documentation.as_deref().map(str::to_owned),
            span: Some(h.span),
            ..bynk_ts::TsClassMethod::new(method, effectful, params, Some(ret), body.into_stmts())
        });
    }
}

/// #3581: in a `bynk dev` build the handler's name rides with its commit, so the
/// instance's history entry names it. `None` for every other build.
fn history_handler<'a>(ctx: &EmitProjectCtx, handler: &'a str) -> Option<&'a str> {
    ctx.inspect_agents.then_some(handler)
}

/// `await this.<method>(<args>)`.
fn this_await(method: &str, args: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
            property: method.to_string(),
        }),
        args,
    }))
}

/// One agent handler method's prologue + lowered body + epilogue (#3167). A
/// store handler that writes wraps its body in the implicit-commit closure; one
/// that emits events directly flushes them after it. The body items are
/// `lowered`; when the closure wraps them they sit inside its arrow body.
fn assemble_agent_body(
    lowered: Body,
    is_store_agent: bool,
    writes_state: bool,
    body_emits_directly: bool,
    history_handler: Option<&str>,
) -> Body {
    let mut body = Body::new();
    let ident = |name: &str| bynk_ts::TsBindingName::Ident(name.to_string());
    let state_decl = if !is_store_agent {
        bynk_ts::TsStmt::const_stmt(
            ident("currentState"),
            None,
            this_await("loadState", Vec::new()),
            None,
        )
    } else if writes_state {
        bynk_ts::TsStmt::const_stmt(
            ident("__state"),
            None,
            bynk_ts::TsExpr::object_entries(vec![bynk_ts::TsObjectEntry::Spread(
                bynk_ts::TsExpr::Paren(Box::new(this_await("loadState", Vec::new()))),
            )]),
            None,
        )
    } else {
        bynk_ts::TsStmt::const_stmt(
            ident("__state"),
            None,
            this_await("loadState", Vec::new()),
            None,
        )
    };
    body.push(Item::Node(Box::new(state_decl)));
    // Only a handler that wraps its body (a commit, or direct events) needs the closure.
    if !writes_state && !body_emits_directly {
        body.append(lowered);
        return body;
    }
    if body_emits_directly {
        body.push(events_decl_item());
    }
    // The body sits inside the closure; the printer indents it.
    body.push(result_closure_item(lowered));
    if writes_state {
        let mut args = vec![bynk_ts::TsExpr::Ident("__state".to_string())];
        if let Some(h) = history_handler {
            args.push(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(h.to_string())));
        }
        body.push(Item::Node(Box::new(bynk_ts::TsStmt::expr_stmt(
            this_await("commitState", args),
            None,
        ))));
    }
    if body_emits_directly {
        body.push(events_flush_item(true));
    }
    body.push(return_result_item());
    body
}

/// What an agent class's dispatch methods depend on, decided while the class
/// header was written (#3167).
struct AgentDispatchFacts<'a> {
    /// The handler reads the agent's key (`self.key`).
    reads_key: bool,
    /// The `given` capability dependencies the RPC branch rebuilds, if any.
    given_deps_expr: &'a Option<GivenDeps>,
    /// Whether the RPC branch rebuilds the agent's environment at all.
    rpc_rebuilds_env: bool,
    /// Whether a handler body emits events directly.
    agent_uses_emit: bool,
    /// Whether a handler body reaches the Worker's own `env`.
    agent_reaches_env: bool,
    /// The agent's declared schema versions, if it has any.
    agent_versions: &'a Option<bynk_check::schema_registry::AgentVersions>,
}

/// The tail of an agent class (#3167): the Workers `fetch` dispatch that turns
/// an RPC request into a method call, the dispatch for each `from websocket`
/// `on open` host, and the class's closing brace. Written onto `out`.
fn emit_agent_dispatch(
    methods: &mut Vec<bynk_ts::TsClassMethod>,
    a: &AgentDecl,
    facts: &AgentDispatchFacts,
    ws_open_hosts: &[WsOpenHost<'_>],
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) {
    let tys = UnitFacts::of(ctx).render(commons);
    let AgentDispatchFacts {
        reads_key,
        given_deps_expr,
        rpc_rebuilds_env,
        agent_uses_emit,
        agent_reaches_env,
        agent_versions,
    } = *facts;
    // v0.9.2: workers-mode DO dispatch. Method calls arrive as `fetch` requests
    // under `/_bynk/agent/<method>`; decode `{ args, deps }`, invoke the
    // handler with deps as the trailing argument, and serialise the result.
    if matches!(ctx.target, BuildTarget::Workers) {
        // Arc C, slice 27 (#1384): the third and final of step (9) sub-slice
        // (5)'s own 3 independent slices — converts to a real
        // `bynk_ts::TsClassMethod`, printed through `print_class_method`, the
        // same zero-`LowerCtx`/no-merge shape #1382 (`emit_ws_dispatch_
        // handlers`) established for this cluster's other Rust-side-only
        // emitter. `...args` stays one opaque `TsExpr::Ident` call argument (the
        // spread-call-argument shape the grounding pass DID predict for this
        // slice, #1382's own PR confirming it belongs here, not there).
        let mut fetch_stmts = vec![bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("url".to_string()),
            None,
            bynk_ts::TsExpr::New {
                callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.URL".to_string())),
                args: vec![bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("request".to_string())),
                    property: "url".to_string(),
                }],
            },
            None,
        )];
        // v0.104 (slice 3b): a forwarded WebSocket upgrade. The edge has already
        // authenticated the actor (the body never runs unverified); accept the
        // socket here, run the on-open body, and return the `101` carrying the
        // client end. The verified identity and route arguments ride in a trusted
        // internal header (the DO is only reachable through the Worker).
        for host in ws_open_hosts {
            fetch_stmts.push(ws_open_fetch_branch_stmt(host, tys));
        }
        // State-migrations S4 (#2259 Decision D): `bynk migrate sweep`'s wake,
        // a branch of its own before the method dispatch. It carries no key
        // (steps are key-free), loads the state (running any step), and
        // commits only a record older than this build's version. Reachable
        // only through this Worker's signed `/_bynk/migrate/` route.
        fetch_stmts.extend(wake_and_inspect_branches(a, agent_versions, ctx));
        // `(this as unknown as Record<string, (...bynkArgs: unknown[]) => unknown>)[methodName]`:
        // the cast chain prints bare as `Index`'s receiver and the printer brackets it.
        let dispatch_callee = crate::ts_build::index(
            record_cast(
                bynk_ts::TsExpr::Ident("this".to_string()),
                bynk_ts::TsType::Fn {
                    params: vec![bynk_ts::TsType::array(bynk_ts::TsType::named("unknown"))],
                    names: vec!["bynkArgs".to_string()],
                    rest: true,
                    ret: Box::new(bynk_ts::TsType::named("unknown")),
                },
            ),
            bynk_ts::TsExpr::Ident("methodName".to_string()),
        );
        let mut agent_dispatch_stmts = vec![
            bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::Ident("methodName".to_string()),
                None,
                bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Member {
                            object: Box::new(bynk_ts::TsExpr::Ident("url".to_string())),
                            property: "pathname".to_string(),
                        }),
                        property: "slice".to_string(),
                    }),
                    args: vec![bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            "/_bynk/agent/".to_string(),
                        ))),
                        property: "length".to_string(),
                    }],
                },
                None,
            ),
            bynk_ts::TsStmt::const_stmt(
                bynk_ts::TsBindingName::ObjectPattern(if reads_key {
                    vec!["args".to_string(), "deps".to_string(), "key".to_string()]
                } else {
                    vec!["args".to_string(), "deps".to_string()]
                }),
                None,
                bynk_ts::TsExpr::As {
                    expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Await(
                        Box::new(bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Member {
                                object: Box::new(bynk_ts::TsExpr::Ident("request".to_string())),
                                property: "json".to_string(),
                            }),
                            args: Vec::new(),
                        }),
                    )))),
                    ty: bynk_ts::TsType::Object(
                        vec![
                            bynk_ts::TsTypeMember::prop(
                                "args",
                                bynk_ts::TsType::array(bynk_ts::TsType::named("unknown")),
                            ),
                            bynk_ts::TsTypeMember::prop("deps", bynk_ts::TsType::named("unknown")),
                        ]
                        .into_iter()
                        .chain(reads_key.then(|| {
                            bynk_ts::TsTypeMember::prop("key", bynk_ts::TsType::named("unknown"))
                        }))
                        .collect(),
                    ),
                },
                None,
            ),
        ];
        // #1818: an agent that reads its key learns it from the call, decoded
        // by its key codec, before the handler runs.
        if reads_key {
            agent_dispatch_stmts.push(agent_key_assign(
                &agent_key_codec_name(&a.name.name),
                &ts_type_ref_to_ts_type(&a.key_type, None),
                ident("key"),
            ));
        }
        // #1678: the wire arguments are decoded through this agent's table
        // before the handler sees them (`decodeAgentArgs` throws on a decode
        // failure, an internal fault), and the result is encoded on the way out.
        let decoded_args = bynk_ts::TsExpr::Spread(Box::new(call(
            ident("__decodeAgentArgs"),
            vec![
                ident(agent_wire_name(&a.name.name)),
                ident("methodName"),
                ident("args"),
            ],
        )));
        let result_expr = if rpc_rebuilds_env {
            // #527: the wire deps are JSON — any capability provider in them
            // is a dead plain object (its methods did not survive
            // serialisation). Rebuild this agent's `given` deps in-process,
            // exactly as compose wires them, and let them win the merge.
            //
            // Events track, slice 0: `deps.__eventsDispatch` is a function
            // too, and does not survive the wire any better than a provider
            // — rebuilt from this Worker's own `env.EVENTS_FANOUT` binding
            // (shared by every DO class this script hosts) rather than
            // trusting whatever the caller's JSON carried.
            // P7.2: matches `emitter/workers.rs`'s own established idiom for a
            // multi-binding env whose exact accessed keys vary by call site
            // (`given_deps_expr` may reference other bindings besides
            // `EVENTS_FANOUT`, computed elsewhere and not fully traced here) —
            // a generic string-keyed record, not a precise structural type.
            //
            agent_dispatch_stmts.push(agent_env_const_stmt());
            let mut entries = vec![bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Paren(
                Box::new(bynk_ts::TsExpr::As {
                    expr: Box::new(bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::NullishCoalescing,
                        left: Box::new(bynk_ts::TsExpr::Ident("deps".to_string())),
                        right: Box::new(bynk_ts::TsExpr::object(Vec::new())),
                    }),
                    ty: bynk_ts::TsType::named_with_args(
                        "globalThis.Record",
                        vec![
                            bynk_ts::TsType::named("string"),
                            bynk_ts::TsType::named("unknown"),
                        ],
                    ),
                }),
            ))];
            if let Some(expr) = &given_deps_expr {
                agent_dispatch_stmts.push(given_deps_const_stmt(expr));
                entries.push(bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(
                    "__givenDeps".to_string(),
                )));
            }
            if agent_uses_emit {
                agent_dispatch_stmts.push(events_deps_const_stmt());
                entries.push(bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(
                    "__eventsDeps".to_string(),
                )));
            }
            if agent_reaches_env {
                entries.push(bynk_ts::TsObjectEntry::Shorthand("env".to_string()));
            }
            // P7.2: `methodName` is read from `url.pathname` at runtime, so no
            // static type can name which handler this is — a callable
            // dictionary is what's actually known, and `result` flows only into
            // a generic `JSON.stringify` below, so `unknown` costs nothing there.
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(dispatch_callee),
                args: vec![
                    decoded_args.clone(),
                    bynk_ts::TsExpr::object_entries(entries),
                ],
            }))
        } else {
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(dispatch_callee),
                args: vec![decoded_args, bynk_ts::TsExpr::Ident("deps".to_string())],
            }))
        };
        agent_dispatch_stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("result".to_string()),
            None,
            result_expr,
            None,
        ));
        // #1678: the result is encoded through the wire table. A void method's
        // `()` codec encodes `null`; before #1678 `?? null` did that here,
        // since `JSON.stringify(undefined)` is the *string* `undefined` — not
        // JSON — which the calling proxy's `response.json()` rejects (#527).
        agent_dispatch_stmts.push(bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::New {
                callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.Response".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Member {
                            object: Box::new(bynk_ts::TsExpr::Ident("globalThis.JSON".to_string())),
                            property: "stringify".to_string(),
                        }),
                        args: vec![bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Ident(
                                "__encodeAgentResult".to_string(),
                            )),
                            args: vec![
                                bynk_ts::TsExpr::Ident(agent_wire_name(&a.name.name)),
                                bynk_ts::TsExpr::Ident("methodName".to_string()),
                                bynk_ts::TsExpr::Ident("result".to_string()),
                            ],
                        }],
                    },
                    bynk_ts::TsExpr::object(vec![(
                        "headers".to_string(),
                        bynk_ts::TsExpr::object(vec![(
                            "\"content-type\"".to_string(),
                            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                "application/json".to_string(),
                            )),
                        )]),
                    )]),
                ],
            }),
            None,
        ));
        fetch_stmts.push(bynk_ts::TsStmt::if_stmt(
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("url".to_string())),
                        property: "pathname".to_string(),
                    }),
                    property: "startsWith".to_string(),
                }),
                args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    "/_bynk/agent/".to_string(),
                ))],
            },
            bynk_ts::TsStmt::block(agent_dispatch_stmts, None),
            None,
        ));
        fetch_stmts.push(bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::New {
                callee: Box::new(bynk_ts::TsExpr::Ident("globalThis.Response".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str("Not Found".to_string())),
                    bynk_ts::TsExpr::object(vec![(
                        "status".to_string(),
                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("404".to_string())),
                    )]),
                ],
            }),
            None,
        ));
        // #2280: the dispatch methods belong to the agent, not to the last
        // hosted lifecycle method's final body line.
        let fetch_method = bynk_ts::TsClassMethod::new(
            "fetch",
            true,
            vec![bynk_ts::TsParam::new(
                "request".to_string(),
                Some(bynk_ts::TsType::named("globalThis.Request")),
            )],
            Some(bynk_ts::TsType::named_with_args(
                "globalThis.Promise",
                vec![bynk_ts::TsType::named("globalThis.Response")],
            )),
            fetch_stmts,
        );
        methods.push(bynk_ts::TsClassMethod {
            span: Some(a.span),
            ..fetch_method
        });
        // v0.106 (slice 3b-iii): the inbound/close dispatch — Cloudflare calls
        // `webSocketMessage`/`webSocketClose` on a hibernatable socket. Decode the
        // frame against `in:` (reject-and-close on failure), recover the sender
        // identity + route args from the socket attachment, and run the body.
        for host in ws_open_hosts {
            emit_ws_dispatch_handlers(methods, host, &ctx.runtime_use, &commons.types, tys);
        }
    }
}

/// #3581: `__noteAgentInstance(env?.__BYNK_INSPECT_REGISTRY, env?.<binding>,
/// "<Agent>", key, <key codec>, <make>)`: the factory's agent, recorded in the
/// Worker's dev registry on first address and returned as it was.
fn inspect_noted(a: &AgentDecl, binding: &str, make: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    let env_prop = |property: &str| bynk_ts::TsExpr::OptionalMember {
        object: Box::new(bynk_ts::TsExpr::Ident("env".to_string())),
        property: property.to_string(),
    };
    bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident("__noteAgentInstance".to_string())),
        args: vec![
            env_prop(bynk_project::env_protocol::INSPECT_REGISTRY),
            env_prop(binding),
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(a.name.name.clone())),
            bynk_ts::TsExpr::Ident("key".to_string()),
            bynk_ts::TsExpr::Ident(agent_key_codec_name(&a.name.name)),
            make,
        ],
    }
}

/// The agent-construction factory (#3167): `AgentName(key)` lowers to a call of
/// this, which on the bundle target builds the agent over its registered state
/// and on Workers hands back a proxy over the Durable Object stub. Pushes the
/// factory declaration onto `stmts`.
fn emit_agent_factory(
    a: &AgentDecl,
    ctx: &EmitProjectCtx,
    reads_key: bool,
    registry: &str,
    stmts: &mut Vec<bynk_ts::TsStmt>,
) {
    // v0.9.2: agent-construction factory. Lowering of `AgentName(key)` calls
    // this. A present DO binding (workers) routes through `makeWorkersAgent`;
    // otherwise the bundle registry path is taken. The single `makeAgent`
    // helper keeps the call site target-agnostic.
    //
    // Arc C, slice 24 (#1377): closes step (9)'s own sixth sub-slice, the
    // factory function half — the smaller, self-contained half of the
    // originally-lumped "(6) the factory function plus the history-driver"
    // item, converted independently of the history-driver (which stays
    // deferred, its own priority undecided). Converts fully, no opaque
    // carve-out at all: every shape needed (`TsDecl::Function`,
    // `TsExpr::OptionalMember`, `TsExpr::Arrow`, `TsExpr::New`) already
    // existed. The WS-hosted DO methods/`fetch` dispatcher/
    // `emit_ws_dispatch_handlers` cluster (originally sub-slice (5)) stays
    // deferred here — as of this slice, not yet grounded; its own dedicated
    // grounding pass (post-#1378, no issue number) has since found it
    // genuinely lower source-map risk than this comment's own caution
    // implied, decomposing into 3 independent slices — see
    // `design/tracks/the-typescript-tree.md` §6 for the current state,
    // since a comment landed at one slice's own commit is a snapshot, not
    // a live pointer.
    let workers = matches!(ctx.target, BuildTarget::Workers);
    // #3581: a `bynk dev` build records the key each instance is addressed by, so it
    // needs the key codec even for an agent that never reads its key.
    let inspect = workers && ctx.inspect_agents;
    if workers {
        stmts.push(agent_wire_table(a, &ctx.runtime_use));
        if reads_key || inspect {
            stmts.push(agent_key_codec(a, &ctx.runtime_use));
        }
    }
    let key_ts = ts_type_ref_to_ts_type(&a.key_type, None);
    let bind = crate::emitter::bindings::agent_binding(&a.name.name).name;
    let mut make_agent_args = vec![
        bynk_ts::TsExpr::Ident(registry.to_string()),
        bynk_ts::TsExpr::OptionalMember {
            object: Box::new(bynk_ts::TsExpr::Ident("env".to_string())),
            property: bind.clone(),
        },
        bynk_ts::TsExpr::Ident("key".to_string()),
        bynk_ts::TsExpr::Arrow {
            params: vec![bynk_ts::TsParam::new("state".to_string(), None)],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(if reads_key {
                // #1818: the bundle instance learns its key here.
                object_assign_key(&a.name.name)
            } else {
                bynk_ts::TsExpr::New {
                    callee: Box::new(bynk_ts::TsExpr::Ident(a.name.name.clone())),
                    args: vec![bynk_ts::TsExpr::Ident("state".to_string())],
                }
            }))),
        },
    ];
    // #1678: on `workers` the proxy over the DO stub encodes arguments and
    // decodes the result through this agent's wire table.
    if workers {
        make_agent_args.push(bynk_ts::TsExpr::Ident(agent_wire_name(&a.name.name)));
        if reads_key {
            make_agent_args.push(bynk_ts::TsExpr::Ident(agent_key_codec_name(&a.name.name)));
        }
    }
    // #3581: the factory hands the agent back through `__noteAgentInstance`, which
    // records the instance in the dev registry and returns it unchanged.
    let make_agent = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident("__makeAgent".to_string())),
        args: make_agent_args,
    };
    let make_agent = if inspect {
        inspect_noted(a, &bind, make_agent)
    } else {
        make_agent
    };
    let mut env_props = vec![bynk_ts::TsTypeMember::Prop {
        name: bind.clone(),
        ty: bynk_ts::TsType::named("__DurableObjectNamespace"),
        optional: true,
        readonly: false,
    }];
    if inspect {
        env_props.push(bynk_ts::TsTypeMember::Prop {
            name: bynk_project::env_protocol::INSPECT_REGISTRY.to_string(),
            ty: bynk_ts::TsType::named("__DurableObjectNamespace"),
            optional: true,
            readonly: false,
        });
    }
    let factory_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
            name: agent_factory_name(&a.name.name),
            generics: Vec::new(),
            params: vec![
                bynk_ts::TsParam::new("key".to_string(), Some(key_ts)),
                bynk_ts::TsParam::optional(
                    "env".to_string(),
                    Some(bynk_ts::TsType::Object(env_props)),
                ),
            ],
            return_type: Some(bynk_ts::TsType::named(a.name.name.clone())),
            body: vec![bynk_ts::TsStmt::return_stmt(Some(make_agent), None)],
            is_async: false,
            inline: false,
        })),
        None,
    );
    stmts.push(factory_decl);
    // #1486: no explicit blank stmt here — the printer's automatic top-level
    // spacing policy supplies the single blank before whatever follows.
}

/// The entries of an agent's fresh-state record, one per state field: a `Cell` field's initialiser
/// (or its zero), then an empty handle for each `Map`, held map, `Set`, `Cache` and `Log`.
fn agent_zero_entries(
    layout: &AgentStoreLayout,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsObjectEntry> {
    let tys = UnitFacts::of(ctx).render(commons);
    let AgentStoreLayout {
        store_field_ty,
        effective_fields,
        held_maps,
        store_map_fields,
        store_map_indexes,
        set_field_names,
        store_cache_fields,
        store_log_fields,
        ..
    } = layout;
    let mut entries: Vec<bynk_ts::TsObjectEntry> = effective_fields
        .iter()
        .map(|f| {
            let val = match &f.init {
                Some(init) => state_field_init_value(init, commons, ctx),
                None => state_field_zero(f, commons, ctx),
            };
            bynk_ts::TsObjectEntry::prop(f.name.name.clone(), val)
        })
        .collect();
    // A fresh `store Map`/`Set`/`Cache`/`Log` is an empty handle (#3254): insertion-ordered
    // whatever its keys. A `Map` carries the indexers of its `@indexed(by:)` fields, so its
    // handle keeps the posting lists itself (v0.93, ADR 0118).
    for (name, _) in store_map_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Map(_, v)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Map in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        let mut indexed: Vec<&String> = store_map_indexes
            .get(&name.name)
            .map(|fs| fs.iter().collect())
            .unwrap_or_default();
        indexed.sort();
        let args = if indexed.is_empty() {
            Vec::new()
        } else {
            vec![bynk_ts::TsExpr::array(
                indexed.into_iter().map(|f| map_indexer(f)).collect(),
            )]
        };
        entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            store_handle_init("__StoreMap", Some(ts_ty_to_ts_type(value_ty, tys)), args),
        ));
    }
    // A fresh held `Map[K, Connection]` (a `K → connId` handle) is empty too.
    for (name, _) in held_maps {
        entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            store_handle_init(
                "__StoreMap",
                Some(bynk_ts::TsType::named("string")),
                Vec::new(),
            ),
        ));
    }
    for name in set_field_names {
        entries.push(bynk_ts::TsObjectEntry::prop(
            name.to_string(),
            store_handle_init("__StoreSet", None, Vec::new()),
        ));
    }
    for (name, _, _) in store_cache_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Cache(_, v, _)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Cache in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            store_handle_init(
                "__StoreCache",
                Some(ts_ty_to_ts_type(value_ty, tys)),
                Vec::new(),
            ),
        ));
    }
    for (name, _, _) in store_log_fields {
        let elem_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Log(t, _)) => *t,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Log in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        entries.push(bynk_ts::TsObjectEntry::prop(
            name.name.clone(),
            store_handle_init(
                "__StoreLog",
                Some(ts_ty_to_ts_type(elem_ty, tys)),
                Vec::new(),
            ),
        ));
    }
    entries
}

/// `new <handle><<ty>>(<args>)`, a fresh store handle: the type argument is the value (or element)
/// type, and a `Set` carries none.
fn store_handle_init(
    handle: &str,
    type_arg: Option<bynk_ts::TsType>,
    args: Vec<bynk_ts::TsExpr>,
) -> bynk_ts::TsExpr {
    let callee = ident(handle);
    bynk_ts::TsExpr::New {
        callee: Box::new(match type_arg {
            Some(ty) => bynk_ts::TsExpr::TypeApp {
                callee: Box::new(callee),
                type_args: vec![ty],
            },
            None => callee,
        }),
        args,
    }
}

/// `{ index: "<field>", of: (__v) => String(__v.<field>) }`: how a `Map` handle derives the posting
/// key of one `@indexed(by:)` field.
fn map_indexer(field: &str) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::object(vec![
        ("index".to_string(), str_lit(field)),
        (
            "of".to_string(),
            bynk_ts::TsExpr::Arrow {
                params: vec![bynk_ts::TsParam::new("__v".to_string(), None)],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(call(
                    ident("String"),
                    vec![member(ident("__v"), field)],
                )))),
            },
        ),
    ])
}

/// The lowered, static initialiser of a state field. A static initialiser lowers to a pure
/// expression (no setup statements). #1029 review: if any appear, wrap them in an IIFE rather
/// than splicing them into a comma sequence. The comma form only ever worked for
/// expression-shaped hoists: a `const` or an `if` operand does not parse, and T2.1 made a
/// statement-shaped hoist reachable here for the first time, since a value-position `if` that
/// hoists now yields `let …; if (…) {…}` where it used to yield a self-contained arrow. An IIFE is
/// sound for this position specifically: a static initialiser has no enclosing function to
/// `return` out of, so the arrow cannot swallow a control transfer the way it would in a handler
/// body.
fn state_field_init_value(
    init: &Expr,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> bynk_ts::TsExpr {
    let mut pre = Pre::new();
    let mut module = ModuleCtx::for_unit(commons, ctx);
    module.target = ctx.target;
    module.agent_method_givens = ctx.agent_method_givens.clone();
    module.agent_called_contexts = ctx.agent_called_contexts.clone();
    module.event_schema_versions = ctx.event_schema_versions.clone();
    module.consumed_type_ns = consumed_type_ns(ctx);
    module.consumed_type_declared = ctx.consumed_type_aliases.clone();
    module.consumed_context_types = ctx.consumed_context_types();
    let mut icx = LowerCtx::new(module, BodyMode::StaticInit);
    icx.local_agents = ctx.local_agents.clone();
    let expr = pre.lower_node(init, &mut icx);
    if pre.is_empty() {
        expr
    } else {
        compact_iife(pre.ts_stmts(), expr)
    }
}

/// The agent's collection-field description const (`__entryFieldsOf<Agent>`, #3189). The one
/// spelling: the declaration ([`AgentStateNames::fields_const`]) and the inspect branch read it
/// through here (1A.39b, Decision C).
fn entry_fields_const(agent: &str) -> String {
    format!("__entryFieldsOf{agent}")
}

/// The names of an agent's state registry and fresh-state factory.
struct AgentStateNames {
    /// The per-agent state registry (`__agentStates…`).
    registry: String,
    /// The fresh-state factory (`__zeroOf<Agent>State`).
    zero_fn: String,
    /// The description of the agent's collection fields the per-entry storage
    /// codec reads (`__entryFieldsOf<Agent>`, #3189).
    fields_const: String,
}

/// The agent's state record, as the TypeScript the rest of the agent reads: the
/// `<Agent>State` interface, the per-agent state registry, and the fresh-state
/// factory (#3167). Pushes those three declarations, in that order, onto `stmts`.
/// Returns the registry's and the factory's names, which `loadState`, the
/// rehydration step and the test reset refer to.
fn emit_agent_state_record(
    a: &AgentDecl,
    layout: &AgentStoreLayout,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    stmts: &mut Vec<bynk_ts::TsStmt>,
) -> AgentStateNames {
    let tys = UnitFacts::of(ctx).render(commons);
    let state_ty = format!("{}State", a.name.name);
    let AgentStoreLayout {
        store_field_ty,
        effective_fields,
        held_maps,
        store_map_fields,
        store_map_indexes,
        set_field_names,
        store_cache_fields,
        store_log_fields,
        ..
    } = layout;
    // 1) State record type.
    //
    // Arc C, slice 19 (#1367): the first of step (9)'s own proposed 5-6
    // sub-slices (design/tracks/the-typescript-tree.md §6's own grounding
    // pass) — converts this block alone to a real `bynk_ts::TsDecl::Interface`
    // (already exists, gained `type_params`/per-member `readonly` by #1339,
    // no new algebra gap needed). Each field's own `readonly {name}: {ty};`
    // line is a real `TsTypeMember::Prop`; `ts_ty_to_ts_type` (the real-node
    // sibling of the plain-`String` `ts_ty` this block called before) already
    // exists and is already used by `emit_service`/`emit_make_surface` for
    // the identical purpose. Three compound member types, one load-bearing
    // trap among them: `Cache`'s `Record<string, { v: T; exp: number }>` and
    // `Log`'s `Array<{ t: number; v: T }>` are real `TsType::named_with_args`
    // over a real inline `TsType::Object` — no opaque text anywhere in this
    // block. The posting-list's own `Record<string, string[]>` is the third,
    // and it must use `TsType::Array { readonly: false, .. }` (postfix `[]`
    // syntax), NOT `named_with_args("Array", ..)` like `Cache`/`Log`
    // themselves use two blocks down — semantically identical, textually
    // distinct; using the generic form here would silently break zero-diff.
    // See its own construction below for the actual choice, not just this
    // note.
    let mut members: Vec<bynk_ts::TsTypeMember> = Vec::new();
    for f in effective_fields {
        let cell_ty = match store_field_ty.get(f.name.name.as_str()) {
            Some(StoreKindIr::Cell(t)) => *t,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Cell in this \
                 function's own state reader, found {other:?}",
                f.name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: f.name.name.clone(),
            ty: ts_ty_to_ts_type(cell_ty, tys),
            optional: false,
            readonly: true,
        });
    }
    for (name, _) in store_map_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Map(_, v)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Map in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreMap",
                vec![ts_ty_to_ts_type(value_ty, tys)],
            ),
            optional: false,
            readonly: true,
        });
    }
    // v0.105 (slice 3b-ii): a held `Map[K, Connection]` persists `K → connId`.
    for (name, _) in held_maps {
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreMap",
                vec![bynk_ts::TsType::named("string")],
            ),
            optional: false,
            readonly: true,
        });
    }
    for name in set_field_names {
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.to_string(),
            ty: bynk_ts::TsType::named("__StoreSet"),
            optional: false,
            readonly: true,
        });
    }
    // v0.93 (ADR 0118): an `@indexed(by: f)` posting list lives inside the `Map`'s handle
    // (#3254), not in a sibling state field; the wire form still carries it.
    for (name, _, _) in store_cache_fields {
        let value_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Cache(_, v, _)) => *v,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Cache in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreCache",
                vec![ts_ty_to_ts_type(value_ty, tys)],
            ),
            optional: false,
            readonly: true,
        });
    }
    for (name, _, _) in store_log_fields {
        let elem_ty = match store_field_ty.get(name.name.as_str()) {
            Some(StoreKindIr::Log(t, _)) => *t,
            other => panic!(
                "bynk internal error: state field `{}` is not a resolved Log in this \
                 function's own state reader, found {other:?}",
                name.name
            ),
        };
        members.push(bynk_ts::TsTypeMember::Prop {
            name: name.name.clone(),
            ty: bynk_ts::TsType::named_with_args(
                "__StoreLog",
                vec![ts_ty_to_ts_type(elem_ty, tys)],
            ),
            optional: false,
            readonly: true,
        });
    }
    // #1486: `no_blank_before` when this agent's own doc comment is the
    // preceding statement — see `emit_free_fn`'s own identical note. The
    // explicit `TsStmt::blank(None)` push this used to carry is gone —
    // `bynk_ts::print` itself now supplies the one blank line between
    // adjacent real statements.
    let mut state_interface = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Interface {
            name: state_ty.clone(),
            type_params: Vec::new(),
            members,
        })),
        None,
    );
    state_interface.no_blank_before = a.documentation.is_some();
    stmts.push(state_interface);
    // v0.9.2: per-agent state registry (bundle mode + `bynk test`) and the
    // zero-value factory used to initialise a fresh key's state.
    //
    // Arc C, slice 20 (#1369): the second of step (9)'s own proposed 5-6
    // sub-slices — the registry `const` and the zero-factory `function`
    // build real nodes; the zero-record's own per-field VALUES stay
    // opaque where they genuinely must (a `Cell` field with an initialiser
    // lowers through `LowerCtx`, `lower.rs`'s own permanently-excluded
    // general expression lowering — the same "carry an unconverted
    // sibling's already-formed JS text as `TsExpr::Ident`" pattern
    // #1355's own `emit_message_entry_renderer` call already established),
    // but every EMPTY-container field (`Map`/`Set`/`Cache`/held-map/
    // posting-list `{}`, `Log`'s `[]`) is fully real — `TsExpr::object(
    // vec![])`/`TsExpr::array(vec![])`, no opacity needed there at all.
    let registry = agent_registry_name(&a.name.name);
    let zero_fn = format!("__zeroOf{}State", a.name.name);
    let registry_const = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::ConstDecl {
            name: registry.clone(),
            ty: None,
            init: bynk_ts::TsExpr::New {
                callee: Box::new(bynk_ts::TsExpr::Ident("__StateRegistry".to_string())),
                args: Vec::new(),
            },
        },
        None,
    );
    stmts.push(registry_const);
    // v0.11: build the fresh-state record. A field with an explicit initialiser
    // lowers its (static) expression; a field without one uses the v0.9.2
    // implicit zero. A fresh store handle is empty.
    let zero_entries = agent_zero_entries(layout, commons, ctx);
    // #1486: `no_blank_before: true`, unconditional — no blank line wanted
    // between `registry_const` and this function, matching the
    // pre-conversion hand-written text this crate's own zero-diff
    // discipline requires preserving exactly (see `TsStmt::no_blank_
    // before`'s own doc, added specifically for this real, already-shipped
    // pairing). The explicit `TsStmt::blank(None)` push this used to carry
    // afterward is also gone — `bynk_ts::print` itself now supplies the one
    // blank line before whatever follows.
    let mut zero_fn_decl = bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Function {
            name: zero_fn.clone(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: Some(bynk_ts::TsType::named(state_ty.clone())),
            body: vec![bynk_ts::TsStmt::return_stmt(
                Some(bynk_ts::TsExpr::object_entries(zero_entries)),
                None,
            )],
            is_async: false,
            // One line when the record fits on it: a genuinely multi-line `Cell` initialiser
            // makes the printer fall back to the ordinary multi-line block form (1A.39a).
            inline: true,
        },
        None,
    );
    zero_fn_decl.no_blank_before = true;
    stmts.push(zero_fn_decl);
    // Per-entry storage (#3189): which fields are collections, and which index
    // posting records belong to which `Map`. The runtime codec splits a state record
    // by this on commit and joins it back on load.
    let fields_const = entry_fields_const(&a.name.name);
    let mut spec: Vec<String> = Vec::new();
    // A held `Map[K, Connection]` keeps its connection ids inline in the state record:
    // its values are live connections, not stored data, so it is not a per-entry field.
    // (On the bundle target `held_maps` is empty, so ask the declaration.)
    let held_decls = &layout.all_held_maps;
    let held_names: HashSet<&str> = held_decls.iter().map(|(n, _)| n.name.as_str()).collect();
    for (name, _) in store_map_fields
        .iter()
        .filter(|(n, _)| !held_names.contains(n.name.as_str()))
    {
        let indexes: Vec<String> = store_map_indexes
            .get(&name.name)
            .map(|fs| {
                let mut fs: Vec<&String> = fs.iter().collect();
                fs.sort();
                fs.into_iter()
                    .map(|f| {
                        format!(
                            "{{ index: \"{f}\", recordField: \"{}__idx_{f}\" }}",
                            name.name
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        if indexes.is_empty() {
            spec.push(format!("{{ name: \"{}\", kind: \"map\" }}", name.name));
        } else {
            spec.push(format!(
                "{{ name: \"{}\", kind: \"map\", indexes: [{}] }}",
                name.name,
                indexes.join(", ")
            ));
        }
    }
    for name in set_field_names {
        spec.push(format!("{{ name: \"{name}\", kind: \"set\" }}"));
    }
    for (name, _, _) in store_cache_fields {
        spec.push(format!("{{ name: \"{}\", kind: \"cache\" }}", name.name));
    }
    for (name, _, _) in store_log_fields {
        spec.push(format!("{{ name: \"{}\", kind: \"log\" }}", name.name));
    }
    stmts.push(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident(fields_const.clone()),
        None,
        bynk_ts::TsExpr::VerbatimExpr(
            format!("[{}] as const", spec.join(", ")),
            bynk_ts::VerbatimOrigin::Emit,
        ),
        None,
    ));
    AgentStateNames {
        registry,
        zero_fn,
        fields_const,
    }
}

/// The Durable Object constructor: `state` alone, or with the Worker's `env` when the agent
/// rebuilds deps inside the object (#527), in which case the `private __env` field joins `fields`.
fn agent_constructor(
    needs_env_ctor: bool,
    fields: &mut Vec<bynk_ts::TsClassField>,
) -> bynk_ts::TsClassCtor {
    let this_assign = |prop: &str, value: &str| {
        bynk_ts::TsStmt::assign(
            bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
                property: prop.to_string(),
            },
            bynk_ts::TsExpr::Ident(value.to_string()),
            None,
        )
    };
    let state = bynk_ts::TsParam::new(
        "state",
        Some(bynk_ts::TsType::named("__DurableObjectState")),
    );
    if !needs_env_ctor {
        return bynk_ts::TsClassCtor {
            params: vec![state],
            body: vec![this_assign("state", "state")],
        };
    }
    fields.push(bynk_ts::TsClassField::new("__env", bynk_ts::TsType::named("unknown")).private());
    bynk_ts::TsClassCtor {
        params: vec![
            state,
            bynk_ts::TsParam::optional("env", Some(bynk_ts::TsType::named("unknown"))),
        ],
        body: vec![this_assign("state", "state"), this_assign("__env", "env")],
    }
}

pub(crate) fn emit_agent(
    a: &AgentDecl,
    state: &[StoreFieldIr],
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    source_text: &str,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    if let Some(doc) = a.documentation.as_deref() {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc, None));
    }
    let layout = classify_agent_store(a, state, commons, ctx);
    let AgentStateNames {
        registry,
        zero_fn,
        fields_const,
    } = emit_agent_state_record(a, &layout, commons, ctx, &mut stmts);
    let AgentPersistFns {
        rehydrate_fn,
        encode_fn,
        migrate_fn,
        hydrate_fn,
        has_rehydrate,
        has_collections,
        has_encode,
        step_plans,
        agent_versions,
        lazy_fields,
    } = emit_agent_state_persistence(a, &layout, &zero_fn, commons, ctx, &mut stmts);
    let is_workers = layout.is_workers;
    // 2) Durable Object class: fields, constructor and every method are nodes, so the
    // printer alone records the source map (#1482 gave the carrier a map of its own).
    let reads_key = agent_reads_key(a);
    let mut fields = vec![bynk_ts::TsClassField::new(
        "state",
        bynk_ts::TsType::named("__DurableObjectState"),
    )];
    if reads_key {
        // #1818: set before any handler runs, by the bundle factory or the
        // workers `fetch` dispatch.
        fields.push(
            bynk_ts::TsClassField::new("__key", ts_type_ref_to_ts_type(&a.key_type, None))
                .definite(),
        );
    }
    // #527: an agent whose methods take `given` capabilities rebuilds those
    // deps *inside* the DO (providers cannot cross the JSON wire), and some
    // providers take the Worker `env` — workerd passes it as the DO
    // constructor's second argument. Bundle mode constructs with `state`
    // only and never uses the fetch path, so the parameter stays optional.
    let given_deps_expr: Option<GivenDeps> = ctx.agent_given_deps.get(&a.name.name).map(|expr| {
        (
            expr.clone(),
            ctx.agent_given_deps_names
                .get(&a.name.name)
                .cloned()
                .unwrap_or_default(),
        )
    });
    // Events track, slice 0 (spine #936): the same wire problem #527 already
    // solved for capability providers hits `deps.__eventsDispatch` too — it is
    // a function, so it does not survive the DO's JSON wire either. An agent
    // whose own handler body emits directly (`body_emits_directly`'s
    // per-handler test, mirrored here at the whole-agent level) needs its
    // fetch dispatch to rebuild it from `env.EVENTS_FANOUT` exactly as a
    // `given` provider is rebuilt, so it takes the same env-carrying
    // constructor.
    let agent_uses_emit = a
        .handlers
        .iter()
        .any(|h| bynk_model::block_uses_emit(&h.body, &commons.callees));
    // #1960: likewise `deps.env` itself — its Service Bindings and DO
    // namespaces are dead plain objects once the caller's deps cross the
    // wire, so an agent whose handlers call a consumed context's service or
    // construct an agent reads them from its own `env` instead.
    let agent_reaches_env = ctx.target == BuildTarget::Workers
        && a.handlers
            .iter()
            .any(|h| bynk_model::block_reaches_worker_env(&h.body, &commons.callees));
    // #3109: likewise the `from websocket` bodies this agent's Durable Object
    // hosts (`on open` / `on message` / `on close`): they run here, so any
    // Service Binding or DO namespace they reach comes from this object's `env`.
    let ws_hosts_reach_env = ctx.target == BuildTarget::Workers
        && ws_open_hosts_for(&a.name.name, commons, &ctx.local_agents, &ctx.actors)
            .iter()
            .any(|h| h.reaches_env(commons));
    // The `/_bynk/agent/` RPC branch rebuilds on its own condition; the ws
    // hosts only widen the constructor (and the ws entry points).
    let rpc_rebuilds_env = given_deps_expr.is_some() || agent_uses_emit || agent_reaches_env;
    let needs_env_ctor = rpc_rebuilds_env || ws_hosts_reach_env;
    let constructor = agent_constructor(needs_env_ctor, &mut fields);
    let mut methods = Vec::new();
    emit_agent_load_commit_state(
        &mut methods,
        a,
        &layout.effective_fields,
        &AgentLoadFns {
            zero_fn: &zero_fn,
            fields_const: &fields_const,
            rehydrate_fn: &rehydrate_fn,
            encode_fn: &encode_fn,
            migrate_fn: &migrate_fn,
            hydrate_fn: &hydrate_fn,
            has_rehydrate,
            has_collections,
            has_encode,
            step_plans: &step_plans,
            agent_versions: &agent_versions,
            lazy_fields: lazy_fields.as_ref(),
        },
        commons,
        ctx,
        source_text,
    );
    emit_agent_handlers(&mut methods, a, &layout, commons, ctx);
    // v0.104 (real-time track slice 3b): the `from websocket` `on open` handlers
    // whose connection transfers to *this* agent are hosted in this Durable Object
    // (DECISION A) — the upgrade is authenticated at the edge then forwarded here,
    // where the socket is accepted and the body runs as a `this`-self-call.
    let mut ws_open_hosts: Vec<WsOpenHost<'_>> = if is_workers {
        ws_open_hosts_for(&a.name.name, commons, &ctx.local_agents, &ctx.actors)
    } else {
        Vec::new()
    };
    if reads_key {
        for host in &mut ws_open_hosts {
            host.agent_key = Some((
                agent_key_codec_name(&a.name.name),
                ts_type_ref_to_ts_type(&a.key_type, None),
            ));
        }
    }
    for host in &mut ws_open_hosts {
        host.rebuild = WsDepsRebuild {
            given: given_deps_expr.clone(),
            uses_emit: agent_uses_emit,
            reaches_env: host.reaches_env(commons),
        };
    }
    for host in &ws_open_hosts {
        emit_ws_do_method(
            &mut methods,
            a,
            host,
            host.handler,
            &ws_open_do_method_name(host.service),
            commons,
            ctx,
        );
        if let Some(m) = host.message {
            emit_ws_do_method(
                &mut methods,
                a,
                host,
                m,
                &ws_message_do_method_name(host.service),
                commons,
                ctx,
            );
        }
        if let Some(c) = host.close {
            emit_ws_do_method(
                &mut methods,
                a,
                host,
                c,
                &ws_close_do_method_name(host.service),
                commons,
                ctx,
            );
        }
    }
    emit_agent_dispatch(
        &mut methods,
        a,
        &AgentDispatchFacts {
            reads_key,
            given_deps_expr: &given_deps_expr,
            rpc_rebuilds_env,
            agent_uses_emit,
            agent_reaches_env,
            agent_versions: &agent_versions,
        },
        &ws_open_hosts,
        commons,
        ctx,
    );
    // #2270: what follows the last method belongs to the agent, not to that method's
    // final body line.
    let class = bynk_ts::TsDecl::class(a.name.name.as_str(), fields, Some(constructor), methods)
        .ending_at(a.span);
    stmts.push(bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::Export(Box::new(class)),
        None,
    ));
    emit_agent_factory(a, ctx, reads_key, &registry, &mut stmts);
    emit_agent_history_driver(
        a,
        &AgentLoadFns {
            zero_fn: &zero_fn,
            fields_const: &fields_const,
            rehydrate_fn: &rehydrate_fn,
            encode_fn: &encode_fn,
            migrate_fn: &migrate_fn,
            hydrate_fn: &hydrate_fn,
            has_rehydrate,
            has_collections,
            has_encode,
            step_plans: &step_plans,
            agent_versions: &agent_versions,
            // The history driver reads and commits whole.
            lazy_fields: None,
        },
        &registry,
        commons,
        ctx,
        &mut stmts,
    );
    stmts
}

/// #3066: a suite's own `type` and `const` declarations, as TypeScript for the
/// body of one test runner: the types (an interface and its companion object, or
/// a union), their JSON codecs (so `select[T]`, `Json.encode`/`decode` and the
/// boundary paths find `__serialise_T` / `__deserialise_T` in scope), and the
/// constants (cached-function declarations, like a unit's). They are declared
/// local to the runner, so another suite declaring the same name cannot collide
/// and nothing here reaches the deployable. `typed` is the suite's view with the
/// suite's types and constants already merged in; `qual` qualifies a target
/// type a suite type's codec mentions, as the module-level codecs do.
///
/// Returns the statements with the `export` modifiers dropped (a function-local declaration
/// cannot carry one). The driver prints them, at its own test-harness boundary.
pub fn emit_suite_decls(
    types: &[TypeDecl],
    consts: &[ConstDecl],
    typed: &TypedCommons,
    qual: &std::collections::HashMap<String, String>,
    runtime_use: &RuntimeUse,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
    for t in types {
        let Some(def) = typed.types.get(&t.name.name) else {
            continue;
        };
        let shape = bynk_model::lower_suite_type_shape_ir(def, typed);
        stmts.extend(emit_type_in(
            t,
            &shape,
            typed,
            "",
            runtime_use,
            UnitFacts::detached(),
        ));
    }
    if !types.is_empty() {
        // The type itself plus the wrappers a test reads one through: a
        // `select[T]` is a `List[T]`, a `firstAs[T]` an `Option[T]`.
        let roots = suite_base_codec_roots(types);
        let (names, insts) = super::serialisation::collect_codec_closure(&roots, &typed.types);
        stmts.extend(super::serialisation::decls_as_stmts_block(
            super::serialisation::emit_helpers_for_owner_qualified(
                &names,
                &typed.types,
                "",
                qual,
                runtime_use,
            ),
        ));
        stmts.extend(super::serialisation::decls_as_stmts(
            super::serialisation::emit_generic_helpers_qualified(
                &insts,
                &typed.types,
                qual,
                runtime_use,
            ),
        ));
    }
    let empty = bynk_check::resolver::CrossContextInfo::default();
    for c in consts {
        stmts.extend(emit_const_in(
            c,
            ModuleCtx::new(typed, &empty, runtime_use, UnitFacts::detached()),
        ));
    }
    stmts
        .into_iter()
        .map(bynk_ts::TsStmt::without_export)
        .collect()
}

/// The roots a suite's local codecs always cover: each non-generic type plus the
/// `List`/`Option` wrappers a test reads one through (a `select[T]` is a
/// `List[T]`, a `firstAs[T]` an `Option[T]`).
fn suite_base_codec_roots(types: &[TypeDecl]) -> Vec<TypeRef> {
    let mut roots: Vec<TypeRef> = Vec::new();
    for t in types.iter().filter(|t| t.type_params.is_empty()) {
        let named = TypeRef::Named(t.name.clone());
        let span = t.name.span;
        roots.push(TypeRef::List(Box::new(named.clone()), span));
        roots.push(TypeRef::Option(Box::new(named.clone()), span));
        roots.push(named);
    }
    roots
}

/// #3111: the codecs a runner's body reached for beyond the always-present
/// ones ([`emit_suite_decls`]): a suite type under another generic (`Result`,
/// a user generic, nested containers) at a `Json` boundary. `reached` is what
/// the runner's lowering noted; only roots that name a suite type are served
/// here (the module-level closure serves the rest). Returns the function
/// declarations, with `export` dropped, to be spliced ahead of the body; empty
/// when the body reached nothing new.
pub fn emit_suite_late_codecs(
    types: &[TypeDecl],
    typed: &TypedCommons,
    qual: &std::collections::HashMap<String, String>,
    runtime_use: &RuntimeUse,
    reached: &[TyId],
) -> Vec<bynk_ts::TsStmt> {
    if types.is_empty() || reached.is_empty() {
        return Vec::new();
    }
    let names: std::collections::HashSet<String> =
        types.iter().map(|t| t.name.name.clone()).collect();
    let extra: Vec<TypeRef> = reached
        .iter()
        .filter_map(|t| super::ty_to_type_ref(*t, typed.tys()))
        .filter(|r| super::serialisation::root_names_any(r, &names))
        .collect();
    if extra.is_empty() {
        return Vec::new();
    }
    let (base_names, base_insts) =
        super::serialisation::collect_codec_closure(&suite_base_codec_roots(types), &typed.types);
    let (all_names, all_insts) = super::serialisation::collect_codec_closure(&extra, &typed.types);
    let new_names: Vec<String> = all_names
        .into_iter()
        .filter(|n| !base_names.contains(n))
        .collect();
    let base_inst_names: std::collections::HashSet<String> =
        base_insts.iter().map(|i| i.ts_name()).collect();
    let new_insts: Vec<_> = all_insts
        .into_iter()
        .filter(|i| !base_inst_names.contains(&i.ts_name()))
        .collect();
    let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
    stmts.extend(super::serialisation::decls_as_stmts_block(
        super::serialisation::emit_helpers_for_owner_qualified(
            &new_names,
            &typed.types,
            "",
            qual,
            runtime_use,
        ),
    ));
    stmts.extend(super::serialisation::decls_as_stmts(
        super::serialisation::emit_generic_helpers_qualified(
            &new_insts,
            &typed.types,
            qual,
            runtime_use,
        ),
    ));
    stmts
        .into_iter()
        .map(bynk_ts::TsStmt::without_export)
        .collect()
}
