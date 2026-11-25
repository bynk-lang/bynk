//! Per-Worker `index.ts` generation (v0.8 §4.3, v0.9 §5.1).
//!
//! Each Worker exports a `default { fetch }` handler that first checks
//! for the internal Service Binding prefix (`/_bynk/call/`) and dispatches
//! to the matching `on call` service operation, then matches the request
//! method + path against any `on http` routes the context declares.
//! Validation and uncaught errors map to 400 / 500 respectively.
//!
//! Arc C slice 4 (#1323): `emit_worker_entry` and its four helpers build a
//! real `bynk_ts::TsProgram` directly instead of `writeln!`-ing a `String` —
//! this file's own conversion, closing the fourth Arc C slice (`workers.rs`
//! was the third, #1321).

use bynk_check::symbols::UnitTable;

use crate::emitter::RuntimeUse;
use crate::ts_build::{
    await_expr, call, cond_expr, const_, const_typed, expr_stmt, ident, member, method_call, new,
    return_, str_lit,
};
use bynk_ts::{
    TsBindingName, TsDecl, TsExpr, TsObjectEntry, TsParam, TsProgram, TsStmt, TsType, TsTypeMember,
};

mod admin;
mod call;
mod email;
mod events;
mod field_specs;
mod http;
mod http_policy;
mod queue;
mod respond;
mod scheduled;
pub(crate) mod trace_scope;
mod websocket;
mod wire;

pub(crate) use admin::INSPECT_PREFIX;
use admin::{admin_routes, inspect_registry_export};
use call::{FOREIGN_CALLERS, call_dispatch};
use events::events_delivery_route;
pub use field_specs::{
    field_spec_source, form_body_specs, renamed_specs_source, table_has_plain_json,
};
pub(crate) use field_specs::{query_leaf_kind, record_field_specs};
use http::{collect_http_routes, emit_http_route_dispatch};
use http_policy::{
    CorsService, PathMethods, SecurityService, build_cors_services, build_path_method_table,
    build_security_services, cors_preflight, method_fall_through,
};
pub(crate) use queue::dead_letter_binding;
use queue::{collect_queue_routes, push_message_entries};
use respond::{
    MISS_HELPER, agent_busy_answer, fault_detail, fault_helpers, miss, report_fault, text_response,
    try_catch_stmt,
};
use scheduled::{collect_cron_routes, emit_scheduled_handler};
use websocket::{collect_ws_open_routes, ws_upgrade_dispatch};
pub(crate) use wire::{deserialise_call, wire_ident};

/// What `fetch`'s catch does with a fault (#1825): log it, report it, and answer
/// the bare 500. #3580: a traced build also hands the fault to the request's record.
fn fault_handler(context: &str, trace: bool) -> Vec<TsStmt> {
    // #3932: `AgentBusy` is load, not a fault: it answers `503` with `Retry-After` and is neither
    // logged here (the queue logged it where it gave up) nor reported as a fault.
    let mut handler = agent_busy_answer();
    handler.push(expr_stmt(method_call(
        ident("globalThis.console"),
        "error",
        vec![
            // #3895: inside a trace the line names it.
            call(
                ident("__withTrace"),
                vec![TsExpr::template_lit(
                    vec![format!("{context} "), " faulted".to_string()],
                    vec![ident("__route")],
                )],
            ),
            fault_detail("e"),
        ],
    )));
    if trace {
        handler.push(expr_stmt(call(ident("__traceFault"), vec![ident("e")])));
    }
    handler.push(report_fault());
    handler.push(return_(Some(call(
        ident("__faultResponse"),
        vec![str_lit(context.to_string()), ident("__route")],
    ))));
    handler
}

/// #3580: a traced build moves the dispatch (with any edge-cache guard) into
/// `__bynkTracedFetch`, and `fetch` serves each request under a record that
/// prints one `__bynk_trace:` line as the response is ready. Returns `fetch`'s
/// new body.
fn traced_fetch(
    context: &str,
    header_decls: &mut Vec<TsStmt>,
    fetch_params: &[TsParam],
    fetch_return: &Option<TsType>,
    fetch_body: Vec<TsStmt>,
) -> Vec<TsStmt> {
    header_decls.push(TsStmt::decl(
        TsDecl::Function {
            name: "__bynkTracedFetch".to_string(),
            generics: Vec::new(),
            params: fetch_params.to_vec(),
            return_type: fetch_return.clone(),
            body: fetch_body,
            is_async: true,
            inline: false,
        },
        None,
    ));
    vec![return_(Some(call(
        ident("__traceFetch"),
        vec![
            str_lit(context.to_string()),
            ident("__bynkTracedFetch"),
            ident("request"),
            ident("env"),
            ident("ctx"),
        ],
    )))]
}

/// #3386: the cookie a route's `SignedCookie` seam reads, if it has one.
fn signed_cookie_name(
    h: &bynk_syntax::ast::Handler,
    actors: &std::collections::HashMap<String, bynk_syntax::ast::ActorDecl>,
) -> Option<String> {
    if let Some(members) = bynk_check::actors::sum_members_for(h, actors) {
        // #3936: a sum's cookie arm (a sum has at most one, as peers differ by scheme).
        return members.into_iter().find_map(|m| {
            if let bynk_check::actors::SumMemberSeam::SignedCookie(s) = m.seam {
                Some(s.cookie)
            } else {
                None
            }
        });
    }
    bynk_check::actors::signed_cookie_seam_for(h, actors).map(|s| s.cookie)
}

/// #3994: the credential headers a route's seam consumes, which the handler's `RequestHeaders`
/// omits: `authorization` for a `Bearer` or `Oidc` actor, the signature header for a `Signature`
/// one. An actor sum hides what any member reads, because the entry builds the view before the
/// sum's wrapper resolves which member matched.
fn consumed_credential_headers(
    h: &bynk_syntax::ast::Handler,
    actors: &std::collections::HashMap<String, bynk_syntax::ast::ActorDecl>,
) -> Vec<String> {
    bynk_check::actors::consumed_credential_headers(h, actors)
        .into_iter()
        .map(|(header, _)| header)
        .collect()
}

/// #3477: each container's Durable Object class lives in `containers.ts`
/// (generated by `emit_containers_ts`) and is re-exported from the entry the
/// same way an agent's is.
///
/// A Worker group's members do not re-export them: `containers.ts` belongs to
/// the Worker, so the group's entry exports the classes of all its members once.
fn container_class_reexport(table: &UnitTable, member_mode: bool) -> Option<TsStmt> {
    if member_mode {
        return None;
    }
    let mut names: Vec<String> = table
        .derived
        .iter()
        .filter(|(_, d)| d.is_container())
        .map(|(capability, _)| bynk_check::containers::container_class_name(capability))
        .collect();
    names.sort();
    (!names.is_empty()).then(|| {
        TsStmt::decl(
            TsDecl::ReExport {
                names,
                from: "./containers.js".to_string(),
            },
            None,
        )
    })
}

/// L7 (#2414, Decision C): an opted-in Worker's `fetch`, which runs what
/// `__bynkFetch` answers through `edgeCacheGuard`. The guard can answer with a
/// copy, which would drop a member's miss mark, so a member re-marks the copy
/// when the answer it guarded was a miss.
pub(super) fn edge_cache_return(member_mode: bool) -> Vec<TsStmt> {
    let answer = await_expr(call(
        ident("__bynkFetch"),
        vec![ident("request"), ident("env"), ident("ctx")],
    ));
    let guarded =
        |response: TsExpr| call(ident("__edgeCacheGuard"), vec![ident("request"), response]);
    if !member_mode {
        return vec![return_(Some(guarded(answer)))];
    }
    vec![
        const_("__answer", answer),
        const_("__guarded", guarded(ident("__answer"))),
        return_(Some(cond_expr(
            call(ident("isMiss"), vec![ident("__answer")]),
            call(ident("__miss"), vec![ident("__guarded")]),
            ident("__guarded"),
        ))),
    ]
}

/// Events track, slice 0 (spine #936, ADR 0284): the re-export of the fan-out DO,
/// which lives in its own file (`events_fanout.ts`, not `handlers.ts`; a fan-out
/// DO has no backing `AgentDecl` for `emit_agent` to emit it from). A group's one
/// fan-out is exported by the group's entry, not a member's (#3562).
fn fanout_reexport(
    context: &str,
    layout: &bynk_project::worker_layout::WorkerLayout,
    wanted: bool,
) -> Option<TsStmt> {
    wanted.then(|| {
        TsStmt::decl(
            TsDecl::ReExport {
                names: vec![crate::emitter::wrangler::EVENTS_FANOUT_CLASS_NAME.to_string()],
                from: layout.own_module_specifier(context, "events_fanout.ts"),
            },
            None,
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub fn emit_worker_entry(
    context: &str,
    layout: &bynk_project::worker_layout::WorkerLayout,
    table: &UnitTable,
    // v0.177 (#643): this context's own contract hash per `on call` service.
    own_contracts: &std::collections::BTreeMap<String, String>,
    // #2554: the older contract hashes each service also accepts, from the
    // contracts lock. Absent for a service that accepts only its own.
    accepted_contracts: &std::collections::BTreeMap<String, Vec<String>>,
    // Locale capability track, slice 2 (#882): whether `compose` was widened
    // to take an optional `request` (the bool `emit_worker_compose` itself
    // returned) — only `fetch` has a `Request` in scope, never
    // `scheduled`/`queue`, so the two entry points need distinct compose
    // calls, not one shared string.
    needs_locale_request: bool,
    // #1187's slice 6 plumbing — see `emit_worker_compose`'s own matching
    // parameter (`emitter/workers.rs`) for the full grounding.
    uses_emit: bool,
    // #2136 (ADR DECISION B): the test build of a context a `system` case
    // drives. Its `scheduled` entry stores each cron handler's `Result` on the
    // execution-context double the harness supplies, so the case's
    // `svc.schedule(…)` yields the handler's own result. Never set for a
    // deployable build: that build carries no suites.
    scheduled_result_hook: bool,
    // #2252: the types this context takes from a consumed *context* (not
    // declared here). Each imports type-only (#661), so the entry validates a
    // path parameter of one with this context's own codec, not `<T>.of`.
    consumed_types: &std::collections::HashSet<String>,
    // State-migrations S4 (#2259 Decision D): each versioned agent class this
    // Worker hosts, with its lock version, for the `/_bynk/migrate/` route.
    // Empty when the build has no lock or the context hosts no agent.
    migratable: &[(String, i64)],
    // #2719: whether any of this context's service handlers hands work to the
    // execution context, from the checker's needs-context set.
    ctx_uses_send: bool,
    // #2903: the context's own types plus those of the commons it `uses`, so a
    // query field typed by an imported refined type or enum resolves to its real
    // wire kind rather than falling through to `string`.
    visible_types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
    // #3580: wrap `fetch` in the request recorder (`bynk dev --trace-events` only).
    trace: bool,
    // #3581: the dev inspect route's agent classes (else empty), and if this member lists them.
    inspect: (&[String], bool),
) -> TsProgram {
    let member_mode = layout.is_member(context);
    let mut program = TsProgram::new();
    // Which conditional runtime helpers the entry's own inbound/outbound codecs
    // reach for; read back below to decide the import injection.
    let runtime_use = RuntimeUse::default();
    program.push(TsStmt::comment(
        "Generated by bynk — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::comment(
        format!("Worker entry point for context `{context}`."),
        None,
    ));

    // v0.79: if any handler uses `detach`, each entry point captures the runtime's
    // execution context and threads it into `compose`, so a fire-and-forget send
    // can hand its promise to `waitUntil`. Otherwise the signatures are unchanged.
    let exec_params: Vec<TsParam> = if ctx_uses_send {
        vec![TsParam::new(
            "ctx".to_string(),
            Some(TsType::Object(vec![TsTypeMember::method(
                "waitUntil",
                vec![TsParam {
                    name: "promise".to_string(),
                    ty: Some(TsType::named_with_args(
                        "globalThis.Promise",
                        vec![TsType::named("unknown")],
                    )),
                    optional: false,
                    rest: false,
                    array_pattern: None,
                    default: None,
                }],
                TsType::named("void"),
            )])),
        )]
    } else {
        Vec::new()
    };
    // L7 (#2414, Decision E): an `EdgeCache` instance purges through the
    // entry's `ctx`, so `compose` takes it as `exec` (optional when nothing
    // sends: an in-process caller may pass none), and so does `scheduled`.
    let edge_cache = table.derived.values().any(|d| d.is_edge_cache());
    let compose_takes_exec = ctx_uses_send || edge_cache;
    // #2192: `fetch` and `queue` always take `ctx`, which their fault handlers
    // report through. Optional when no send needs it, so an in-process caller
    // (the test runner) can still pass just `request`/`batch` and `env`.
    let fault_exec_params: Vec<TsParam> = if ctx_uses_send {
        exec_params.clone()
    } else {
        vec![TsParam::optional(
            "ctx".to_string(),
            Some(TsType::Object(vec![TsTypeMember::method(
                "waitUntil",
                vec![TsParam {
                    name: "promise".to_string(),
                    ty: Some(TsType::named_with_args(
                        "globalThis.Promise",
                        vec![TsType::named("unknown")],
                    )),
                    optional: false,
                    rest: false,
                    array_pattern: None,
                    default: None,
                }],
                TsType::named("void"),
            )])),
        )]
    };
    // `fetch` alone has a `request` in scope; `scheduled`/`queue` never do.
    let fetch_compose_args: Vec<TsExpr> = match (needs_locale_request, compose_takes_exec) {
        (true, true) => vec![ident("env"), ident("request"), ident("ctx")],
        (true, false) => vec![ident("env"), ident("request")],
        (false, true) => vec![ident("env"), ident("ctx")],
        (false, false) => vec![ident("env")],
    };
    let other_compose_args: Vec<TsExpr> = if compose_takes_exec {
        vec![ident("env"), ident("ctx")]
    } else {
        vec![ident("env")]
    };

    // Collect HTTP routes across all services, sorted so literal-segment
    // routes precede parameter-segment routes that share the same prefix.
    let mut service_names: Vec<&String> = table.services.keys().collect();
    service_names.sort();
    let http_routes = collect_http_routes(
        table,
        &service_names,
        visible_types,
        edge_cache,
        needs_locale_request,
        context,
    );

    // v0.131 (ADR 0159): the per-service CORS policies. A `from http` service
    // with a `cors { }` section gets one synthesised `CorsPolicy` constant; its
    // routes are answered with a preflight `OPTIONS` branch and stamped with the
    // `Access-Control-*` headers. Allow-methods is derived from the service's
    // routes (never restated); allow-headers defaults to `content-type` (plus
    // `Authorization` when the service has a Bearer route) unless overridden.
    let cors_services: Vec<CorsService> = build_cors_services(&service_names, table, &http_routes);

    // v0.141 (ADR 0164): the per-service security-headers policies. Unlike CORS,
    // *every* `from http` service with routes gets one — the safe header
    // (`nosniff`) is on by default, so a service with no `security { }` still
    // stamps the defaults. Its responses (and the synthesised preflight / `405` /
    // `304`) carry `applySecurityHeaders`, composing with `applyCors`.
    let security_services: Vec<SecurityService> =
        build_security_services(&service_names, table, &http_routes);

    // v0.139 (ADR 0162): the per-path method table driving the method-aware
    // router fall-through — a wrong method to a live path is a `405 + Allow`, a
    // bare `OPTIONS` is a `204 + Allow`, and both are derived from this one table
    // (the same derivation CORS reads for its allow-methods).
    let path_methods: Vec<PathMethods> =
        build_path_method_table(&http_routes, &cors_services, &security_services);

    // v0.10a: the cron handlers across all services.
    let cron_routes = collect_cron_routes(table, &service_names);

    // v0.10b: the queue consumers across all services.
    let queue_routes = collect_queue_routes(table, &service_names);

    // v0.104 (real-time track slice 3b): the `from websocket` upgrade routes.
    let ws_open_routes = collect_ws_open_routes(table, &service_names);

    // The runtime import is read off the finished tree (below), so the header is held until the
    // body has been built (1A.36b-2).
    let mut header_decls: Vec<TsStmt> = vec![
        TsStmt::decl(
            TsDecl::Import {
                type_only: false,
                names: vec!["compose".to_string(), "type Env".to_string()],
                from: layout.own_module_specifier(context, "compose.ts"),
            },
            None,
        ),
        TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: "handlers".to_string(),
                from: layout.own_handlers_specifier(context),
            },
            None,
        ),
    ];

    // v0.9.2: re-export each agent's Durable Object class from the Worker
    // entry. Cloudflare resolves a `class_name` binding against the named
    // exports of the Worker's `main`, so the DO classes declared in
    // `handlers.ts` must be visible here.
    let mut agent_names: Vec<&String> = table.agents.keys().collect();
    agent_names.sort();
    //
    // #3202: each is exported wrapped, so the instance runs the handlers that
    // reach it one at a time (`__serialiseAgentClass`, `runtime/src/agent.ts`).
    for name in &agent_names {
        header_decls.push(TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::ConstDecl {
                name: name.to_string(),
                ty: None,
                init: call(
                    ident("__serialiseAgentClass"),
                    vec![member(ident("handlers"), name.as_str())],
                ),
            })),
            None,
        ));
    }
    header_decls.extend(fanout_reexport(context, layout, uses_emit && !member_mode));

    header_decls.extend(container_class_reexport(table, member_mode));
    header_decls.extend(inspect_registry_export(inspect.0));

    let mut fetch_params: Vec<TsParam> = vec![
        TsParam::new(
            "request".to_string(),
            Some(TsType::named("globalThis.Request")),
        ),
        TsParam::new("env".to_string(), Some(TsType::named("Env"))),
    ];
    fetch_params.extend(fault_exec_params.iter().cloned());

    let mut fetch_body: Vec<TsStmt> = vec![
        const_(
            "url",
            new(
                ident("globalThis.URL"),
                vec![member(ident("request"), "url")],
            ),
        ),
        const_("path", member(ident("url"), "pathname")),
        const_("method", member(ident("request"), "method")),
        const_(
            "surface",
            call(ident("compose"), fetch_compose_args.clone()),
        ),
    ];
    // v0.131: the synthesised CORS policy constants, one per CORS-enabled service.
    for cs in &cors_services {
        fetch_body.push(const_typed(
            cs.const_name.clone(),
            TsType::named("__CorsPolicy"),
            cs.literal.clone(),
        ));
    }
    // v0.141: the synthesised security-headers policy constants, one per
    // `from http` service (with routes) — the default `nosniff` and any opt-in HSTS.
    for ss in &security_services {
        fetch_body.push(const_typed(
            ss.const_name.clone(),
            TsType::named("__SecurityPolicy"),
            ss.literal.clone(),
        ));
    }

    let mut try_body: Vec<TsStmt> = Vec::new();

    // 1. Internal Service Binding dispatch.
    //
    // I3 (#1971): the foreign callers any `on call` handler here admits, by
    // actor name. Empty for a Worker that admits none, whose dispatch then
    // admits Bynk contexts alone, as before.
    let mut foreign_callers: Vec<bynk_check::actors::ForeignCaller> = Vec::new();
    for sname in &service_names {
        for h in &table.services[*sname].handlers {
            for f in bynk_check::actors::call_admission_for(h, &table.actors)
                .map(|a| a.foreign)
                .unwrap_or_default()
            {
                if !foreign_callers.iter().any(|g| g.actor == f.actor) {
                    foreign_callers.push(f);
                }
            }
        }
    }
    let has_foreign = !foreign_callers.is_empty();
    if has_foreign {
        fetch_body.push(const_typed(
            FOREIGN_CALLERS,
            TsType::named_with_args(
                "globalThis.Record",
                vec![TsType::named("string"), TsType::named("string")],
            ),
            TsExpr::object(
                foreign_callers
                    .iter()
                    .map(|f| (f.actor.clone(), str_lit(f.secret.clone())))
                    .collect(),
            ),
        ));
    }
    // The foreign callers any `on call` handler admits are `foreign_callers`
    // above; `has_foreign` makes the dispatch check them.
    try_body.extend(call_dispatch(
        context,
        member_mode,
        table,
        &service_names,
        (own_contracts, accepted_contracts),
        has_foreign,
        &runtime_use,
    ));
    try_body.push(TsStmt::blank(None));

    // 1.25. The internal admin routes: see `admin_routes`.
    try_body.extend(admin_routes(context, member_mode, migratable, inspect));

    // 1.5. WebSocket upgrade dispatch: see `ws_upgrade_dispatch`.
    if !ws_open_routes.is_empty() {
        try_body.push(ws_upgrade_dispatch(&ws_open_routes, table, context));
        try_body.push(TsStmt::blank(None));
    }

    // 1.6. Events dispatch: see `events_delivery_route`.
    if let Some(events) =
        events_delivery_route(context, member_mode, table, &service_names, &runtime_use)
    {
        try_body.push(events);
        try_body.push(TsStmt::blank(None));
    }

    // v0.131 (ADR 0159): CORS preflight, one branch per CORS-enabled service.
    for cs in &cors_services {
        try_body.push(cors_preflight(cs, &security_services));
    }

    // 2. External HTTP routes.
    for route in &http_routes {
        let cors_const = cors_services
            .iter()
            .find(|cs| cs.service == route.service)
            .map(|cs| cs.const_name.as_str());
        // v0.141: every `from http` route's service has a security policy constant.
        let security_const = security_services
            .iter()
            .find(|ss| ss.service == route.service)
            .map(|ss| ss.const_name.as_str());
        try_body.push(emit_http_route_dispatch(
            route,
            cors_const,
            security_const,
            &runtime_use,
            consumed_types,
        ));
    }

    // v0.139 (ADR 0162): the method-aware router fall-through, one branch per path.
    for pm in &path_methods {
        try_body.push(method_fall_through(pm, member_mode));
    }

    try_body.push(return_(Some(miss(
        member_mode,
        text_response("Not Found", 404),
    ))));

    // #1825: a fault answers a bare 500, never leaking the error to the
    // client (#184), but it is logged here first, with the context and the
    // dispatch that faulted: the route *pattern* or `call <service>`, never
    // the request, so no key or value reaches the log. Caught, it is no
    // uncaught exception the platform would record, so without this a fault
    // left no trace at all. #2053: the log carries the error's stack
    // (`fault_detail`), not its bare string, so a tail shows the frames.
    // #2192: they stay `index.js:L:C`, since Cloudflare maps an uploaded
    // source map only onto an exception it records, so the fault is also
    // reported (`__bynkReportFault`). It does not rethrow: that would replace
    // this 500 with the platform's own error page, and the queue handler's
    // per-message `retry()` with a whole-batch retry. #2193: the 500 comes
    // from `__faultResponse`, which marks a fault in an `on call` service with
    // this context's name, so the calling Worker reports `CalleeFaulted`
    // naming it rather than a bare `Transport`.
    fetch_body.push(TsStmt::let_stmt(
        TsBindingName::Ident("__route".to_string()),
        Some(TsType::named("string")),
        Some(str_lit("request")),
        None,
    ));
    fetch_body.push(try_catch_stmt(
        try_body,
        Some("e"),
        fault_handler(context, trace),
    ));

    let fetch_return = Some(TsType::named_with_args(
        "globalThis.Promise",
        vec![TsType::named("globalThis.Response")],
    ));
    // L7 (#2414, Decision C): an opted-in Worker's dispatch moves into
    // `__bynkFetch`, and `fetch` runs whatever it answers through
    // `edgeCacheGuard`, so the router's own answers (a `404`, a `405`, a
    // rejection, a fault) say `no-store` too.
    let fetch_body = if edge_cache {
        header_decls.push(TsStmt::decl(
            TsDecl::Function {
                name: "__bynkFetch".to_string(),
                generics: Vec::new(),
                params: fetch_params.clone(),
                return_type: fetch_return.clone(),
                body: fetch_body,
                is_async: true,
                inline: false,
            },
            None,
        ));
        edge_cache_return(member_mode)
    } else {
        fetch_body
    };
    let fetch_body = if trace {
        traced_fetch(
            context,
            &mut header_decls,
            &fetch_params,
            &fetch_return,
            fetch_body,
        )
    } else {
        fetch_body
    };
    let mut default_entries: Vec<TsObjectEntry> = vec![TsObjectEntry::method(
        "fetch",
        true,
        fetch_params,
        fetch_return,
        fetch_body,
    )];

    // v0.10a: scheduled (cron) entry point. Dispatches on `event.cron`. A
    // failing run has no retry channel, so an `Err` is logged and the run
    // completes (v0.10 §5.1, [DECISION 3]).
    if !cron_routes.is_empty() {
        default_entries.push(emit_scheduled_handler(
            &cron_routes,
            &fault_exec_params,
            &other_compose_args,
            scheduled_result_hook,
        ));
    }

    // v0.10b: the queue (consumer) entry point; #3328: the email entry point.
    push_message_entries(
        table,
        &queue_routes,
        &fault_exec_params,
        &other_compose_args,
        &runtime_use,
        &mut default_entries,
    );

    let runtime_import_at = program.stmts.len();
    for decl in header_decls {
        program.push(decl);
    }
    if member_mode {
        // 1A.38: fixed authored snippet, no interpolation
        program.push(TsStmt::raw_using(MISS_HELPER, &[]));
    }
    // 1A.38: fixed authored snippet, no interpolation
    program.push(TsStmt::raw_using(
        fault_helpers(!cron_routes.is_empty(), !queue_routes.is_empty()),
        &[],
    ));
    program.push(TsStmt::decl(
        TsDecl::ExportDefault(trace_scope::export_default(default_entries)),
        None,
    ));

    // 1A.36b-2: the runtime import is what the module's tree uses, after the banner and before
    // the other imports.
    let names =
        super::runtime_header::derived_worker_runtime_names(&program.stmts[runtime_import_at..]);
    for (i, stmt) in super::runtime_files::runtime_import_stmts(false, &names, "../../runtime.js")
        .into_iter()
        .enumerate()
    {
        program.stmts.insert(runtime_import_at + i, stmt);
    }
    // Each fixed import (`handlers`, `compose`) is kept only if the tree uses it.
    program.prune_unused_imports();

    program
}

/// #2341 (decision C): the compose-surface method a service's `on rejected`
/// is reached by, when it declares one.
fn rejected_surface(service: &bynk_syntax::ast::ServiceDecl, sname: &str) -> Option<String> {
    service
        .handlers
        .iter()
        .any(|h| h.kind == bynk_syntax::ast::HandlerKind::Rejected)
        .then(|| format!("rejected_{sname}"))
}

#[cfg(test)]
mod tests {
    use super::respond::own_path_guard;
    use super::*;

    fn printed(stmts: &[TsStmt]) -> String {
        stmts.iter().map(|s| bynk_ts::print_stmt(s, 0)).collect()
    }

    /// Default (non-member) output: the guard is the bare prefix test, nothing is
    /// marked, and the edge-cache return is the one it has always been.
    #[test]
    fn outside_a_group_nothing_is_marked_or_gated() {
        let guard = bynk_ts::print_expr(&own_path_guard(false, "/_bynk/call/", &[]));
        assert_eq!(guard, "path.startsWith(\"/_bynk/call/\")");
        let marked = bynk_ts::print_expr(&miss(false, text_response("Not Found", 404)));
        assert!(!marked.contains("__miss"), "{marked}");
        let edge = printed(&edge_cache_return(false));
        assert!(
            !edge.contains("isMiss") && !edge.contains("__miss"),
            "{edge}"
        );
        assert!(
            edge.contains("__edgeCacheGuard(request, await __bynkFetch("),
            "{edge}"
        );
    }

    /// The edge-cache guard can answer with a copy, so a member re-marks it.
    #[test]
    fn a_members_edge_cache_guard_keeps_a_miss_a_miss() {
        let edge = printed(&edge_cache_return(true));
        assert!(
            edge.contains("isMiss(__answer) ? __miss(__guarded) : __guarded"),
            "{edge}"
        );
    }
}
