//! The Worker edge wrappers: the `on call`, `on http`, event, cron, queue and
//! WebSocket adapters that a Worker's `compose.ts` returns (v0.8 §4.5, v0.9 §5.1).
//!
//! The composition root that assembles them, `emit_worker_compose`, lives in
//! [`super::compose::workers`] (back-half paydown 1B.1a moved it out).
//!
//! Arc C slice 3 (#1321): these wrappers build a real `bynk_ts::TsProgram`
//! tree directly instead of `writeln!`-ing a `String`.

use std::collections::{BTreeMap, HashMap, HashSet};

use super::internal_fault::undefined_node;
use super::workers_entry::wire_ident;
use crate::emitter::bindings::agent_binding;
use crate::emitter::http_handler_method_name_ir;
use crate::ts_build::{
    as_expr, await_expr, binary, block, call, const_, expr_stmt, ident, if_, member, method_call,
    new, not_expr, null_lit, num_lit, or_expr, return_, str_lit, strict_eq, strict_neq,
    typeof_expr,
};
use bynk_check::symbols::UnitTable;
use bynk_syntax::ast::{ActorDecl, ExprKind, Handler, ServiceDecl, ServiceProtocol, TypeRef};
use bynk_ts::{TsBindingName, TsExpr, TsObjectEntry, TsParam, TsStmt, TsType, TsTypeMember};

// -- Helpers over the shared builders (`crate::ts_build`) -----------------

/// `base.a.b. …` — a left-associative chain of plain member accesses, e.g.
/// `member_chain(ident("handlers"), &[sname, "call"])` for
/// `handlers.<sname>.call`.
pub(super) fn member_chain(base: TsExpr, properties: &[&str]) -> TsExpr {
    properties.iter().fold(base, |acc, p| member(acc, *p))
}

pub(super) fn status_object(code: &str) -> TsExpr {
    TsExpr::object(vec![("status".to_string(), num_lit(code))])
}

pub(super) fn new_response(args: Vec<TsExpr>) -> TsExpr {
    new(ident("globalThis.Response"), args)
}

/// `{ ...deps, <key>: <value> }` — the three real "spread `deps`, override
/// one identity-shaped field" sites (Decision A, gap 2): `emit_call_wrapper`'s
/// `identity`, `emit_http_wrapper`/`emit_http_oidc_wrapper`'s `identity`,
/// `emit_http_sum_wrapper`'s `who`.
pub(super) fn deps_spread_with(key: &str, value: TsExpr) -> TsExpr {
    TsExpr::object_entries(vec![
        TsObjectEntry::Spread(ident("deps")),
        TsObjectEntry::prop(key.to_string(), value),
    ])
}

/// `(env as unknown as Record<string, unknown>)["<secret>"] ?? (globalThis
/// as { process?: { env?: Record<string, unknown> } }).process?.env?.[
/// "<secret>"]` — the shared secret-probe idiom (Decision A, gaps 4/5):
/// three real, identical sites (`emit_websocket_upgrade`,
/// `emit_http_wrapper`, `emit_secret_lookup`'s own `emit_http_sum_wrapper`
/// callers). `secret` is already TS-string-escaped by the caller
/// (`crate::emitter::escape_ts_string`) — this function only builds the
/// tree, matching every other helper here.
pub(super) fn secret_probe_expr(secret: &str) -> TsExpr {
    let env_record_ty = TsType::named_with_args(
        "globalThis.Record",
        vec![TsType::named("string"), TsType::named("unknown")],
    );
    let explicit = TsExpr::Index {
        object: Box::new(as_expr(
            as_expr(ident("env"), TsType::named("unknown")),
            env_record_ty.clone(),
        )),
        index: Box::new(str_lit(secret)),
    };
    let process_ty = TsType::Object(vec![TsTypeMember::optional_prop(
        "process",
        TsType::Object(vec![TsTypeMember::optional_prop("env", env_record_ty)]),
    )]);
    let global_probe = TsExpr::OptionalIndex {
        object: Box::new(TsExpr::OptionalMember {
            object: Box::new(member(as_expr(ident("globalThis"), process_ty), "process")),
            property: "env".to_string(),
        }),
        index: Box::new(str_lit(secret)),
    };
    binary(
        bynk_ts::TsBinaryOp::NullishCoalescing,
        explicit,
        global_probe,
    )
}

/// v0.176 (#642, Decision E): an `on call` wrapper's parameters carry their real
/// TS type instead of `any`. This is what makes the boundary's type-safety
/// *checkable*: with `any` here, a wrong codec still type-checks, so the only
/// guard against regression would be the goldens. Named types are qualified
/// against the `handlers` namespace this file already imports — `compose.ts`
/// imports no type names of its own.
///
/// Scoped to `on call` deliberately. The other wrappers keep `: any` because
/// their parameters are not all codec-produced: an HTTP or WebSocket wrapper
/// mixes a deserialised `body` with route/query params the entry lifts from the
/// URL as raw strings, so typing them against the *declared* type would assert a
/// coercion that seam does not perform. Those are separate boundaries with their
/// own extraction rules, and they are not what this increment fixed.
pub(super) fn emit_call_wrapper(
    sname: &str,
    h: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> TsObjectEntry {
    // Qualify *every* named type in the signature, not a subset. The wrapper
    // forwards to `handlers.{sname}.call(...)`, whose parameter `handlers.ts`
    // types with this same name in its own scope — so `handlers.<Name>` resolves
    // by construction, for a context-declared type and for a `uses`-imported
    // commons type alike (the latter is re-exported rebranded, and is exactly
    // the case a `table.types`-derived scope would miss), and for a consumed
    // context's type (#2036: re-exported as a type, `export { type Batch }`).
    let scope: HashSet<String> = h
        .params
        .iter()
        .flat_map(|p| named_types_in(&p.type_ref))
        .collect();
    // The wrapper forwards positionally, so the binder and the forwarded argument
    // must agree — route both through `ts_ident` so a reserved-word param name
    // (`class`, `void`, …) doesn't emit an invalid TS binding (#723).
    let mut params: Vec<TsParam> = h
        .params
        .iter()
        .map(|p| {
            TsParam::new(
                wire_ident(&p.name.name),
                Some(crate::emitter::ts_types::ts_type_ref_qualified_ts_type(
                    &p.type_ref,
                    &scope,
                    "handlers",
                )),
            )
        })
        .collect();
    let param_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(wire_ident(&p.name.name)))
        .collect();
    // v0.54: a `by c: Caller` handler's wrapper takes the caller's context name
    // (read from the header in the entry dispatch) and threads it into `deps`
    // as the `CallerId` identity — mirroring the Bearer identity threading.
    // I3 (#1971): a `call` sum's wrapper takes the resolved actor, which the
    // entry dispatch builds from the verified caller, and threads it into
    // `deps` as `who`.
    let call_sum = bynk_check::actors::call_admission_for(h, actors).and_then(|a| a.sum);
    let deps_expr = if let Some(members) = &call_sum {
        params.insert(
            0,
            TsParam::new(
                "__who".to_string(),
                Some(crate::emitter::emit::call_sum_ts_type(members)),
            ),
        );
        deps_spread_with("who", ident("__who"))
    } else if bynk_check::actors::caller_binder_for(h, actors).is_some() {
        params.insert(
            0,
            TsParam::new("__caller".to_string(), Some(TsType::named("string"))),
        );
        deps_spread_with("identity", ident("__caller"))
    } else {
        ident("deps")
    };
    let mut call_args = param_args;
    call_args.push(deps_expr);
    TsObjectEntry::Method {
        name: sname.to_string(),
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: None,
        doc: None,
        inline: false,
        body: vec![return_(Some(call(
            member_chain(ident("handlers"), &[sname, "call"]),
            call_args,
        )))],
        span: None,
    }
}

/// Events track, slice 0 (spine #936): the compose-surface wrapper for a
/// `from Events(E)` service's `on event` handler. Reached by
/// `emit_worker_entry`'s `/_bynk/event/<sname>` route (which the *publisher*
/// context's fan-out DO calls over this subscriber's Service Binding) — never
/// by an HTTP route, so no path/CORS/actor plumbing to mirror from
/// `emit_call_wrapper`.
///
/// Slice 2: the route above always passes both `payload` and `envelope`
/// uniformly; this wrapper is the one place that knows whether *this*
/// subscriber's handler actually declared the optional second
/// `env: EventEnvelope` parameter (`h.params.len() == 2`, already enforced
/// by `bynk.event.bad_params`), and forwards it into the generated method
/// only then — a handler that kept `on event(e: E)` sees no change to its
/// call at all.
///
/// Slice 4 (#985): a `via schema(N)` clause needs the envelope even when
/// the handler didn't declare it — `emit_service` inserts a synthetic
/// `env` parameter in that case, and needs the value forwarded to line up
/// positionally, so the condition widens to match.
/// #2341 (decision C): the compose-surface wrapper for `on rejected` — the
/// rejection the router built, passed to the handler with its deps.
pub(super) fn emit_rejected_wrapper(sname: &str) -> TsObjectEntry {
    TsObjectEntry::Method {
        name: format!("rejected_{sname}"),
        is_async: true,
        generics: Vec::new(),
        params: vec![TsParam::new(
            "why".to_string(),
            Some(TsType::named(
                "{ readonly tag: string; readonly detail: string }",
            )),
        )],
        return_type: None,
        doc: None,
        inline: false,
        body: vec![return_(Some(call(
            member_chain(ident("handlers"), &[sname, "rejected"]),
            vec![ident("why"), ident("deps")],
        )))],
        span: None,
    }
}

/// `wants_envelope` is the plan's decision for this subscriber (`EventSubscriberShape::
/// wants_envelope`, 1B.1c): the wrapper only spells it.
pub(super) fn emit_event_wrapper(sname: &str, wants_envelope: bool) -> TsObjectEntry {
    // P7.2: deferred, not narrowed. Two attempts: a bare `ts_type_ref` failed
    // ("Cannot find name" — the event's synthetic type needs the `handlers.`
    // prefix here), and qualifying it via `qualified_ts_type_ref` (the same
    // `emit_call_wrapper` idiom) *also* failed, differently, on cross-context
    // fixtures ("Namespace has no exported member" — the event type genuinely
    // isn't exported from `handlers.ts` in that shape for a cross-context
    // subscriber). The real fix needs tracing exactly where a cross-context
    // event's synthetic type is actually exported from, not another guess at
    // the qualification scheme.
    let mut args = vec![as_expr(ident("payload"), TsType::named("any"))];
    if wants_envelope {
        args.push(as_expr(ident("envelope"), TsType::named("any")));
    }
    args.push(ident("deps"));
    TsObjectEntry::Method {
        name: sname.to_string(),
        is_async: true,
        generics: Vec::new(),
        params: vec![
            TsParam::new("payload".to_string(), Some(TsType::named("unknown"))),
            TsParam::new("envelope".to_string(), Some(TsType::named("unknown"))),
        ],
        return_type: None,
        doc: None,
        inline: false,
        body: vec![return_(Some(call(
            member_chain(ident("handlers"), &[sname, "event"]),
            args,
        )))],
        span: None,
    }
}

pub(super) fn emit_cron_wrapper(sname: &str, cron_idx: usize, h: &Handler) -> TsObjectEntry {
    let method_key = crate::emitter::cron_handler_method_name(sname, cron_idx);
    // A cron handler takes an optional scheduled-time parameter; forward it (if
    // any) to the bound handler, with deps trailing. P7.2: the checker requires
    // this parameter be `Int`, and `workers_entry.rs`'s scheduled() dispatch hands
    // it Cloudflare's own `event.scheduledTime: number` directly — codec-matched,
    // safe to type for real.
    let params: Vec<TsParam> = h
        .params
        .iter()
        .map(|p| {
            TsParam::new(
                wire_ident(&p.name.name),
                Some(qualified_ts_type_ref(&p.type_ref)),
            )
        })
        .collect();
    let mut call_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(wire_ident(&p.name.name)))
        .collect();
    call_args.push(ident("deps"));
    TsObjectEntry::Method {
        name: method_key.clone(),
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: None,
        doc: None,
        inline: false,
        body: vec![return_(Some(call(
            member_chain(ident("handlers"), &[sname, &method_key]),
            call_args,
        )))],
        span: None,
    }
}

/// The compose wrapper of a service's `on message` handler, or `None` for a
/// `from websocket` one: that is an *inbound* handler that runs in the
/// connection-hosting Durable Object (`webSocketMessage`), not at the edge
/// (v0.106, slice 3b-iii), so it has no compose wrapper. A `from queue` one is the
/// queue consumer and a `from email` one (#3328) the email handler. A raw
/// `ServiceProtocol` match (not `ProtocolIr`, P6.30): `emit_worker_compose` has no
/// `TypedCommons` in scope to lower it with.
pub(super) fn message_wrapper(
    service: &ServiceDecl,
    sname: &str,
    queue_idx: usize,
    h: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> Option<TsObjectEntry> {
    if matches!(service.protocol, ServiceProtocol::WebSocket { .. }) {
        return None;
    }
    Some(emit_queue_wrapper(
        sname,
        queue_idx,
        h,
        email_actors(service, actors),
    ))
}

/// `Some(actors)` for a `from email` service (see [`emit_queue_wrapper`]).
fn email_actors<'a>(
    service: &ServiceDecl,
    actors: &'a HashMap<String, ActorDecl>,
) -> Option<&'a HashMap<String, ActorDecl>> {
    matches!(service.protocol, ServiceProtocol::Email { .. }).then_some(actors)
}

/// The compose wrapper of a `from queue` or `from email` `on message` handler.
/// `email` is `Some(actors)` for the latter: the Worker's `email()` entry
/// passes the minted `SenderIdentity` as a second argument, which a handler
/// that binds it (`by s: Sender`) reads through `deps.identity`.
fn emit_queue_wrapper(
    sname: &str,
    queue_idx: usize,
    h: &Handler,
    email: Option<&HashMap<String, ActorDecl>>,
) -> TsObjectEntry {
    let method_key = crate::emitter::queue_handler_method_name(sname, queue_idx);
    // The queue handler takes its message parameter; forward it with deps. P7.2:
    // `workers_entry.rs`'s queue() dispatch always calls `deserialise_call` against
    // this same declared type before forwarding `__r.value` here — codec-matched,
    // safe to type for real. (No param at all when the handler declares none, in
    // which case this list — and the corresponding entry-side msg_type — is empty.)
    let mut params: Vec<TsParam> = h
        .params
        .iter()
        .map(|p| {
            TsParam::new(
                wire_ident(&p.name.name),
                Some(qualified_ts_type_ref(&p.type_ref)),
            )
        })
        .collect();
    let mut call_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(wire_ident(&p.name.name)))
        .collect();
    match email {
        Some(actors) => {
            params.push(TsParam::new(
                "__sender".to_string(),
                Some(TsType::named("__SenderIdentity")),
            ));
            if bynk_check::actors::sender_binder_for(h, actors).is_some() {
                call_args.push(deps_spread_with("identity", ident("__sender")));
            } else {
                call_args.push(ident("deps"));
            }
        }
        None => call_args.push(ident("deps")),
    }
    TsObjectEntry::Method {
        name: method_key.clone(),
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: None,
        doc: None,
        inline: false,
        body: vec![return_(Some(call(
            member_chain(ident("handlers"), &[sname, &method_key]),
            call_args,
        )))],
        span: None,
    }
}

/// The TypeScript literal of a `WebSocket` open key written as a literal
/// (`classify_open_key` admits only a string, an integer or a boolean).
fn open_key_literal(e: &bynk_syntax::ast::Expr) -> TsExpr {
    match &e.kind {
        ExprKind::StrLit(v) => str_lit(v.clone()),
        ExprKind::IntLit { value, .. } => num_lit(value.to_string()),
        ExprKind::BoolLit(b) => ident(b.to_string()),
        ExprKind::FloatLit { .. }
        | ExprKind::DecimalLit { .. }
        | ExprKind::DurationLit { .. }
        | ExprKind::InterpStr(_)
        | ExprKind::Ident(_)
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
        | ExprKind::FieldAccess { .. }
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
        | ExprKind::Trace { .. } => {
            unreachable!("classify_open_key returns only these literals")
        }
    }
}

/// v0.104 (real-time track slice 3b): emit the WebSocket upgrade route — the
/// edge half of DECISION A. The upgrade authenticates the actor **in the Worker,
/// before any request reaches the Durable Object** (the safety boundary): it
/// reads the Bearer token from the first `Sec-WebSocket-Protocol` element
/// (DECISION C), verifies it fail-closed with the same audited JWT verifier HTTP
/// uses, and only on success forwards the upgrade request to the addressed DO —
/// the agent the `on open` transfers the connection to (DECISION B), keyed by a
/// request parameter. The verified identity rides in a trusted internal header
/// (the DO is only reachable through this Worker, the same Internal-channel trust
/// the cross-context caller seam relies on). A failure returns `401`/`426` and
/// **does not forward** — no socket is accepted unauthenticated.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_websocket_upgrade(
    sname: &str,
    h: &Handler,
    seam: Option<&bynk_check::actors::BearerSeam>,
    sum: Option<&[bynk_check::actors::SumMember]>,
    local_agents: &HashSet<String>,
    key_readers: &HashSet<String>,
    consts: &HashMap<String, bynk_check::consts::VisibleConst<'_>>,
    const_imports: &mut BTreeMap<String, (String, String)>,
) -> TsObjectEntry {
    use crate::emitter::websocket::{
        WsOpenKey, WsOpenShape, analyse_open_shape, classify_open_key,
    };
    // The route params (e.g. `roomId`) ride as wrapper arguments — the entry
    // extracts them from the upgrade URL's query string and passes them through.
    // P7.2: `string`, not `unknown` — a first attempt used `unknown` (raw, not
    // the declared type, v0.176's own rationale for `emit_call_wrapper`'s
    // scoping applying "the entry never coerces them"), but broke real
    // `tsc --strict` fixtures: the wrapper's own body below validates each
    // param itself, inline, via `handlers.{Type}.of(roomId)` — genuinely a
    // `string` argument (`url.searchParams.get(...)`, non-null-checked by the
    // entry before this wrapper is called), just not yet refined/branded.
    // `unknown` was one step too vague for `.of`'s own `string`-typed
    // constructor parameter.
    let mut params: Vec<TsParam> = vec![TsParam::new(
        "request".to_string(),
        Some(TsType::named("globalThis.Request")),
    )];
    params.extend(
        h.params
            .iter()
            .map(|p| TsParam::new(wire_ident(&p.name.name), Some(TsType::named("string")))),
    );
    let method_name = format!("ws_{sname}_open");

    let mut stmts: Vec<TsStmt> = Vec::new();
    // Require an actual WebSocket upgrade before anything else.
    stmts.push(if_(
        strict_neq(
            method_call(
                member(ident("request"), "headers"),
                "get",
                vec![str_lit("Upgrade")],
            ),
            str_lit("websocket"),
        ),
        return_(Some(new_response(vec![
            str_lit("Expected a WebSocket upgrade"),
            status_object("426"),
        ]))),
    ));

    // DECISION C: a Bearer token arrives as the first `Sec-WebSocket-Protocol`
    // subprotocol element (a browser sets it via `new WebSocket(url, [token])`),
    // verified fail-closed before the request is forwarded. The `on open` requires
    // a `by` actor, but — exactly as an HTTP `by v: Visitor` route — that actor's
    // scheme may be `None` (an intentional anonymous channel): then `seam` is
    // `None` and no token is read. `Signature` is rejected at the WS boundary (a
    // browser cannot sign the handshake), so a present seam is always Bearer.
    // An actor sum (`by who: User | Visitor`): resolve first-wins, as the HTTP
    // sum wrapper does, with the same token source as the single-actor path. A
    // `None` member (`Visitor`) always resolves, so a bad or absent token falls
    // through to it; with no catch-all member nothing resolving is a `401`.
    // The member that verified is forwarded as a tagged `who`, the shape the
    // HTTP path threads through `deps.who`, so the Durable Object rebuilds it.
    if let Some(members) = sum {
        use bynk_check::actors::SumMemberSeam;
        stmts.push(const_(
            "__proto",
            method_call(
                member(ident("request"), "headers"),
                "get",
                vec![str_lit("Sec-WebSocket-Protocol")],
            ),
        ));
        stmts.push(TsStmt::let_stmt(
            TsBindingName::Ident("__token".to_string()),
            Some(TsType::named("string | undefined")),
            Some(ident("undefined")),
            None,
        ));
        stmts.push(if_(
            strict_neq(ident("__proto"), null_lit()),
            TsStmt::assign(
                ident("__token"),
                method_call(
                    TsExpr::Index {
                        object: Box::new(method_call(
                            ident("__proto"),
                            "split",
                            vec![str_lit(",")],
                        )),
                        index: Box::new(num_lit("0")),
                    },
                    "trim",
                    vec![],
                ),
                None,
            ),
        ));
        stmts.push(TsStmt::let_stmt(
            TsBindingName::Ident("__who".to_string()),
            Some(TsType::named(
                "{ tag: string; identity?: string } | undefined",
            )),
            Some(ident("undefined")),
            None,
        ));
        for m in members {
            let tag = crate::emitter::escape_ts_string(&m.actor_name);
            let inner: Vec<TsStmt> = match &m.seam {
                SumMemberSeam::None => vec![TsStmt::assign(
                    ident("__who"),
                    TsExpr::object(vec![("tag".to_string(), str_lit(tag))]),
                    None,
                )],
                SumMemberSeam::Bearer {
                    secret,
                    identity_type,
                } => {
                    let secret = crate::emitter::escape_ts_string(secret);
                    vec![if_(
                        strict_neq(ident("__token"), ident("undefined")),
                        block(vec![
                            const_("__secret", secret_probe_expr(&secret)),
                            if_(
                                strict_eq(typeof_expr(ident("__secret")), str_lit("string")),
                                block(vec![
                                    const_(
                                        "__claims",
                                        await_expr(call(
                                            ident("__verifyBearerJwtHs256"),
                                            vec![ident("__token"), ident("__secret")],
                                        )),
                                    ),
                                    if_(
                                        strict_eq(member(ident("__claims"), "tag"), str_lit("Ok")),
                                        block(vec![
                                            const_(
                                                "__id",
                                                method_call(
                                                    member(
                                                        ident("handlers"),
                                                        identity_type.clone(),
                                                    ),
                                                    "of",
                                                    vec![member(
                                                        member(ident("__claims"), "value"),
                                                        "sub",
                                                    )],
                                                ),
                                            ),
                                            if_(
                                                strict_eq(
                                                    member(ident("__id"), "tag"),
                                                    str_lit("Ok"),
                                                ),
                                                TsStmt::assign(
                                                    ident("__who"),
                                                    TsExpr::object(vec![
                                                        ("tag".to_string(), str_lit(tag)),
                                                        (
                                                            "identity".to_string(),
                                                            member(ident("__id"), "value"),
                                                        ),
                                                    ]),
                                                    None,
                                                ),
                                            ),
                                        ]),
                                    ),
                                ]),
                            ),
                        ]),
                    )]
                }
                // Rejected at the WebSocket boundary (no body to sign), so a
                // checked program has none; resolve nothing, fail closed.
                // `Signature`, `Oidc` and `SignedCookie` are not WebSocket schemes (the checker
                // refuses them on `from websocket`), so no member of those resolves here.
                SumMemberSeam::Signature(_)
                | SumMemberSeam::Oidc(_)
                | SumMemberSeam::SignedCookie(_) => Vec::new(),
            };
            stmts.push(if_(
                strict_eq(ident("__who"), ident("undefined")),
                block(inner),
            ));
        }
        stmts.push(if_(
            strict_eq(ident("__who"), ident("undefined")),
            return_(Some(new_response(vec![
                str_lit("Unauthorized"),
                status_object("401"),
            ]))),
        ));
    }

    if let Some(seam) = seam {
        let secret = crate::emitter::escape_ts_string(&seam.secret);
        stmts.push(const_(
            "__proto",
            method_call(
                member(ident("request"), "headers"),
                "get",
                vec![str_lit("Sec-WebSocket-Protocol")],
            ),
        ));
        stmts.push(if_(
            strict_eq(ident("__proto"), null_lit()),
            return_(Some(new_response(vec![
                str_lit("Unauthorized"),
                status_object("401"),
            ]))),
        ));
        stmts.push(const_(
            "__token",
            method_call(
                TsExpr::Index {
                    object: Box::new(method_call(ident("__proto"), "split", vec![str_lit(",")])),
                    index: Box::new(num_lit("0")),
                },
                "trim",
                vec![],
            ),
        ));
        // The hosting context's `Env` carries the DO binding (a non-empty object
        // type), so the secret probe casts through `unknown` to index it.
        stmts.push(const_("__secret", secret_probe_expr(&secret)));
        stmts.push(if_(
            strict_neq(typeof_expr(ident("__secret")), str_lit("string")),
            return_(Some(new_response(vec![
                str_lit("Unauthorized"),
                status_object("401"),
            ]))),
        ));
        stmts.push(const_(
            "__claims",
            await_expr(call(
                ident("__verifyBearerJwtHs256"),
                vec![ident("__token"), ident("__secret")],
            )),
        ));
        stmts.push(if_(
            strict_eq(member(ident("__claims"), "tag"), str_lit("Err")),
            return_(Some(new_response(vec![
                str_lit("Unauthorized"),
                status_object("401"),
            ]))),
        ));
        // A refinement actor's authorisation invariant: scheme verified (401
        // above), a failed claim predicate is 403, checked against verified claims.
        if let Some(pred) = &seam.authorization {
            stmts.push(if_(
                super::claim_predicate::claim_predicate_denied(pred),
                return_(Some(new_response(vec![
                    str_lit("Forbidden"),
                    status_object("403"),
                ]))),
            ));
        }
    }

    // DECISION B: resolve the hosting Durable Object from the single connection
    // transfer (`Room(roomId)` → the `ROOM` namespace, keyed by `roomId`). The
    // shape constraint guarantees exactly one routable target, and #2959's key
    // constraint (`bynk.ws.open_transfer_key`) a key the edge can evaluate: a
    // route param, a `const` of the context or of a commons it `uses`, or a
    // literal.
    let internal_error = |mut stmts: Vec<TsStmt>, params: Vec<TsParam>| {
        stmts.push(return_(Some(new_response(vec![
            str_lit("Internal Server Error"),
            status_object("500"),
        ]))));
        TsObjectEntry::method(method_name.clone(), true, params, None, stmts)
    };
    // The checker rejects zero / multiple transfers
    // (`bynk.ws.open_transfer_shape`) and a key the edge cannot evaluate
    // (`bynk.ws.open_transfer_key`); these arms are defensive, and never route
    // to a different Durable Object than the one the body addresses.
    let WsOpenShape::One(target) = analyse_open_shape(&h.body, local_agents) else {
        return internal_error(stmts, params);
    };
    let key = classify_open_key(h, target.key, |n| consts.contains_key(n));
    if matches!(key, WsOpenKey::Other) {
        return internal_error(stmts, params);
    }
    let binding = agent_binding(target.agent).name;
    // The verified identity (when the actor binds one) is forwarded in the trusted
    // internal header alongside the route arguments. A binder-less `by` verifies
    // but mints no identity.
    let has_identity = seam.is_some_and(|s| s.binder.is_some());
    if has_identity {
        let id_ty = &seam.unwrap().identity_type;
        stmts.push(const_(
            "__id",
            method_call(
                member(ident("handlers"), id_ty.clone()),
                "of",
                vec![member(member(ident("__claims"), "value"), "sub")],
            ),
        ));
        stmts.push(if_(
            strict_eq(member(ident("__id"), "tag"), str_lit("Err")),
            return_(Some(new_response(vec![
                str_lit("Unauthorized"),
                status_object("401"),
            ]))),
        ));
    }
    // The route params arrive attacker-controlled (the upgrade URL's query string).
    // Validate each refined / opaque param through its `.of` constructor fail-closed
    // — a `400` with a `RefinementViolation`, exactly as the HTTP path validates a
    // path param — *before* it addresses a Durable Object or is forwarded to the
    // on-open body. A malformed value must never reach the DO typed as though it had
    // satisfied its refinement. (Validation runs after auth, so an unauthenticated
    // client still sees only `401`.)
    // `validated` is keyed by the Bynk param name (so the `key` lookup and the
    // arg walk below match) but its *value* is the real JS *expression* to
    // forward — which references the `ts_ident`-renamed wrapper binder, not
    // the raw name.
    let mut validated: HashMap<String, TsExpr> = HashMap::new();
    for p in &h.params {
        let pn = &p.name.name;
        let jn = wire_ident(pn);
        match &p.type_ref {
            TypeRef::Named(id) => {
                let r_name = format!("__r_{pn}");
                stmts.push(const_(
                    r_name.clone(),
                    method_call(
                        member(ident("handlers"), id.name.clone()),
                        "of",
                        vec![ident(jn)],
                    ),
                ));
                stmts.push(if_(
                    strict_eq(member(ident(r_name.clone()), "tag"), str_lit("Err")),
                    return_(Some(new_response(vec![
                        method_call(
                            ident("globalThis.JSON"),
                            "stringify",
                            // #4278: never the offending value on the wire.
                            vec![call(
                                ident("__wireRefusal"),
                                vec![TsExpr::object(vec![
                                    ("kind".to_string(), str_lit("RefinementViolation")),
                                    ("path".to_string(), str_lit(format!("param.{pn}"))),
                                    (
                                        "violation".to_string(),
                                        member(ident(r_name.clone()), "error"),
                                    ),
                                ])],
                            )],
                        ),
                        TsExpr::object(vec![
                            ("status".to_string(), num_lit("400")),
                            (
                                "headers".to_string(),
                                TsExpr::object(vec![(
                                    // Not a bare identifier — needs its own
                                    // quotes, matching `events_fanout.rs`'s
                                    // own `event_routes_table` convention
                                    // for a pre-quoted, string-literal-
                                    // shaped object key (`TsObjectEntry`'s
                                    // key field is raw text pushed
                                    // verbatim, not itself a `TsExpr`).
                                    "\"content-type\"".to_string(),
                                    str_lit("application/json"),
                                )]),
                            ),
                        ]),
                    ]))),
                ));
                validated.insert(pn.clone(), member(ident(r_name), "value"));
            }
            // A plain `String` param (or a shape the static check already rejected)
            // passes through unchanged.
            TypeRef::Base(..)
            | TypeRef::Result(..)
            | TypeRef::Option(..)
            | TypeRef::Effect(..)
            | TypeRef::HttpResult(..)
            | TypeRef::QueueResult(_)
            | TypeRef::Asset(_)
            | TypeRef::RequestHeaders(_)
            | TypeRef::RequestQuery(_)
            | TypeRef::RequestCookies(_)
            | TypeRef::TokenClaims(_)
            | TypeRef::BoundaryRejection(_)
            | TypeRef::List(..)
            | TypeRef::Map(..)
            | TypeRef::Set(..)
            | TypeRef::Query(..)
            | TypeRef::Stream(..)
            | TypeRef::Connection(..)
            | TypeRef::History(..)
            | TypeRef::ValidationError(_)
            | TypeRef::DecodeError(_)
            | TypeRef::Unit(_)
            | TypeRef::Fn(..)
            | TypeRef::App { .. } => {
                validated.insert(pn.clone(), ident(jn));
            }
        }
    }
    let key_ref = match key {
        // A route param addresses the instance by its validated value.
        WsOpenKey::Param(name) => validated
            .get(name)
            .cloned()
            .unwrap_or_else(|| ident(wire_ident(name))),
        // #2959: a constant is a hoisted function (`emit_consts`). The
        // context's own is exported from `handlers.ts`; #2967: a used
        // commons' is imported from the commons, under an alias.
        WsOpenKey::Const(name) => match consts.get(name).and_then(|c| c.owner) {
            None => call(
                member(ident("handlers"), bynk_check::emitted_names::ts_ident(name)),
                vec![],
            ),
            Some(owner) => {
                let alias = format!("__const_{}_{name}", owner.replace('.', "_"));
                const_imports.insert(alias.clone(), (owner.to_string(), name.to_string()));
                call(ident(alias), vec![])
            }
        },
        WsOpenKey::Literal(e) => open_key_literal(e),
        WsOpenKey::Other => unreachable!("rejected above"),
    };
    let args_json: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| {
            validated
                .get(&p.name.name)
                .cloned()
                .unwrap_or_else(|| ident(wire_ident(&p.name.name)))
        })
        .collect();
    stmts.push(const_(
        "__ns",
        // `member_chain`, not a single `Member { property: "env.{binding}" }`
        // node — the printed text is two chained accesses (`deps.env.<binding>`),
        // and a `format!`-built dotted-property slot would claim it is one
        // property literally named "env.<binding>" (review of #1322, finding 3).
        member_chain(ident("deps"), &["env", &binding]),
    ));
    stmts.push(const_(
        "__stub",
        method_call(
            ident("__ns"),
            "get",
            vec![method_call(
                ident("__ns"),
                "idFromName",
                vec![call(ident("__serialiseAgentKey"), vec![key_ref.clone()])],
            )],
        ),
    ));
    stmts.push(const_(
        "__fwd",
        new(
            ident("globalThis.Headers"),
            vec![member(ident("request"), "headers")],
        ),
    ));
    let mut fwd_entries = vec![("args".to_string(), TsExpr::array(args_json))];
    if has_identity {
        fwd_entries.push(("identity".to_string(), member(ident("__id"), "value")));
    }
    if sum.is_some() {
        fwd_entries.push(("who".to_string(), ident("__who")));
    }
    // #1818: the hosting agent reads its key, so the upgrade carries it, encoded
    // by the agent's key codec.
    if key_readers.contains(target.agent) {
        fwd_entries.push((
            "key".to_string(),
            method_call(
                member(
                    ident("handlers"),
                    crate::emitter::emit::agent_key_codec_name(target.agent),
                ),
                "enc",
                vec![key_ref.clone()],
            ),
        ));
    }
    stmts.push(expr_stmt(method_call(
        ident("__fwd"),
        "set",
        vec![
            str_lit("X-Bynk-Ws-Open"),
            method_call(
                ident("globalThis.JSON"),
                "stringify",
                vec![TsExpr::object(fwd_entries)],
            ),
        ],
    )));
    stmts.push(return_(Some(method_call(
        ident("__stub"),
        "fetch",
        vec![new(
            ident("globalThis.Request"),
            vec![
                str_lit(format!("https://_bynk/_bynk/ws/open/{sname}")),
                TsExpr::object(vec![
                    ("method".to_string(), member(ident("request"), "method")),
                    ("headers".to_string(), ident("__fwd")),
                ]),
            ],
        )],
    ))));

    TsObjectEntry::method(method_name, true, params, None, stmts)
}

/// What a verified actor leaves for the handler: the key a rate limit counts by (the
/// verified `sub`), and the entries added to the handler's `deps` after `...deps`
/// (the identity, the claims, the session). Empty `extras` means the handler gets
/// plain `deps`.
pub(super) struct SeamPrincipal {
    pub(super) key: TsExpr,
    pub(super) extras: Vec<TsObjectEntry>,
}

/// How a verification step reports a refusal: given the `HttpResult` to refuse with,
/// the statement that leaves the step. A wrapper returns it; a `verify` step (#3926)
/// returns it as the `Err` of its outcome.
pub(super) type Failure<'a> = &'a dyn Fn(TsExpr) -> TsStmt;

/// A `verify` step's refusal (#3926): `return { tag: "Err", error: <result> }`.
pub(super) fn err_failure(result: TsExpr) -> TsStmt {
    return_(Some(TsExpr::object(vec![
        ("tag".to_string(), str_lit("Err")),
        ("error".to_string(), result),
    ])))
}

fn refused(failure: Failure, status: &str) -> TsStmt {
    let result = member(ident("HttpResult"), status);
    failure(result)
}

/// The call of the handler a verified wrapper ends with: the route's params, then
/// `deps`, extended by `extras` when the seam leaves a principal.
fn handler_return(
    sname: &str,
    method_key: &str,
    mut param_args: Vec<TsExpr>,
    extras: Vec<TsObjectEntry>,
) -> TsStmt {
    if extras.is_empty() {
        param_args.push(ident("deps"));
    } else {
        let mut entries = vec![TsObjectEntry::Spread(ident("deps"))];
        entries.extend(extras);
        param_args.push(TsExpr::object_entries(entries));
    }
    return_(Some(call(
        member_chain(ident("handlers"), &[sname, method_key]),
        param_args,
    )))
}

#[allow(clippy::vec_init_then_push)]
/// A Bearer actor's verification: the header, the secret, the JWT, the claim predicate
/// (a refinement actor's authorisation invariant, `403`) and the identity mint. Every
/// refusal goes through `failure`, in the order they always ran.
fn bearer_seam_stmts(
    seam: &bynk_check::actors::BearerSeam,
    failure: Failure,
) -> (Vec<TsStmt>, SeamPrincipal) {
    let secret = crate::emitter::escape_ts_string(&seam.secret);
    let mut stmts: Vec<TsStmt> = Vec::new();
    stmts.push(const_(
        "__authz",
        method_call(
            member(ident("request"), "headers"),
            "get",
            vec![str_lit("Authorization")],
        ),
    ));
    stmts.push(if_(
        or_expr(
            strict_eq(ident("__authz"), null_lit()),
            not_expr(method_call(
                ident("__authz"),
                "startsWith",
                vec![str_lit("Bearer ")],
            )),
        ),
        refused(failure, "Unauthorized"),
    ));
    // Source the signing secret from the same env the `Secrets` capability
    // reads (explicit env first, then a `process.env` probe).
    stmts.push(const_("__secret", secret_probe_expr(&secret)));
    stmts.push(if_(
        strict_neq(typeof_expr(ident("__secret")), str_lit("string")),
        refused(failure, "Unauthorized"),
    ));
    stmts.push(const_(
        "__claims",
        await_expr(call(
            ident("__verifyBearerJwtHs256"),
            vec![
                method_call(ident("__authz"), "slice", vec![num_lit("7")]),
                ident("__secret"),
            ],
        )),
    ));
    stmts.push(if_(
        strict_eq(member(ident("__claims"), "tag"), str_lit("Err")),
        refused(failure, "Unauthorized"),
    ));
    // v0.53: a refinement actor's authorisation invariant — the scheme
    // verified (else 401 above), so a failed claim predicate is 403, not
    // 401. Checked against the *verified* claims, before the identity mints
    // or the body runs.
    if let Some(pred) = &seam.authorization {
        stmts.push(if_(
            super::claim_predicate::claim_predicate_denied(pred),
            refused(failure, "Forbidden"),
        ));
    }
    let mut extras: Vec<TsObjectEntry> = Vec::new();
    if seam.binder.is_some() {
        // Capture the identity: construct the declared type from `sub`
        // (fail-closed on a refinement violation) and thread it into deps.
        stmts.push(const_(
            "__id",
            method_call(
                member(ident("handlers"), seam.identity_type.clone()),
                "of",
                vec![member(member(ident("__claims"), "value"), "sub")],
            ),
        ));
        stmts.push(if_(
            strict_eq(member(ident("__id"), "tag"), str_lit("Err")),
            refused(failure, "Unauthorized"),
        ));
        extras.push(TsObjectEntry::prop(
            "identity".to_string(),
            member(ident("__id"), "value"),
        ));
        // #3937: and the verified token's claims, which the binder's `.claims` reads.
        extras.push(TsObjectEntry::prop(
            "claims".to_string(),
            call(
                ident("__tokenClaims"),
                vec![member(member(ident("__claims"), "value"), "claims")],
            ),
        ));
    }
    (
        stmts,
        SeamPrincipal {
            key: member(member(ident("__claims"), "value"), "sub"),
            extras,
        },
    )
}

// `stmts`/`secret_body` in the functions below are built incrementally —
// unconditional pushes followed by later conditional ones, not a fixed
// literal clippy's suggested `vec![]` could replace.
#[allow(clippy::vec_init_then_push)]
pub(super) fn emit_http_wrapper(
    sname: &str,
    h: &Handler,
    method: bynk_model::IrHttpMethod,
    path: &str,
    seam: Option<&bynk_check::actors::BearerSeam>,
) -> TsObjectEntry {
    let method_key = http_handler_method_name_ir(method, path);
    // Route params (and the `body`) forward positionally; `ts_ident` keeps a
    // reserved-word param name from emitting an invalid binder (#723).
    let param_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(wire_ident(&p.name.name)))
        .collect();

    // v0.47: a Bearer handler's wrapper takes the request, runs the fail-closed
    // verification seam, mints the identity, and threads it into `deps`. The
    // boundary owns `env` (the secret source) and `deps`, so the whole seam is
    // one cohesive block here; any failure returns `Unauthorized` (401), which
    // the entry's `httpResultToResponse` maps. The body never runs unverified.
    if let Some(seam) = seam {
        // P7.2: real declared types, not `unknown` — a first attempt assumed
        // these were raw, uncoerced strings like `emit_websocket_upgrade`'s own
        // params, but broke real `tsc --strict` fixtures: `workers_entry.rs`'s
        // own dispatch validates each path param against its declared type
        // (a `Named` type via `.of(...)`, a bare `String` trivially) before
        // ever calling into this wrapper, so the real type is what's actually
        // known here.
        let mut params: Vec<TsParam> = vec![TsParam::new(
            "request".to_string(),
            Some(TsType::named("globalThis.Request")),
        )];
        params.extend(h.params.iter().map(|p| {
            TsParam::new(
                wire_ident(&p.name.name),
                Some(qualified_ts_type_ref(&p.type_ref)),
            )
        }));
        let (mut stmts, principal) = bearer_seam_stmts(seam, &|result| return_(Some(result)));
        stmts.push(handler_return(
            sname,
            &method_key,
            param_args,
            principal.extras,
        ));
        return TsObjectEntry::Method {
            name: method_key,
            is_async: true,
            generics: Vec::new(),
            params,
            return_type: None,
            doc: None,
            inline: false,
            body: stmts,
            span: None,
        };
    }

    // P7.2: real declared types — same correction as the Bearer branch above.
    let params: Vec<TsParam> = h
        .params
        .iter()
        .map(|p| {
            TsParam::new(
                wire_ident(&p.name.name),
                Some(qualified_ts_type_ref(&p.type_ref)),
            )
        })
        .collect();
    let mut call_args = param_args;
    call_args.push(ident("deps"));
    TsObjectEntry::Method {
        name: method_key.clone(),
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: None,
        doc: None,
        inline: false,
        body: vec![return_(Some(call(
            member_chain(ident("handlers"), &[sname, &method_key]),
            call_args,
        )))],
        span: None,
    }
}

/// #3926: a route with `@rateLimit(Limiter, key: identity)` is served by two compose
/// methods in place of the one wrapper, so the entry can verify the caller, ask the
/// limiter about them, and only then read the body. `<route>__verify(request)` runs the
/// seam's own statements, refusals included, and gives `{ tag: "Err", error }` (the
/// `HttpResult` the wrapper would have returned) or `{ tag: "Ok", value: { key,
/// principal } }`; `<route>(params…, principal)` runs the handler with the principal
/// the verification left. Verification runs once: the entry never calls the wrapper's
/// own seam again.
fn identity_split_wrapper(
    sname: &str,
    h: &Handler,
    method: bynk_model::IrHttpMethod,
    path: &str,
    (mut stmts, principal): (Vec<TsStmt>, SeamPrincipal),
) -> Vec<TsObjectEntry> {
    let method_key = http_handler_method_name_ir(method, path);
    let verify_return = TsType::named(
        "globalThis.Promise<{ tag: \"Err\"; error: unknown } | { tag: \"Ok\"; value: { key: string; principal: object } }>",
    );
    stmts.push(return_(Some(TsExpr::object(vec![
        ("tag".to_string(), str_lit("Ok")),
        (
            "value".to_string(),
            TsExpr::object(vec![
                (
                    "key".to_string(),
                    call(ident("globalThis.String"), vec![principal.key]),
                ),
                (
                    "principal".to_string(),
                    TsExpr::object_entries(principal.extras),
                ),
            ]),
        ),
    ]))));
    let verify = TsObjectEntry::Method {
        name: format!("{method_key}__verify"),
        is_async: true,
        generics: Vec::new(),
        params: vec![TsParam::new(
            "request".to_string(),
            Some(TsType::named("globalThis.Request")),
        )],
        return_type: Some(verify_return),
        doc: None,
        inline: false,
        body: stmts,
        span: None,
    };
    let mut params: Vec<TsParam> = h
        .params
        .iter()
        .map(|p| {
            TsParam::new(
                wire_ident(&p.name.name),
                Some(qualified_ts_type_ref(&p.type_ref)),
            )
        })
        .collect();
    params.push(TsParam::new(
        "principal".to_string(),
        Some(TsType::named("object")),
    ));
    let mut args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(wire_ident(&p.name.name)))
        .collect();
    args.push(TsExpr::object_entries(vec![
        TsObjectEntry::Spread(ident("deps")),
        TsObjectEntry::Spread(ident("principal")),
    ]));
    let run = TsObjectEntry::Method {
        name: method_key.clone(),
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: None,
        doc: None,
        inline: false,
        body: vec![return_(Some(call(
            member_chain(ident("handlers"), &[sname, &method_key]),
            args,
        )))],
        span: None,
    };
    vec![verify, run]
}

/// #3926: the verify/run pair of an identity-limited `Bearer` route.
pub(super) fn bearer_identity_split(
    sname: &str,
    h: &Handler,
    method: bynk_model::IrHttpMethod,
    path: &str,
    seam: &bynk_check::actors::BearerSeam,
) -> Vec<TsObjectEntry> {
    identity_split_wrapper(
        sname,
        h,
        method,
        path,
        bearer_seam_stmts(seam, &err_failure),
    )
}

/// #3926: the verify/run pair of an identity-limited `Oidc` route.
pub(super) fn oidc_identity_split(
    sname: &str,
    h: &Handler,
    method: bynk_model::IrHttpMethod,
    path: &str,
    seam: &bynk_check::actors::OidcSeam,
) -> Vec<TsObjectEntry> {
    identity_split_wrapper(sname, h, method, path, oidc_seam_stmts(seam, &err_failure))
}

/// #3926: the verify/run pair of an identity-limited `SignedCookie` route. The seam's own
/// statements run in `verify`: the cross-site check for a state-changing request, the
/// cookie, its signature and lifetime, and with `sessions = true` the session-live check.
pub(super) fn signed_cookie_identity_split(
    service: &ServiceDecl,
    sname: &str,
    h: &Handler,
    method: bynk_model::IrHttpMethod,
    path: &str,
    seam: &bynk_check::actors::SignedCookieSeam,
) -> Vec<TsObjectEntry> {
    let stmts = signed_cookie_seam_stmts(method, seam, &csrf_origins(service), &err_failure);
    identity_split_wrapper(sname, h, method, path, stmts)
}

/// An `Oidc` actor's verification: the header, the trust roots (`trustFrom` replaces the
/// declared ones, fail closed), the JWT against the provider's JWKS and the identity mint.
/// Every refusal goes through `failure`, in the order they always ran.
fn oidc_seam_stmts(
    seam: &bynk_check::actors::OidcSeam,
    failure: Failure,
) -> (Vec<TsStmt>, SeamPrincipal) {
    let issuer = crate::emitter::escape_ts_string(&seam.issuer);
    let audience = crate::emitter::escape_ts_string(&seam.audience);
    let jwks = crate::emitter::escape_ts_string(&seam.jwks);
    let mut stmts: Vec<TsStmt> = Vec::new();
    stmts.push(const_(
        "__authz",
        method_call(
            member(ident("request"), "headers"),
            "get",
            vec![str_lit("Authorization")],
        ),
    ));
    stmts.push(if_(
        or_expr(
            strict_eq(ident("__authz"), null_lit()),
            not_expr(method_call(
                ident("__authz"),
                "startsWith",
                vec![str_lit("Bearer ")],
            )),
        ),
        refused(failure, "Unauthorized"),
    ));
    // #2380 (L5): `trustFrom`'s variable, when set, replaces the declared
    // trust roots; set but malformed refuses the request (fail closed).
    let (issuer, audience, jwks) = match &seam.trust_from {
        Some(var) => {
            stmts.push(const_(
                "__trust",
                call(
                    ident("__oidcTrust"),
                    vec![
                        secret_probe_expr(&crate::emitter::escape_ts_string(var)),
                        str_lit(issuer),
                        str_lit(audience),
                        str_lit(jwks),
                    ],
                ),
            ));
            stmts.push(if_(
                strict_eq(ident("__trust"), null_lit()),
                refused(failure, "Unauthorized"),
            ));
            (
                member(ident("__trust"), "issuer"),
                member(ident("__trust"), "audience"),
                member(ident("__trust"), "jwks"),
            )
        }
        None => (str_lit(issuer), str_lit(audience), str_lit(jwks)),
    };
    stmts.push(const_(
        "__claims",
        await_expr(call(
            ident("__verifyOidcJwt"),
            vec![
                method_call(ident("__authz"), "slice", vec![num_lit("7")]),
                issuer,
                audience,
                jwks,
            ],
        )),
    ));
    stmts.push(if_(
        strict_eq(member(ident("__claims"), "tag"), str_lit("Err")),
        refused(failure, "Unauthorized"),
    ));
    let mut extras: Vec<TsObjectEntry> = Vec::new();
    if seam.binder.is_some() {
        // Capture the identity: construct the declared type from the verified
        // `sub` (fail-closed on a refinement violation) and thread it into deps.
        stmts.push(const_(
            "__id",
            method_call(
                member(ident("handlers"), seam.identity_type.clone()),
                "of",
                vec![member(member(ident("__claims"), "value"), "sub")],
            ),
        ));
        stmts.push(if_(
            strict_eq(member(ident("__id"), "tag"), str_lit("Err")),
            refused(failure, "Unauthorized"),
        ));
        // #2380 (L5): the binder's `.claims` read the verified token's claims.
        extras.push(TsObjectEntry::prop(
            "identity".to_string(),
            member(ident("__id"), "value"),
        ));
        extras.push(TsObjectEntry::prop(
            "claims".to_string(),
            call(
                ident("__tokenClaims"),
                vec![member(member(ident("__claims"), "value"), "claims")],
            ),
        ));
    }
    (
        stmts,
        SeamPrincipal {
            key: member(member(ident("__claims"), "value"), "sub"),
            extras,
        },
    )
}

/// v0.151: the compose wrapper for a single-actor `Oidc` HTTP handler. It reads
/// the `Authorization: Bearer <token>`, verifies the JWT against the provider's
/// JWKS (RS256/ES256, `iss`/`aud`/`exp`/`nbf`) via `verifyOidcJwt`, mints the
/// identity from the verified `sub`, and threads it into `deps` — all before the
/// body runs. Any failure returns `Unauthorized` (401), fail-closed. Unlike the
/// Bearer wrapper it sources **no secret**: the trust parameters are the public
/// `issuer`/`audience`/`jwks` literals from the actor declaration.
#[allow(clippy::vec_init_then_push)]
pub(super) fn emit_http_oidc_wrapper(
    sname: &str,
    h: &Handler,
    method: bynk_model::IrHttpMethod,
    path: &str,
    seam: &bynk_check::actors::OidcSeam,
) -> TsObjectEntry {
    let method_key = http_handler_method_name_ir(method, path);
    let param_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(wire_ident(&p.name.name)))
        .collect();
    // P7.2: real declared types — same correction as `emit_http_wrapper`'s own.
    let mut params: Vec<TsParam> = vec![TsParam::new(
        "request".to_string(),
        Some(TsType::named("globalThis.Request")),
    )];
    params.extend(h.params.iter().map(|p| {
        TsParam::new(
            wire_ident(&p.name.name),
            Some(qualified_ts_type_ref(&p.type_ref)),
        )
    }));
    let (mut stmts, principal) = oidc_seam_stmts(seam, &|result| return_(Some(result)));
    stmts.push(handler_return(
        sname,
        &method_key,
        param_args,
        principal.extras,
    ));
    TsObjectEntry::Method {
        name: method_key,
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: None,
        doc: None,
        inline: false,
        body: stmts,
        span: None,
    }
}

/// #3386: `deps.__session`, the signing keys of this context's `SignedCookie` actors by name,
/// read from the environment, for a handler's `Actor.issue(identity)`. `None` when it has none.
pub(super) fn session_deps_entry(table: &UnitTable) -> Option<TsObjectEntry> {
    let mut actors: Vec<_> = table
        .actors
        .values()
        .filter_map(|a| bynk_check::actors::signed_cookie_seam_of(a, None))
        .collect();
    if actors.is_empty() {
        return None;
    }
    actors.sort_by(|a, b| a.actor.cmp(&b.actor));
    let probe = |var: &str| secret_probe_expr(&crate::emitter::escape_ts_string(var));
    let keys = actors
        .iter()
        .map(|s| {
            // A legitimate default: no previous secret is `undefined`.
            let previous = s.previous.as_deref().map_or_else(undefined_node, probe);
            (
                crate::emitter::escape_ts_string(&s.actor),
                TsExpr::object(vec![
                    ("secret".to_string(), probe(&s.secret)),
                    ("previous".to_string(), previous),
                ]),
            )
        })
        .collect();
    Some(TsObjectEntry::prop(
        "__session".to_string(),
        TsExpr::object(keys),
    ))
}

/// #3386: the compose entry for one `SignedCookie` handler. A credentialed `cors` origin is one
/// the CSRF check trusts.
pub(super) fn signed_cookie_entry(
    service: &ServiceDecl,
    sname: &str,
    h: &Handler,
    method: bynk_model::IrHttpMethod,
    path: &str,
    seam: &bynk_check::actors::SignedCookieSeam,
) -> TsObjectEntry {
    emit_http_signed_cookie_wrapper(sname, h, method, path, seam, &csrf_origins(service))
}

/// The origins a `SignedCookie` arm's CSRF check trusts: the service's credentialed `cors`
/// origins (never a wildcard).
pub(super) fn csrf_origins(service: &ServiceDecl) -> Vec<String> {
    service
        .cors
        .as_ref()
        .filter(|c| c.credentials() && !c.is_wildcard())
        .map(|c| c.origins())
        .unwrap_or_default()
}

/// A `SignedCookie` actor's verification: the cross-site check for a state-changing
/// request, the cookie, its signature and lifetime, the identity mint and, for a
/// `sessions = true` actor, the session-live check. Every refusal goes through
/// `failure`, in the order they always ran, so nothing here is skipped when the step
/// runs ahead of the body.
fn signed_cookie_seam_stmts(
    method: bynk_model::IrHttpMethod,
    seam: &bynk_check::actors::SignedCookieSeam,
    csrf_origins: &[String],
    failure: Failure,
) -> (Vec<TsStmt>, SeamPrincipal) {
    let mut stmts: Vec<TsStmt> = Vec::new();
    if !matches!(method, bynk_model::IrHttpMethod::Get) {
        let origins: Vec<TsExpr> = csrf_origins
            .iter()
            .map(|o| str_lit(crate::emitter::escape_ts_string(o)))
            .collect();
        stmts.push(if_(
            not_expr(call(
                ident("__csrfAllowed"),
                vec![ident("request"), TsExpr::array(origins)],
            )),
            refused(failure, "Forbidden"),
        ));
    }
    stmts.push(const_(
        "__cookie",
        call(
            ident("__sessionCookieOf"),
            vec![
                ident("request"),
                str_lit(crate::emitter::escape_ts_string(&seam.cookie)),
            ],
        ),
    ));
    stmts.push(if_(
        strict_eq(ident("__cookie"), null_lit()),
        refused(failure, "Unauthorized"),
    ));
    let previous = match &seam.previous {
        Some(var) => secret_probe_expr(&crate::emitter::escape_ts_string(var)),
        // A legitimate default: no previous secret is `undefined`.
        None => undefined_node(),
    };
    stmts.push(const_(
        "__session",
        await_expr(call(
            ident("__verifySignedCookie"),
            vec![
                ident("__cookie"),
                TsExpr::object(vec![
                    (
                        "secret".to_string(),
                        secret_probe_expr(&crate::emitter::escape_ts_string(&seam.secret)),
                    ),
                    ("previous".to_string(), previous),
                ]),
                str_lit(crate::emitter::escape_ts_string(&seam.actor)),
                num_lit(seam.ttl_secs.to_string()),
            ],
        )),
    ));
    stmts.push(if_(
        strict_eq(member(ident("__session"), "tag"), str_lit("Err")),
        refused(failure, "Unauthorized"),
    ));
    let mut extras: Vec<TsObjectEntry> = Vec::new();
    if seam.binder.is_some() || seam.sessions {
        // Capture the identity: construct the declared type from the verified `sub`
        // (fail-closed on a refinement violation) and thread it into deps. A `sessions = true`
        // actor needs it even without a binder: it keys the session agent.
        stmts.push(const_(
            "__id",
            method_call(
                member(ident("handlers"), seam.identity_type.clone()),
                "of",
                vec![member(member(ident("__session"), "value"), "sub")],
            ),
        ));
        stmts.push(if_(
            strict_eq(member(ident("__id"), "tag"), str_lit("Err")),
            refused(failure, "Unauthorized"),
        ));
        if seam.sessions {
            push_session_check(&mut stmts, seam, failure);
            // The verified session, for `Actor.expire()` to end. Spread from its own object so the
            // handler's exact `deps` type does not flag it as an excess property.
            extras.push(TsObjectEntry::Spread(TsExpr::object(vec![(
                "__sessionOf".to_string(),
                TsExpr::object(vec![
                    (
                        "sid".to_string(),
                        member(member(ident("__session"), "value"), "sid"),
                    ),
                    (
                        "sub".to_string(),
                        member(member(ident("__session"), "value"), "sub"),
                    ),
                ]),
            )])));
        }
        if seam.binder.is_some() {
            extras.push(TsObjectEntry::prop(
                "identity".to_string(),
                member(ident("__id"), "value"),
            ));
        }
    }
    (
        stmts,
        SeamPrincipal {
            key: member(member(ident("__session"), "value"), "sub"),
            extras,
        },
    )
}

/// #3386: the compose wrapper for a single-actor `SignedCookie` HTTP handler. Before the body
/// runs it (1) refuses a cross-site state-changing request (fetch metadata, then `Origin`;
/// a `GET` is safe and skips this), (2) reads the session cookie, refusing an absent or
/// repeated one, (3) verifies it with `verifySignedCookie` (HMAC-SHA-256, `kid`-selected key,
/// `aud` = this actor, expiry, lifetime within the actor's `ttl`) and (4) mints the identity
/// from the verified `sub`. Any failure is `Unauthorized` (401), a CSRF failure `Forbidden`
/// (403), both fail-closed; a bad key is a thrown `SessionKeyFault` (a faulted request), never
/// a quiet accept. The credential is verified here and never reaches the handler.
#[allow(clippy::vec_init_then_push)]
fn emit_http_signed_cookie_wrapper(
    sname: &str,
    h: &Handler,
    method: bynk_model::IrHttpMethod,
    path: &str,
    seam: &bynk_check::actors::SignedCookieSeam,
    csrf_origins: &[String],
) -> TsObjectEntry {
    let method_key = http_handler_method_name_ir(method, path);
    let param_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(wire_ident(&p.name.name)))
        .collect();
    let mut params: Vec<TsParam> = vec![TsParam::new(
        "request".to_string(),
        Some(TsType::named("globalThis.Request")),
    )];
    params.extend(h.params.iter().map(|p| {
        TsParam::new(
            wire_ident(&p.name.name),
            Some(qualified_ts_type_ref(&p.type_ref)),
        )
    }));
    let (mut stmts, principal) =
        signed_cookie_seam_stmts(method, seam, csrf_origins, &|result| return_(Some(result)));
    stmts.push(handler_return(
        sname,
        &method_key,
        param_args,
        principal.extras,
    ));
    TsObjectEntry::Method {
        name: method_key,
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: None,
        doc: None,
        inline: false,
        body: stmts,
        span: None,
    }
}

/// #3938: the seam's session check, after the signature, `kid`, audience and expiry have passed
/// and the identity is minted: the cookie must carry a session id (a `sessions = true` actor refuses
/// one that does not, so a cookie from before the option was set fails closed) and the identity's
/// session agent must still hold it live. A forged cookie never reaches here, so costs no agent
/// call. A revoked, expired or unknown session is `Unauthorized`; an agent that cannot answer
/// faults the request, which also refuses it.
fn push_session_check(
    stmts: &mut Vec<TsStmt>,
    seam: &bynk_check::actors::SignedCookieSeam,
    failure: Failure,
) {
    let sid = member(member(ident("__session"), "value"), "sid");
    stmts.push(if_(
        strict_eq(sid.clone(), ident("undefined")),
        refused(failure, "Unauthorized"),
    ));
    let agent = bynk_check::actors::sessions_agent_name(&seam.actor);
    let factory = member(
        ident("handlers"),
        crate::emitter::agent_factory_name(&agent),
    );
    stmts.push(const_(
        "__live",
        await_expr(method_call(
            call(factory, vec![member(ident("__id"), "value"), ident("env")]),
            "isLive",
            vec![
                sid,
                method_call(member(ident("globalThis"), "Date"), "now", vec![]),
                ident("deps"),
            ],
        )),
    ));
    stmts.push(if_(
        not_expr(ident("__live")),
        refused(failure, "Unauthorized"),
    ));
}

/// Source a string secret from the same env the `Secrets` capability reads
/// (explicit `env` first, then a `process.env` probe), binding it to `var`.
pub(super) fn emit_secret_lookup(var: &str, secret: &str) -> TsStmt {
    let secret = crate::emitter::escape_ts_string(secret);
    const_(var, secret_probe_expr(&secret))
}

/// v0.176 (#642): the user-named types appearing anywhere in a type-ref, including
/// through generic arguments — the set a compose wrapper qualifies against the
/// `handlers` namespace.
pub(crate) fn named_types_in(r: &TypeRef) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(r: &TypeRef, out: &mut Vec<String>) {
        match r {
            TypeRef::Named(id) => out.push(id.name.clone()),
            TypeRef::App { name, args, .. } => {
                out.push(name.name.clone());
                for a in args {
                    walk(a, out);
                }
            }
            TypeRef::Result(a, b, _) => {
                walk(a, out);
                walk(b, out);
            }
            TypeRef::Map(k, v, _) => {
                walk(k, out);
                walk(v, out);
            }
            TypeRef::Option(a, _)
            | TypeRef::List(a, _)
            | TypeRef::Set(a, _)
            | TypeRef::Effect(a, _)
            | TypeRef::HttpResult(a, _) => walk(a, out),
            TypeRef::Base(..)
            | TypeRef::QueueResult(_)
            | TypeRef::Asset(_)
            | TypeRef::RequestHeaders(_)
            | TypeRef::RequestQuery(_)
            | TypeRef::RequestCookies(_)
            | TypeRef::TokenClaims(_)
            | TypeRef::BoundaryRejection(_)
            | TypeRef::Query(..)
            | TypeRef::Stream(..)
            | TypeRef::Connection(..)
            | TypeRef::History(..)
            | TypeRef::ValidationError(_)
            | TypeRef::DecodeError(_)
            | TypeRef::Unit(_)
            | TypeRef::Fn(..) => {}
        }
    }
    walk(r, &mut out);
    out
}

/// P7.2: [`crate::emitter::ts_types::ts_type_ref_qualified_ts_type`] over `r`, scoped
/// to `r`'s own named types — the minimal correct qualification for a single
/// type-ref rendered in isolation, the same helper `emit_call_wrapper` uses
/// across a whole param list. A bare `ts_type_ref` collides with anything of
/// the same name already in scope where the rendered text lands: a
/// `handlers.ts`-exported Bynk type (`Cannot find name` — the name resolves
/// to nothing without the `handlers.` prefix) or, for a common enough name,
/// a browser-ambient DOM global (`Notification`, `Event`) that silently wins
/// instead. Both broke real `tsc --strict` fixtures before this existed.
///
/// #1321: renamed from `qualified_type_ref` (returned `String`) — every real
/// caller now builds a `TsProgram` directly, so this returns the `TsType`
/// [`crate::emitter::ts_types::ts_type_ref_qualified_ts_type`] (Decision B) itself
/// builds, not its printed text. That function originally had a
/// `String`-returning sibling, `ts_type_ref_qualified`, kept for a
/// still-`String`-based caller elsewhere in the crate; Arc C slice 32
/// (#1399) converted that last caller and deleted it — this function's own
/// call site was never affected, since it always used the `TsType`-returning
/// form.
pub(super) fn qualified_ts_type_ref(r: &TypeRef) -> TsType {
    let scope: HashSet<String> = named_types_in(r).into_iter().collect();
    crate::emitter::ts_types::ts_type_ref_qualified_ts_type(r, &scope, "handlers")
}
