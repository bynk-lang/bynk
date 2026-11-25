//! Agent handlers and dispatch: an agent class's handler methods (`emit_agent_handlers`), the Workers
//! `fetch` dispatch (`emit_agent_dispatch`) and the agent factory (`emit_agent_factory`).

use super::{
    AgentStoreLayout, GivenDeps, UniqueIndex, WsOpenHost, agent_env_const_stmt, agent_key_assign,
    agent_key_codec, agent_key_codec_name, agent_wire_name, agent_wire_table, append_deps_field,
    build_deps_object_ty_with_surface, consumed_type_ns, effective_given,
    emit_ws_dispatch_handlers, events_decl_item, events_deps_const_stmt, events_flush_item,
    given_deps_const_stmt, object_assign_key, record_cast, result_closure_item, return_result_item,
    wake_and_inspect_branches, ws_open_fetch_branch_stmt,
};
use crate::emit_model::EmitProjectCtx;
use crate::emitter::ctx::{AgentStoreState, BodyMode, HandlerShared, ModuleCtx};
use crate::emitter::lower_ctx::LowerCtx;
use crate::emitter::lower_stmt::{Body, Item, block_body_with_return, claim_scoped};
use crate::emitter::ts_types::ts_type_ref_to_ts_type;
use crate::emitter::{BuildTarget, Ty, UnitFacts, agent_factory_name, ts_ident};
use crate::ts_build::{call, ident};
use bynk_check::checker::TypedCommons;
use bynk_check::firstparty::FirstPartyCap;
use bynk_model::{
    IrHandlerKind, StoreKindIr, body_writes_state, is_effectful_return, lower_handler_kind_ir,
};
use bynk_syntax::ast::AgentDecl;
use std::collections::{HashMap, HashSet};

/// An agent class's handler methods (#3167): one per `on` handler, each lowered
/// to a class method and written onto `out`, in declaration order.
pub(super) fn emit_agent_handlers(
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
                indexes: routable_indexes(store_map_indexes, &layout.unique_indexes),
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
                composite_keys: layout.composite_keys.clone(),
                composite_indexes: layout.composite_indexes.clone(),
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
pub(super) struct AgentDispatchFacts<'a> {
    /// The handler reads the agent's key (`self.key`).
    pub(super) reads_key: bool,
    /// The `given` capability dependencies the RPC branch rebuilds, if any.
    pub(super) given_deps_expr: &'a Option<GivenDeps>,
    /// Whether the RPC branch rebuilds the agent's environment at all.
    pub(super) rpc_rebuilds_env: bool,
    /// Whether a handler body emits events directly.
    pub(super) agent_uses_emit: bool,
    /// Whether a handler body reaches the Worker's own `env`.
    pub(super) agent_reaches_env: bool,
    /// The agent's declared schema versions, if it has any.
    pub(super) agent_versions: &'a Option<bynk_check::schema_registry::AgentVersions>,
}

/// The tail of an agent class (#3167): the Workers `fetch` dispatch that turns
/// an RPC request into a method call, the dispatch for each `from websocket`
/// `on open` host, and the class's closing brace. Written onto `out`.
pub(super) fn emit_agent_dispatch(
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
pub(super) fn emit_agent_factory(
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

/// The `@indexed` fields an equality `filter` may route through (#4038): every indexed field but a
/// `@unique` `Option`, whose posting is keyed by the `Some` payload, so a lookup by the whole
/// `Option` would not find it.
fn routable_indexes(
    indexes: &HashMap<String, Vec<String>>,
    unique: &[UniqueIndex],
) -> HashMap<String, Vec<String>> {
    indexes
        .iter()
        .map(|(map, fields)| {
            let kept = fields
                .iter()
                .filter(|f| {
                    !unique
                        .iter()
                        .any(|u| u.optional.is_some() && u.map == *map && u.field == **f)
                })
                .cloned()
                .collect();
            (map.clone(), kept)
        })
        .collect()
}
