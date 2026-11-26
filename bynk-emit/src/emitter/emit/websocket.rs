//! WebSocket dispatch: the Durable Object methods that host a `from websocket` service's
//! `on open`, `on message` and `on close` handlers, and the `fetch` upgrade branch that reaches them.

use super::{
    actor_sum_ts_union, agent_key_assign, append_deps_field, build_deps_object_ty_with_surface,
    consumed_type_ns, effective_given, entry_fields_const, step_ranges_expr,
};
use crate::emit_model::EmitProjectCtx;
use crate::emitter::ctx::{BodyMode, HandlerShared, ModuleCtx};
use crate::emitter::lower_ctx::LowerCtx;
use crate::emitter::lower_stmt::function_body_stmts;
use crate::emitter::serialisation;
use crate::emitter::ts_types::{ts_ty_to_ts_type, ts_type_ref_to_ts_type};
use crate::emitter::{RuntimeUse, TyRender, UnitFacts, ts_ident, ty_to_type_ref};
use crate::ts_build::{ident, member, num_lit};
use bynk_check::actors::ActorDecl;
use bynk_check::checker::{TyId, TypedCommons};
use bynk_check::firstparty::FirstPartyCap;
use bynk_model::{
    IrHandlerKind, ProtocolIr, is_effectful_return, lower_handler_kind_ir,
    lower_protocol_ir_from_commons,
};
use bynk_syntax::ast::{AgentDecl, CommonsItem, Handler, TypeDecl, TypeRef};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// v0.104 (real-time track slice 3b): a `from websocket` `on open` handler hosted
/// in a Durable Object (DECISION A) — the service it belongs to, the handler, the
/// protocol's `out` frame type (the `Connection`'s parameter), and the Bearer
/// seam (the edge authenticates with it; the DO method threads the verified
/// identity through `deps`).
pub(super) struct WsOpenHost<'a> {
    pub(super) service: &'a str,
    pub(super) handler: &'a Handler,
    // P6.52 (design/tracks/the-ir.md §6b): `TyId`, not `&'a TypeRef` — read
    // off `ProtocolIr::WebSocket` (`lower_protocol_ir_from_commons`) once at
    // `ws_open_hosts_for`'s own construction site, instead of every renderer
    // below re-resolving the raw `ServiceProtocol::WebSocket` frame types
    // itself.
    pub(super) out_ty: TyId,
    // v0.106 (slice 3b-iii): the service's inbound `in` frame type and its optional
    // `on message`/`on close` handlers — run in this same DO (`webSocketMessage`/
    // `webSocketClose`), with identity + route args recovered from the socket
    // attachment the `on open` accept wrote.
    pub(super) in_ty: TyId,
    pub(super) message: Option<&'a Handler>,
    pub(super) close: Option<&'a Handler>,
    pub(super) seam: Option<bynk_check::actors::BearerSeam>,
    /// An actor-sum `by who: A | B`: the members in first-wins order. The edge
    /// forwards which one verified (`{ tag, identity? }`) and the DO rebuilds
    /// `deps.who` from it. Mutually exclusive with `seam`, which a sum never has.
    pub(super) sum: Option<Vec<bynk_check::actors::SumMember>>,
    // #1818: for a hosting agent that reads its own key, the key codec's name
    // and the key's TS type. A WebSocket body runs through these entry points,
    // not the `/_bynk/agent/` dispatch, so each sets `this.__key` itself.
    pub(super) agent_key: Option<(String, bynk_ts::TsType)>,
    // #3045: the `given`/events deps every entry point rebuilds from the DO's `env`.
    pub(super) rebuild: WsDepsRebuild,
}

impl WsOpenHost<'_> {
    /// True if the service has an inbound/close handler — then the `on open` accept
    /// writes the identity + route args into the socket attachment so a waking
    /// `webSocketMessage`/`webSocketClose` can recover them.
    pub(super) fn has_inbound(&self) -> bool {
        self.message.is_some() || self.close.is_some()
    }

    /// #3900: whether the upgrade reads a Bearer token from `Sec-WebSocket-Protocol`: a lone `Bearer`
    /// actor, or an actor sum with a `Bearer` member.
    pub(super) fn reads_bearer_token(&self) -> bool {
        self.seam.is_some()
            || self.sum.as_ref().is_some_and(|members| {
                members
                    .iter()
                    .any(|m| matches!(m.seam, bynk_check::actors::SumMemberSeam::Bearer { .. }))
            })
    }

    /// #3109: whether any of the service's hosted lifecycle bodies reaches the
    /// Worker `env` (a consumed context's service or a local agent).
    pub(super) fn reaches_env(&self, commons: &TypedCommons) -> bool {
        std::iter::once(self.handler)
            .chain(self.message)
            .chain(self.close)
            .any(|h| bynk_model::block_reaches_worker_env(&h.body, &commons.callees))
    }
}

/// The DO method name a hosted `on open` lowers to (`__wsOpen_<Service>`). The
/// edge forwards the upgrade to `/_bynk/ws/open/<Service>`, which dispatches here.
pub(super) fn ws_open_do_method_name(service: &str) -> String {
    format!("__wsOpen_{service}")
}

/// The DO method names a hosted `on message` / `on close` lower to.
pub(super) fn ws_message_do_method_name(service: &str) -> String {
    format!("__wsMessage_{service}")
}

pub(super) fn ws_close_do_method_name(service: &str) -> String {
    format!("__wsClose_{service}")
}

/// Collect the `on open` handlers in `commons` whose connection transfers to the
/// agent named `agent` — its statically-routable single-transfer target (D2). The
/// shape constraint (`bynk.ws.open_transfer_shape`) guarantees at most one such
/// transfer per handler, so the host DO is unambiguous.
pub(super) fn ws_open_hosts_for<'a>(
    agent: &str,
    commons: &'a TypedCommons,
    local_agents: &HashSet<String>,
    actors: &HashMap<String, ActorDecl>,
) -> Vec<WsOpenHost<'a>> {
    let mut hosts = Vec::new();
    for item in &commons.commons.items {
        let CommonsItem::Service(s) = item else {
            continue;
        };
        let ProtocolIr::WebSocket { out_ty, in_ty } =
            lower_protocol_ir_from_commons(&s.protocol, commons)
        else {
            continue;
        };
        for h in &s.handlers {
            if !matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Open) {
                continue;
            }
            if let crate::emitter::websocket::WsOpenShape::One(t) =
                crate::emitter::websocket::analyse_open_shape(&h.body, local_agents)
                && t.agent == agent
            {
                hosts.push(WsOpenHost {
                    service: &s.name.name,
                    handler: h,
                    out_ty,
                    in_ty,
                    message: s
                        .handlers
                        .iter()
                        .find(|h| matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Message)),
                    close: s
                        .handlers
                        .iter()
                        .find(|h| matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Close)),
                    sum: bynk_check::actors::sum_members_for(h, actors),
                    seam: if h.by_clause.as_ref().is_some_and(|by| by.is_sum()) {
                        None
                    } else {
                        bynk_check::actors::bearer_seam_for(h, actors)
                    },
                    agent_key: None,
                    rebuild: WsDepsRebuild::default(),
                });
            }
        }
    }
    hosts
}

/// `typeof message === "string" ? message : new globalThis.TextDecoder().decode(message)`: an
/// incoming frame as text, whichever form the platform delivered.
fn ws_frame_text_expr() -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Conditional {
        test: Box::new(bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::StrictEq,
            left: Box::new(bynk_ts::TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Typeof,
                expr: Box::new(bynk_ts::TsExpr::Ident("message".to_string())),
            }),
            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                "string".to_string(),
            ))),
        }),
        consequent: Box::new(bynk_ts::TsExpr::Ident("message".to_string())),
        alternate: Box::new(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::New {
                    callee: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("globalThis".to_string())),
                        property: "TextDecoder".to_string(),
                    }),
                    args: Vec::new(),
                }),
                property: "decode".to_string(),
            }),
            args: vec![bynk_ts::TsExpr::Ident("message".to_string())],
        }),
    }
}

/// `globalThis.JSON.parse(__raw) as __JsonValue`: the frame text parsed, before the codec decodes it.
fn ws_frame_json_expr() -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::As {
        expr: Box::new(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("globalThis".to_string())),
                    property: "JSON".to_string(),
                }),
                property: "parse".to_string(),
            }),
            args: vec![bynk_ts::TsExpr::Ident("__raw".to_string())],
        }),
        ty: bynk_ts::TsType::named("__JsonValue"),
    }
}

/// `void code; void reason; void wasClean;`: the close handler's parameters, marked used.
fn ws_close_unused_params() -> Vec<bynk_ts::TsStmt> {
    ["code", "reason", "wasClean"]
        .into_iter()
        .map(|name| {
            bynk_ts::TsStmt::expr_stmt(
                bynk_ts::TsExpr::Unary {
                    op: bynk_ts::TsUnaryOp::Void,
                    expr: Box::new(bynk_ts::TsExpr::Ident(name.to_string())),
                },
                None,
            )
        })
        .collect()
}

/// `try { <target> = <value>; } catch { ws.close(1003, "<reason>"); return; }`:
/// one step of decoding an incoming WebSocket frame, closing the socket and
/// returning when it fails.
fn ws_decode_attempt(target: &str, value: bynk_ts::TsExpr, reason: &str) -> bynk_ts::TsStmt {
    let close = bynk_ts::TsStmt::expr_stmt(
        bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("ws".to_string())),
                property: "close".to_string(),
            }),
            args: vec![
                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("1003".to_string())),
                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(reason.to_string())),
            ],
        },
        None,
    );
    bynk_ts::TsStmt::try_catch(
        bynk_ts::TsStmt::inline_block(
            vec![bynk_ts::TsStmt::assign(
                bynk_ts::TsExpr::Ident(target.to_string()),
                value,
                None,
            )],
            None,
        ),
        None::<String>,
        bynk_ts::TsStmt::inline_block(vec![close, bynk_ts::TsStmt::return_stmt(None, None)], None),
        None,
    )
}

/// Emit the DO method that runs a hosted `from websocket` lifecycle body (`on
/// open`/`on message`/`on close`). The synthetic `connection` arrives as the first
/// parameter (the fresh socket for `on open`, the firing socket for `on message`/
/// `on close`), the handler's own parameters follow (for `on message`, the decoded
/// frame and any route values), and the verified identity rides in `deps`. The
/// body lowers with `ws_self_agent` set, so an agent transfer becomes a `this`
/// self-call rather than a cross-instance RPC.
#[allow(clippy::too_many_arguments)]
// Arc C, slice 25 (#1380): the first of step (9) sub-slice (5)'s own 3
// independent slices (the grounding pass post-#1378, `design/tracks/
// the-typescript-tree.md` §6), and the least risky by that pass's own
// finding — the identical shape `emit_provider`'s own already-landed ops
// (#1359) established: params/return-type real, the whole body one opaque
// `TsStmt::Raw`, a single-level offset (no nesting — this method wraps no
// events-IIFE, no implicit-commit closure, unlike `emit_service`/
// `emit_agent`'s own handler loop). `connection: Connection<T>`'s own
// generic type stays one opaque `TsType::named` string, the same "an
// odd, one-off shape stays opaque text" precedent `this.state.storage.
// get<T>` already set twice (#1371/#1373) — `Connection`'s own type
// argument is exactly this function's own real, single grounded need,
// not a general case for a new `TsType` type-argument mechanism. No
// `doc` — confirmed directly (review of #1379): this function has never
// emitted one, no `doc` parameter, no local, no `emit_doc_block` call.
pub(super) fn emit_ws_do_method(
    methods: &mut Vec<bynk_ts::TsClassMethod>,
    agent: &AgentDecl,
    host: &WsOpenHost<'_>,
    h: &Handler,
    method: &str,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) {
    let mut params = vec![bynk_ts::TsParam::new(
        "connection".to_string(),
        Some(bynk_ts::TsType::named_with_args(
            "Connection",
            vec![ts_ty_to_ts_type(
                host.out_ty,
                UnitFacts::of(ctx).render(commons),
            )],
        )),
    )];
    for p in &h.params {
        params.push(bynk_ts::TsParam::new(
            ts_ident(&p.name.name),
            Some(ts_type_ref_to_ts_type(&p.type_ref, None)),
        ));
    }
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
        BodyMode::WsDoMethod {
            handler: HandlerShared {
                capabilities: bynk_model::lower_handler_given_ir(h)
                    .into_iter()
                    .map(|c| c.name)
                    .collect::<HashSet<_>>(),
                // #934: the hosting agent + lifecycle method name
                // (`open`/`message`/`close`).
                handler_scope: Some(format!(
                    "{}.{}.{}",
                    ctx.commons_name, agent.name.name, method
                )),
                owning_context: ctx.commons_name.clone(),
                ..HandlerShared::default()
            },
            // Each lifecycle handler names its own binder: `on message`/`on close`
            // carry their own `by`, and nothing requires the name to match
            // `on open`'s. The principal itself comes from the `on open`
            // verification (the attachment), whichever name a handler gives it.
            deps_identity_binder: host.seam.as_ref().and_then(|_| {
                h.by_clause
                    .as_ref()
                    .and_then(|by| by.binder.as_ref())
                    .map(|b| b.name.clone())
            }),
            actor_sum_binder: host
                .sum
                .as_ref()
                .and(h.by_clause.as_ref())
                .filter(|by| by.is_sum())
                .and_then(|by| by.binder.as_ref())
                .map(|b| b.name.clone()),
            ws_self_agent: Some(agent.name.name.clone()),
        },
    );
    cx.local_agents = ctx.local_agents.clone();
    // #1653: parameters are bindings of the body's own scope, so a `let` that
    // re-binds one gets a fresh name instead of redeclaring it.
    for p in &h.params {
        cx.declare_binder(&p.name.name);
    }
    let async_tail = is_effectful_return(&h.return_type);
    let body = function_body_stmts(&h.body, &mut cx, async_tail, Some(&h.return_type));
    let mut deps_members = build_deps_object_ty_with_surface(
        &effective_given(&bynk_model::lower_handler_given_ir(h), &cx),
        &cx,
        &ctx.cross_context,
        ctx.target,
    );
    if let Some(seam) = host.seam.as_ref().filter(|s| s.binder.is_some()) {
        append_deps_field(
            &mut deps_members,
            "identity",
            bynk_ts::TsType::named(seam.identity_type.clone()),
        );
    }
    if let Some(members) = &host.sum {
        append_deps_field(&mut deps_members, "who", actor_sum_ts_union(members));
    }
    // #3045: the body hands its `deps` on to an agent handler that emits (or emits
    // itself), so `deps` must carry `__eventsDispatch` as an HTTP handler's does.
    // The entry points above spread `__eventsDeps` into it from the DO's `env`.
    if cx.is_first_party(FirstPartyCap::Events)
        && (bynk_model::block_uses_emit(&h.body, &commons.callees)
            || cx
                .agent_given_caps_used()
                .is_some_and(|m| m.contains_key("Events")))
    {
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
    // #2280: anchored to the lifecycle handler's own `on open`/`on message`/
    // `on close` line, not to the previous method's last body line.
    methods.push(bynk_ts::TsClassMethod {
        span: Some(h.span),
        ..bynk_ts::TsClassMethod::new(
            method,
            async_tail,
            params,
            Some(ts_type_ref_to_ts_type(&h.return_type, None)),
            body,
        )
    });
}

/// The `fetch` branches ahead of the method dispatch that need the agent's schema
/// versions: the migration sweep's wake (S4, #2259), and in a `bynk dev` build
/// (#3581) the inspect read.
pub(super) fn wake_and_inspect_branches(
    a: &AgentDecl,
    agent_versions: &Option<bynk_check::schema_registry::AgentVersions>,
    ctx: &EmitProjectCtx,
    commons: &TypedCommons,
) -> Vec<bynk_ts::TsStmt> {
    let mut branches = Vec::new();
    if let Some(v) = agent_versions {
        let unique = super::unique_store_indexes(a, &commons.types);
        branches.push(migrate_wake_branch_stmt(v.version, &unique));
    }
    if ctx.inspect_agents {
        branches.push(inspect_branch_stmt(a, agent_versions));
    }
    branches
}

/// #3581: the `/_bynk/inspect` branch of an agent class's `fetch` in a `bynk dev`
/// build: `return __inspectAgent(this.state, <collection fields>, <versions>, url)`.
/// It reads the stored record and writes nothing, and is reachable only through
/// the Worker's signed `/_bynk/inspect/` route. A class with no schema lock has no
/// version to hold the record to, so `current` is `null` and the check is skipped.
fn inspect_branch_stmt(
    a: &AgentDecl,
    agent_versions: &Option<bynk_check::schema_registry::AgentVersions>,
) -> bynk_ts::TsStmt {
    let num = |n: i64| bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(n.to_string()));
    let (current, oldest, steps) = match agent_versions {
        Some(v) => (
            num(v.version),
            num(v.oldest_readable),
            step_ranges_expr(&v.steps),
        ),
        None => (
            bynk_ts::TsExpr::Ident("null".to_string()),
            num(1),
            step_ranges_expr(&[]),
        ),
    };
    let info = bynk_ts::TsExpr::object(vec![
        (
            "agent".to_string(),
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(a.name.name.clone())),
        ),
        ("current".to_string(), current),
        ("oldestReadable".to_string(), oldest),
        ("steps".to_string(), steps),
    ]);
    bynk_ts::TsStmt::if_stmt(
        bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::StrictEq,
            left: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("url".to_string())),
                property: "pathname".to_string(),
            }),
            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                "/_bynk/inspect".to_string(),
            ))),
        },
        bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__inspectAgent".to_string())),
                args: vec![
                    bynk_ts::TsExpr::Ident("this.state".to_string()),
                    bynk_ts::TsExpr::Ident(entry_fields_const(&a.name.name)),
                    info,
                    bynk_ts::TsExpr::Ident("url".to_string()),
                ],
            }),
            None,
        ),
        None,
    )
}

/// State-migrations S4 (#2259 Decision D): `if (url.pathname ===
/// "/_bynk/migrate") return await __wakeForMigration(await
/// this.state.storage.get("state"), <version>, () => this.loadState(), (s) =>
/// this.commitState(s));`
///
/// #4390: a class with `@unique(by:)` indexes passes them as the wake's last argument, `[["map", "field"], …]`
/// (after `undefined` for the layout, which the runtime defaults), so the answer reports the values more than one
/// key already holds.
fn migrate_wake_branch_stmt(version: i64, unique: &[super::UniqueIndex]) -> bynk_ts::TsStmt {
    let arrow = |params: Vec<bynk_ts::TsParam>, body: bynk_ts::TsExpr| bynk_ts::TsExpr::Arrow {
        params,
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(body))),
    };
    let this_call = |method: &str, args: Vec<bynk_ts::TsExpr>| bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
            property: method.to_string(),
        }),
        args,
    };
    let wake = bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Ident("__wakeForMigration".to_string())),
        args: vec![
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("this.state.storage.get".to_string())),
                args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    "state".to_string(),
                ))],
            })),
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(version.to_string())),
            arrow(Vec::new(), this_call("loadState", Vec::new())),
            arrow(
                vec![bynk_ts::TsParam::new("s".to_string(), None)],
                this_call("commitState", vec![bynk_ts::TsExpr::Ident("s".to_string())]),
            ),
        ]
        .into_iter()
        .chain(unique_wake_args(unique))
        .collect(),
    }));
    bynk_ts::TsStmt::if_stmt(
        bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::StrictEq,
            left: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("url".to_string())),
                property: "pathname".to_string(),
            }),
            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                "/_bynk/migrate".to_string(),
            ))),
        },
        bynk_ts::TsStmt::return_stmt(Some(wake), None),
        None,
    )
}

/// #4390: the wake's trailing `undefined, [["map", "field"], …]`, empty for a class with no `@unique(by:)`.
fn unique_wake_args(unique: &[super::UniqueIndex]) -> Vec<bynk_ts::TsExpr> {
    if unique.is_empty() {
        return Vec::new();
    }
    let str_lit = |s: &str| bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(s.to_string()));
    let pairs = unique
        .iter()
        .map(|u| bynk_ts::TsExpr::array(vec![str_lit(&u.map), str_lit(&u.field)]))
        .collect();
    vec![
        bynk_ts::TsExpr::Ident("undefined".to_string()),
        bynk_ts::TsExpr::array(pairs),
    ]
}

/// `const __eventsDeps = { __eventsDispatch: (events) => __dispatchToEventsFanout(env.EVENTS_FANOUT, events) };`
/// — the fan-out callback is a function, so it never survives the DO wire and is
/// rebuilt from the DO's own `env` (which must already be in scope).
pub(super) fn events_deps_const_stmt() -> bynk_ts::TsStmt {
    let bind = crate::emitter::bindings::events_fanout_binding().name;
    // P7.2: `env` is now `Record<string, unknown>` (above), so this
    // specific binding needs its own cast to what
    // `dispatchToEventsFanout` actually declares.
    bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__eventsDeps".to_string()),
        None,
        bynk_ts::TsExpr::object(vec![(
            "__eventsDispatch".to_string(),
            bynk_ts::TsExpr::Arrow {
                params: vec![bynk_ts::TsParam::new(
                    "events".to_string(),
                    Some(bynk_ts::TsType::named_with_args(
                        "globalThis.Array",
                        vec![bynk_ts::TsType::named(
                            crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE,
                        )],
                    )),
                )],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(
                    bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Ident(
                            "__dispatchToEventsFanout".to_string(),
                        )),
                        args: vec![
                            bynk_ts::TsExpr::As {
                                expr: Box::new(bynk_ts::TsExpr::Member {
                                    object: Box::new(bynk_ts::TsExpr::Ident("env".to_string())),
                                    property: bind,
                                }),
                                ty: bynk_ts::TsType::named("__DurableObjectNamespace"),
                            },
                            bynk_ts::TsExpr::Ident("events".to_string()),
                        ],
                    },
                ))),
            },
        )]),
        None,
    )
}

/// `const __givenDeps = <expr>;` — the agent's `given` providers rebuilt in-process.
pub(super) fn given_deps_const_stmt((expr, names): &GivenDeps) -> bynk_ts::TsStmt {
    // The expression is the driver's printed text; the namespaces it spells are declared beside
    // it (1A.36b-2), so the module's imports follow the tree.
    bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("__givenDeps".to_string()),
        None,
        bynk_ts::TsExpr::raw_using(expr.clone(), names.clone()),
        None,
    )
}

/// An agent's DO-side `given` deps expression and the namespaces (binding modules, other Workers'
/// handlers) it names, both from the driver's deps plan.
pub(super) type GivenDeps = (String, Vec<String>);

/// The in-DO deps a WebSocket entry point (`on open`'s fetch branch,
/// `webSocketMessage`, `webSocketClose`) rebuilds from the DO's own `env`, as the
/// `/_bynk/agent/` branch does: the `given` providers and the events fan-out
/// callback. Neither survives the wire or the socket attachment. Returns the
/// statements (`env`, `__givenDeps`, `__eventsDeps`) and the spreads to append
/// last in the deps object so the rebuilt providers win. Spreads, unlike the
/// shorthand `env`, are exempt from excess-property checks, so the body's own
/// `deps` type need not name them.
#[derive(Clone, Default)]
pub(super) struct WsDepsRebuild {
    pub(super) given: Option<GivenDeps>,
    pub(super) uses_emit: bool,
    /// #3109: a body reaches `deps.env` (Service Bindings, DO namespaces).
    pub(super) reaches_env: bool,
}

impl WsDepsRebuild {
    pub(super) fn is_empty(&self) -> bool {
        self.given.is_none() && !self.uses_emit && !self.reaches_env
    }

    pub(super) fn stmts(&self) -> Vec<bynk_ts::TsStmt> {
        if self.is_empty() {
            return Vec::new();
        }
        let mut stmts = vec![agent_env_const_stmt()];
        if let Some(expr) = &self.given {
            stmts.push(given_deps_const_stmt(expr));
        }
        if self.uses_emit {
            stmts.push(events_deps_const_stmt());
        }
        stmts
    }

    fn spreads(&self) -> Vec<bynk_ts::TsObjectEntry> {
        let mut v = Vec::new();
        if self.given.is_some() {
            v.push(bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(
                "__givenDeps".to_string(),
            )));
        }
        if self.uses_emit {
            v.push(bynk_ts::TsObjectEntry::Spread(bynk_ts::TsExpr::Ident(
                "__eventsDeps".to_string(),
            )));
        }
        if self.reaches_env {
            // A spread, not a shorthand `env`: exempt from excess-property
            // checks for a lifecycle body whose own deps type omits `env`. The
            // `never` cast satisfies the body's precise binding-shape type.
            v.push(bynk_ts::TsObjectEntry::Spread(
                bynk_ts::TsExpr::object_entries(vec![bynk_ts::TsObjectEntry::prop(
                    "env",
                    bynk_ts::TsExpr::As {
                        expr: Box::new(bynk_ts::TsExpr::Ident("env".to_string())),
                        ty: bynk_ts::TsType::named("never"),
                    },
                )]),
            ));
        }
        v
    }

    /// `{ <principal>, ...rebuilt }`, or `{}` when both are empty.
    fn deps_object(&self, principal: Option<bynk_ts::TsObjectEntry>) -> bynk_ts::TsExpr {
        let mut entries: Vec<bynk_ts::TsObjectEntry> = principal.into_iter().collect();
        entries.extend(self.spreads());
        bynk_ts::TsExpr::object_entries(entries)
    }
}

/// `const env = this.__env as unknown as Record<string, unknown>;` — the DO's own
/// `env`, which the `given` providers (and the events fan-out) are rebuilt from.
pub(super) fn agent_env_const_stmt() -> bynk_ts::TsStmt {
    bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("env".to_string()),
        None,
        record_cast(
            crate::ts_build::member(bynk_ts::TsExpr::Ident("this".to_string()), "__env"),
            bynk_ts::TsType::named("unknown"),
        ),
        None,
    )
}

/// `<expr> as unknown as globalThis.Record<string, <value>>`: the double cast the Worker's `env`
/// and the agent's method table are read through. The chain prints bare; a receiver position
/// brackets it.
pub(crate) fn record_cast(expr: bynk_ts::TsExpr, value: bynk_ts::TsType) -> bynk_ts::TsExpr {
    crate::ts_build::as_expr(
        crate::ts_build::as_expr(expr, bynk_ts::TsType::named("unknown")),
        bynk_ts::TsType::named_with_args(
            "globalThis.Record",
            vec![bynk_ts::TsType::named("string"), value],
        ),
    )
}

/// Build the `fetch` branch that completes a forwarded WebSocket upgrade for a
/// hosted `on open`. The edge has already verified the actor, so the body runs
/// authenticated; this accepts the socket, reconstructs the route arguments and
/// identity from the trusted internal header, runs the on-open body, and returns
/// the `101` handing the client end back.
///
/// Arc C, slice 27 (#1384): a real `TsStmt::If` node (a `Block` then-branch),
/// composed directly into `fetch`'s own body — the last consumer of this
/// `&mut String`-writing shape converted, matching #1382's own "boilerplate
/// converts fully" posture. `__payload.args[i] as T`/the deps-object text/
/// the generic `acceptHibernatableConnection<T>` callee stay opaque
/// `TsExpr::Ident` leaves, the same established escape hatch #1382 used —
/// `__payload.identity` itself is a real `TsExpr::Member`, not opaque
/// (review of #1385, finding 2: this doc previously listed it as one).
pub(super) fn ws_open_fetch_branch_stmt(
    host: &WsOpenHost<'_>,
    tys: TyRender<'_>,
) -> bynk_ts::TsStmt {
    let h = host.handler;
    let path = format!("/_bynk/ws/open/{}", host.service);
    let method = ws_open_do_method_name(host.service);
    // The trusted internal header carries the route args, and the verified
    // identity only when the actor binds one (a binder-less `by` forwards none).
    let binds_identity = host.seam.as_ref().is_some_and(|s| s.binder.is_some());
    // An actor sum forwards the member that verified (`who`) in place of a bare
    // identity: the DO cannot tell which member it was from an identity alone.
    let id_field = if host.sum.is_some() {
        "; who: unknown"
    } else if binds_identity {
        "; identity: string"
    } else {
        ""
    };
    let key_field = if host.agent_key.is_some() {
        "; key: unknown"
    } else {
        ""
    };
    let payload_ty = format!("{{ args: unknown[]{id_field}{key_field} }}");
    let mut stmts = vec![
        bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__pair".to_string()),
            None,
            bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Ident("__newWebSocketPair".to_string())),
                args: Vec::new(),
            },
            None,
        ),
        bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__payload".to_string()),
            None,
            bynk_ts::TsExpr::As {
                expr: Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("globalThis.JSON".to_string())),
                        property: "parse".to_string(),
                    }),
                    args: vec![bynk_ts::TsExpr::Binary {
                        op: bynk_ts::TsBinaryOp::NullishCoalescing,
                        left: Box::new(bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Member {
                                object: Box::new(bynk_ts::TsExpr::Member {
                                    object: Box::new(bynk_ts::TsExpr::Ident("request".to_string())),
                                    property: "headers".to_string(),
                                }),
                                property: "get".to_string(),
                            }),
                            args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                "X-Bynk-Ws-Open".to_string(),
                            ))],
                        }),
                        right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                            "{}".to_string(),
                        ))),
                    }],
                }),
                ty: bynk_ts::TsType::named(payload_ty),
            },
            None,
        ),
    ];
    // v0.105 (slice 3b-ii): accept the server socket into the DO *hibernatably*
    // (tagged with a fresh connId, attached for wake-time recovery) so a stored
    // connection survives eviction — not `server.accept()`, which is in-memory only.
    // v0.106 (slice 3b-iii): when the service has an inbound/close handler, also
    // attach the sender identity + route args so a waking `webSocketMessage`/
    // `webSocketClose` recovers them without re-authenticating.
    let mut accept_args = vec![
        bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
            property: "state".to_string(),
        },
        bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("__pair".to_string())),
            property: "server".to_string(),
        },
    ];
    if host.has_inbound() {
        let identity = if binds_identity {
            bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("__payload".to_string())),
                property: "identity".to_string(),
            }
        } else {
            bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(String::new()))
        };
        let mut meta = vec![
            ("identity".to_string(), identity),
            (
                "args".to_string(),
                bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("__payload".to_string())),
                    property: "args".to_string(),
                },
            ),
        ];
        // An actor sum's resolved member rides in the attachment too, so a
        // waking `webSocketMessage`/`webSocketClose` rebuilds `deps.who`.
        if host.sum.is_some() {
            meta.push((
                "who".to_string(),
                bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("__payload".to_string())),
                    property: "who".to_string(),
                },
            ));
        }
        // #1818: a waking `webSocketMessage`/`webSocketClose` recovers the key
        // from the attachment, as it does the identity and route args.
        if host.agent_key.is_some() {
            meta.push((
                "key".to_string(),
                bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("__payload".to_string())),
                    property: "key".to_string(),
                },
            ));
        }
        accept_args.push(bynk_ts::TsExpr::object(meta));
    }
    // #1818: the edge forwards the agent's key with the upgrade.
    if let Some((codec, key_ty)) = &host.agent_key {
        stmts.push(agent_key_assign(
            codec,
            key_ty,
            member(ident("__payload"), "key"),
        ));
    }
    stmts.push(bynk_ts::TsStmt::const_stmt(
        bynk_ts::TsBindingName::Ident("connection".to_string()),
        None,
        bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::TypeApp {
                callee: Box::new(bynk_ts::TsExpr::Ident(
                    "__acceptHibernatableConnection".to_string(),
                )),
                type_args: vec![ts_ty_to_ts_type(host.out_ty, tys)],
            }),
            args: accept_args,
        },
        None,
    ));
    let mut call_args = vec![bynk_ts::TsExpr::Ident("connection".to_string())];
    for (i, p) in h.params.iter().enumerate() {
        // `__payload.args[i] as <type>`: an `Index` under an `As`.
        call_args.push(ws_arg_cast("__payload", i, &p.type_ref));
    }
    // #3042: the agent handler an `on open` transfers to may declare `given`,
    // and `on open` hands it `deps` as they stand. Providers cannot cross the
    // wire, so rebuild them from this DO's own `env`, exactly as the
    // `/_bynk/agent/` branch does, and let them lead the deps object.
    stmts.extend(host.rebuild.stmts());
    let principal = if let Some(members) = &host.sum {
        Some(principal_entry(
            "who",
            "__payload",
            actor_sum_ts_union(members),
        ))
    } else {
        host.seam
            .as_ref()
            .filter(|s| s.binder.is_some())
            .map(|seam| {
                principal_entry(
                    "identity",
                    "__payload",
                    bynk_ts::TsType::named(seam.identity_type.clone()),
                )
            })
    };
    // Rebuilt providers last, as in the `/_bynk/agent/` branch: they win.
    let deps_arg = host.rebuild.deps_object(principal);
    call_args.push(deps_arg);
    let call_expr = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
            property: method,
        }),
        args: call_args,
    };
    stmts.push(bynk_ts::TsStmt::expr_stmt(
        if is_effectful_return(&h.return_type) {
            bynk_ts::TsExpr::Await(Box::new(call_expr))
        } else {
            call_expr
        },
        None,
    ));
    let mut upgrade_args = vec![bynk_ts::TsExpr::Member {
        object: Box::new(bynk_ts::TsExpr::Ident("__pair".to_string())),
        property: "client".to_string(),
    }];
    // #3900: a Bearer upgrade selects the `bynk.bearer` subprotocol the client offered, so a browser,
    // which fails a connection whose response selects none of the subprotocols it offered, keeps it.
    // An upgrade with no Bearer seam offers none and is answered with none.
    if host.reads_bearer_token() {
        upgrade_args.push(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Ident(
                "__webSocketBearerProtocol".to_string(),
            )),
            args: vec![bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("request".to_string())),
                        property: "headers".to_string(),
                    }),
                    property: "get".to_string(),
                }),
                args: vec![bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                    "Sec-WebSocket-Protocol".to_string(),
                ))],
            }],
        });
    }
    stmts.push(bynk_ts::TsStmt::return_stmt(
        Some(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Ident(
                "__webSocketUpgradeResponse".to_string(),
            )),
            args: upgrade_args,
        }),
        None,
    ));
    bynk_ts::TsStmt::if_stmt(
        bynk_ts::TsExpr::Binary {
            op: bynk_ts::TsBinaryOp::StrictEq,
            left: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident("url".to_string())),
                property: "pathname".to_string(),
            }),
            right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(path))),
        },
        bynk_ts::TsStmt::block(stmts, None),
        None,
    )
}

/// v0.106 (slice 3b-iii): the deps argument a hosted `on message`/`on close` body
/// receives — the verified identity recovered from the socket attachment, when the
/// actor binds one (else `{}`).
fn ws_attachment_deps_arg(host: &WsOpenHost<'_>) -> bynk_ts::TsExpr {
    let principal = if let Some(members) = &host.sum {
        Some(principal_entry("who", "__att", actor_sum_ts_union(members)))
    } else {
        host.seam
            .as_ref()
            .filter(|s| s.binder.is_some())
            .map(|seam| {
                principal_entry(
                    "identity",
                    "__att",
                    bynk_ts::TsType::named(seam.identity_type.clone()),
                )
            })
    };
    host.rebuild.deps_object(principal)
}

/// `<field>: <holder>.<field> as <ty>`: the verified principal a hosted body's
/// deps object leads with, read back from the upgrade payload or the socket
/// attachment.
fn principal_entry(field: &str, holder: &str, ty: bynk_ts::TsType) -> bynk_ts::TsObjectEntry {
    bynk_ts::TsObjectEntry::prop(
        field,
        bynk_ts::TsExpr::As {
            expr: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(bynk_ts::TsExpr::Ident(holder.to_string())),
                property: field.to_string(),
            }),
            ty,
        },
    )
}

/// v0.106 (slice 3b-iii): emit the hibernatable-WebSocket dispatch handlers
/// Cloudflare invokes on an accepted socket — `webSocketMessage` (an inbound frame)
/// and `webSocketClose`. Each recovers `{ connId, identity, args }` from the socket
/// attachment the `on open` accept wrote, re-wraps the firing socket as the
/// `connection`, and runs the corresponding DO-hosted body. `webSocketMessage`
/// decodes the raw frame against the service's `in:` type first — a malformed frame
/// closes the socket (`1003`/`1008`) and is never dispatched (the client-bytes trust
/// boundary).
///
/// Arc C, slice 26 (#1382): both methods convert to real
/// `bynk_ts::TsClassMethod` fragments, printed through `print_class_method`.
/// Unlike every prior Arc C slice, this function lowers no `.bynk` body at
/// all (it has no `LowerCtx`, no `body_smb`, no `source_map` parameter) — it
/// is pure dispatch plumbing, so there is nothing to merge back into a
/// module source map, and every statement below builds through real
/// `TsStmt`/`TsExpr` nodes rather than one opaque `Raw` blob, mirroring
/// `loadState`'s own (#1371) "pure boilerplate converts fully" precedent.
/// The two `try { ... } catch { ... }` lines and the `void`-suppression
/// line stay opaque `TsStmt::raw` text — see the proposal issue (#1382) for
/// why (`TryCatch`'s own renderer has no single-line form, and no
/// `TsUnaryOp::Void` exists for a single real call site).
pub(super) fn emit_ws_dispatch_handlers(
    methods: &mut Vec<bynk_ts::TsClassMethod>,
    host: &WsOpenHost<'_>,
    runtime_use: &RuntimeUse,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: TyRender<'_>,
) {
    if !host.has_inbound() {
        return;
    }
    let out_ty = ts_ty_to_ts_type(host.out_ty, tys);
    let att_who = if host.sum.is_some() {
        "; who: unknown"
    } else {
        ""
    };
    let att_ty = if host.agent_key.is_some() {
        format!("{{ connId: string; identity: string{att_who}; args: unknown[]; key: unknown }}")
    } else {
        format!("{{ connId: string; identity: string{att_who}; args: unknown[] }}")
    };
    // The firing socket's minimal structural surface (attachment + send/close), so
    // emitted code stays free of `@cloudflare/workers-types`.
    let ws_ty = "{ deserializeAttachment(): unknown; send(data: string): void; close(code?: number, reason?: string): void }";
    let deps_arg = ws_attachment_deps_arg(host);
    let att_or_null = bynk_ts::TsType::union(vec![
        bynk_ts::TsType::named(att_ty.clone()),
        bynk_ts::TsType::named("null"),
    ]);
    // `const __att = ws.deserializeAttachment() as {att_ty} | null;` / the
    // missing-session guard / `const connection = new WorkersConnection<...>`
    // — identical in both branches below (the pre-conversion `writeln!` code
    // duplicated this same text too), factored once so the two call sites
    // can't independently drift.
    let att_guard = |stmts: &mut Vec<bynk_ts::TsStmt>| {
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__att".to_string()),
            None,
            bynk_ts::TsExpr::As {
                expr: Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("ws".to_string())),
                        property: "deserializeAttachment".to_string(),
                    }),
                    args: Vec::new(),
                }),
                ty: att_or_null.clone(),
            },
            None,
        ));
        stmts.push(bynk_ts::TsStmt::if_stmt(
            bynk_ts::TsExpr::Binary {
                op: bynk_ts::TsBinaryOp::StrictEq,
                left: Box::new(bynk_ts::TsExpr::Ident("__att".to_string())),
                right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Null)),
            },
            bynk_ts::TsStmt::inline_block(
                vec![
                    bynk_ts::TsStmt::expr_stmt(
                        bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Member {
                                object: Box::new(bynk_ts::TsExpr::Ident("ws".to_string())),
                                property: "close".to_string(),
                            }),
                            args: vec![
                                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("1011".to_string())),
                                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str("no session".to_string())),
                            ],
                        },
                        None,
                    ),
                    bynk_ts::TsStmt::return_stmt(None, None),
                ],
                None,
            ),
            None,
        ));
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("connection".to_string()),
            None,
            bynk_ts::TsExpr::New {
                callee: Box::new(bynk_ts::TsExpr::TypeApp {
                    callee: Box::new(bynk_ts::TsExpr::Ident("__WorkersConnection".to_string())),
                    type_args: vec![out_ty.clone()],
                }),
                args: vec![
                    bynk_ts::TsExpr::Ident("ws".to_string()),
                    bynk_ts::TsExpr::Member {
                        object: Box::new(bynk_ts::TsExpr::Ident("__att".to_string())),
                        property: "connId".to_string(),
                    },
                ],
            },
            None,
        ));
        // #1818: restore the key the `on open` attached.
        if let Some((codec, key_ty)) = &host.agent_key {
            stmts.push(agent_key_assign(
                codec,
                key_ty,
                member(ident("__att"), "key"),
            ));
        }
    };

    if let Some(m) = host.message {
        let method = ws_message_do_method_name(host.service);
        // P6.52 (design/tracks/the-ir.md §6b): `host.in_ty` round-trips back
        // to a `TypeRef` for `serialisation::deserialise_expr`, the excluded
        // codec renderer's own boundary (P6.33 ruled it phase 7, `TypeRef`-
        // driven by definition). `ty_to_type_ref` only returns `None` for a
        // function/effect/type-variable `Ty` — none of which a `from
        // websocket` service's own `in:` frame type can ever resolve to,
        // since `check_service_protocols` already constrains it to a
        // codec-eligible type before this function ever runs.
        let in_type = ty_to_type_ref(host.in_ty, tys.types()).unwrap_or_else(|| {
            panic!(
                "bynk internal error: a `from websocket` service's own `in:` frame type did \
                 not round-trip to a TypeRef, but check_service_protocols already certified it \
                 codec-eligible"
            )
        });
        let decode =
            serialisation::deserialise_expr(&in_type, &ident("__json"), "frame", runtime_use);
        let mut stmts = Vec::new();
        att_guard(&mut stmts);
        stmts.extend(host.rebuild.stmts());
        stmts.push(bynk_ts::TsStmt::let_stmt(
            bynk_ts::TsBindingName::Ident("__raw".to_string()),
            Some(bynk_ts::TsType::named("string")),
            None,
            None,
        ));
        stmts.push(ws_decode_attempt(
            "__raw",
            ws_frame_text_expr(),
            "unreadable frame",
        ));
        stmts.push(bynk_ts::TsStmt::let_stmt(
            bynk_ts::TsBindingName::Ident("__json".to_string()),
            Some(bynk_ts::TsType::named("__JsonValue")),
            None,
            None,
        ));
        stmts.push(ws_decode_attempt(
            "__json",
            ws_frame_json_expr(),
            "malformed frame",
        ));
        stmts.push(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident("__dec".to_string()),
            None,
            decode,
            None,
        ));
        stmts.push(bynk_ts::TsStmt::if_stmt(
            bynk_ts::TsExpr::Binary {
                op: bynk_ts::TsBinaryOp::StrictEq,
                left: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("__dec".to_string())),
                    property: "tag".to_string(),
                }),
                right: Box::new(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str("Err".to_string()))),
            },
            bynk_ts::TsStmt::inline_block(
                vec![
                    bynk_ts::TsStmt::expr_stmt(
                        bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Member {
                                object: Box::new(bynk_ts::TsExpr::Ident("ws".to_string())),
                                property: "close".to_string(),
                            }),
                            args: vec![
                                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num("1008".to_string())),
                                bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(
                                    "invalid frame".to_string(),
                                )),
                            ],
                        },
                        None,
                    ),
                    bynk_ts::TsStmt::return_stmt(None, None),
                ],
                None,
            ),
            None,
        ));
        // The decoded frame fills the param typed as the service `in`; the rest are
        // route values recovered (positionally) from the attachment args.
        let mut call_args = vec![ident("connection")];
        let mut route_idx = 0usize;
        // Resolved-`Ty` equality (not `type_refs_match`'s surface-syntax
        // comparison, which fell through to `_ => false` for a `List`/`Map`/
        // `Query`/… frame type — a false negative here emits every param as
        // a route value, silently wrong for a frame type that shape covers):
        // must agree with `check_service_protocols`'s identical resolved
        // check, or the checker's "exactly one frame param" could accept a
        // program this picks a different (or no) param's argument for.
        let no_vars = HashSet::new();
        let resolve_ty = |t: &TypeRef| {
            bynk_check::checker::resolve_type_ref_in(t, types, &no_vars, tys.types())
                .unwrap_or(tys.intern(bynk_check::checker::Ty::Unit))
        };
        for p in &m.params {
            if resolve_ty(&p.type_ref) == host.in_ty {
                call_args.push(member(ident("__dec"), "value"));
            } else {
                call_args.push(ws_arg_cast("__att", route_idx, &p.type_ref));
                route_idx += 1;
            }
        }
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
                    property: method,
                }),
                args: call_args.into_iter().chain([deps_arg.clone()]).collect(),
            })),
            None,
        ));
        let message_method = bynk_ts::TsClassMethod::new(
            "webSocketMessage",
            true,
            vec![
                bynk_ts::TsParam::new("ws".to_string(), Some(bynk_ts::TsType::named(ws_ty))),
                bynk_ts::TsParam::new(
                    "message".to_string(),
                    Some(bynk_ts::TsType::union(vec![
                        bynk_ts::TsType::named("string"),
                        bynk_ts::TsType::named("globalThis.ArrayBuffer"),
                    ])),
                ),
            ],
            Some(bynk_ts::TsType::named_with_args(
                "globalThis.Promise",
                vec![bynk_ts::TsType::named("void")],
            )),
            stmts,
        );
        methods.push(message_method);
    }

    if let Some(c) = host.close {
        let method = ws_close_do_method_name(host.service);
        let mut stmts = ws_close_unused_params();
        att_guard(&mut stmts);
        stmts.extend(host.rebuild.stmts());
        let mut call_args = vec![ident("connection")];
        for (i, p) in c.params.iter().enumerate() {
            call_args.push(ws_arg_cast("__att", i, &p.type_ref));
        }
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Await(Box::new(bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident("this".to_string())),
                    property: method,
                }),
                args: call_args.into_iter().chain([deps_arg]).collect(),
            })),
            None,
        ));
        let close_method = bynk_ts::TsClassMethod::new(
            "webSocketClose",
            true,
            vec![
                bynk_ts::TsParam::new("ws".to_string(), Some(bynk_ts::TsType::named(ws_ty))),
                bynk_ts::TsParam::new("code".to_string(), Some(bynk_ts::TsType::named("number"))),
                bynk_ts::TsParam::new("reason".to_string(), Some(bynk_ts::TsType::named("string"))),
                bynk_ts::TsParam::new(
                    "wasClean".to_string(),
                    Some(bynk_ts::TsType::named("boolean")),
                ),
            ],
            Some(bynk_ts::TsType::named_with_args(
                "globalThis.Promise",
                vec![bynk_ts::TsType::named("void")],
            )),
            stmts,
        );
        methods.push(close_method);
    }
}

/// `<holder>.args[<i>] as <type>`: a wire argument recovered from a hibernated connection's
/// attachment (or the forwarded payload), read at its declared type.
fn ws_arg_cast(holder: &str, i: usize, ty: &TypeRef) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::As {
        expr: Box::new(bynk_ts::TsExpr::Index {
            object: Box::new(member(ident(holder), "args")),
            index: Box::new(num_lit(i)),
        }),
        ty: ts_type_ref_to_ts_type(ty, None),
    }
}
