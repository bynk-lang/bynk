//! The Cloudflare Workers composition root (v0.8 §4.5, v0.9 §5.1).
//!
//! Each Worker's `compose.ts` exports a `compose(env)` function that
//! assembles the context's deps and returns the surface the entry point
//! invokes — `on call` services for the internal Service Binding protocol
//! plus `on http` route wrappers for the external HTTP router.
//!
//! Arc C slice 3 (#1321): `emit_worker_compose` and its private helpers build
//! a real `bynk_ts::TsProgram` directly instead of `writeln!`-ing a `String`.
//! Back-half paydown 1B.1a moved them here from `workers.rs`. Since 1B.1b the
//! unit and its dependencies come from the [`ComposePlan`]; the order of the
//! `deps` entries, the `const` per provider, the `__exec` condition, the
//! cross-capability set, `env`, the session entry and the delivery through the
//! fan-out are this renderer's own (divergences 1 to 7 of #3928).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::emitter::compose::{ComposePlan, empty_deps};

use crate::emit_model::{ImportExt, LocaleNegotiationArgs};
use crate::emitter::RuntimeUse;
use crate::emitter::bindings::{BindingSource, WorkerBinding, events_fanout_binding};
use crate::emitter::workers::*;
use crate::emitter::workers_sum::emit_http_sum_wrapper;
use crate::emitter::workers_ws_auth::WsCredential;
use crate::emitter::wrangler::binding_ts_type;
use crate::ts_build::{as_expr, call, const_, ident, member, method_call, return_, str_lit};
use bynk_check::symbols::MessageBundleInfo;
use bynk_check::symbols::UnitTable;
use bynk_ts::{
    TsArrowBody, TsDecl, TsExpr, TsObjectEntry, TsParam, TsProgram, TsStmt, TsType, TsTypeMember,
};

/// Where `compose.ts` imports the runtime from — it sits two levels below the
/// out root (`<out>/workers/<worker>/compose.ts`).
///
/// The import is built from the finished tree (`derived_worker_runtime_names`)
/// and inserted after the banner, so this is the one place the specifier is
/// spelled. It is named so that it cannot drift from the layout the entry and
/// the other per-Worker modules assume.
const COMPOSE_RUNTIME_SPECIFIER: &str = "../../runtime.js";

/// The events a publishing handler hands its fan-out. A group's one fan-out
/// serves every publishing member, so a member names itself on each event
/// (`publisher`), as two members may declare an event of one name (#3562, G3-1).
/// A context on a Worker of its own sends them as they are.
fn fanout_events(context: &str, layout: &bynk_project::worker_layout::WorkerLayout) -> TsExpr {
    if !layout.is_member(context) {
        return ident("events");
    }
    let tagged = TsExpr::Arrow {
        params: vec![TsParam::new("e".to_string(), None)],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        // Parenthesised, or the braces read as the arrow's body.
        body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Paren(Box::new(
            as_expr(
                TsExpr::object_entries(vec![
                    TsObjectEntry::Spread(ident("e")),
                    TsObjectEntry::prop("publisher".to_string(), str_lit(context.to_string())),
                ]),
                TsType::named(crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE),
            ),
        ))))),
    };
    method_call(ident("events"), "map", vec![tagged])
}

/// Events track, slice 0 (spine #936, ADR 0284): a publishing handler's
/// release-at-commit event batch is handed to this context's own fan-out DO
/// (`env.<bind>` is typed by the `Env` interface, one instance per publishing
/// context), as `deps.__eventsDispatch`.
/// Divergence 7 of #3928, delivery: through the fan-out Durable Object (the
/// Bundle dispatches in-process).
fn events_dispatch_entry(
    context: &str,
    layout: &bynk_project::worker_layout::WorkerLayout,
) -> TsObjectEntry {
    let bind = events_fanout_binding().name;
    let arrow = TsExpr::Arrow {
        params: vec![TsParam::new(
            "events".to_string(),
            Some(TsType::named_with_args(
                "globalThis.Array",
                vec![TsType::named(crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE)],
            )),
        )],
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Expr(Box::new(call(
            ident("__dispatchToEventsFanout"),
            vec![member(ident("env"), bind), fanout_events(context, layout)],
        )))),
    };
    TsObjectEntry::prop("__eventsDispatch".to_string(), arrow)
}

/// Divergence 1 of #3928, the Workers' entry order for the capability part of
/// `deps`: a plain `String` sort, so `Db` comes before `Db2` (the Bundle sorts
/// by `key + ":"`, which puts `Db2` first).
fn deps_caps_in_order(mut caps: Vec<String>) -> Vec<String> {
    caps.sort();
    caps
}

/// The compose methods of one `on http` handler: its wrapper, which runs the actor's verification
/// seam (or none) and calls the handler, or, for a route counted by `key: identity` (#3926), a
/// verify step and a run step.
fn http_route_wrappers(
    service: &bynk_syntax::ast::ServiceDecl,
    sname: &str,
    h: &bynk_syntax::ast::Handler,
    (method, path): (bynk_model::IrHttpMethod, &str),
    table: &UnitTable,
    runtime_use: &RuntimeUse,
) -> Vec<TsObjectEntry> {
    let mut return_entries: Vec<TsObjectEntry> = Vec::new();
    // v0.151: a single-actor `Oidc` handler gets the JWKS
    // verification wrapper. v0.52: a multi-actor sum handler gets
    // the first-wins resolution wrapper; otherwise the
    // single-actor Bearer/plain path. Review of #1209: this site
    // used to check `oidc` ahead of `sum`; `lower_actor_seam_ir`
    // checks `sum` ahead of `oidc` (matching `ActorSeamIr`'s own
    // canonical order). That swap is licensed by
    // `oidc_seam_for`'s own `by.is_sum()` early return
    // (`bynk-check/src/actors.rs`) — Oidc and a sum can never
    // both resolve for one handler, so which is tried first is a
    // no-op, unlike the sum-vs-Bearer order `ActorSeamIr`'s own
    // doc comment is about (Bearer was already checked last at
    // this site, so that pair's order didn't change here).
    // `Caller` never arises here — `caller_binder_for` only ever
    // resolves for `HandlerKind::Call` — so it shares the plain
    // path with `None`, exactly like the old fallthrough
    // (`bearer_seam_for` also misses on a `Caller` handler, since
    // `Caller` is a prelude actor, not a key of `table.actors`).
    // #3926: a route counted by `identity` is served by a verify step and a
    // run step, so the entry can ask the limiter between them.
    let identity_limited = bynk_check::http_rate_limit::route_rate_limit(h, table)
        .is_some_and(|r| r.key == bynk_check::http_rate_limit::RateLimitKey::Identity);
    match bynk_model::lower_actor_seam_ir(h, &table.actors) {
        bynk_model::ActorSeamIr::Oidc(oidc) if identity_limited => {
            return_entries.extend(oidc_identity_split(sname, h, method, path, &oidc));
        }
        bynk_model::ActorSeamIr::Oidc(oidc) => {
            return_entries.push(emit_http_oidc_wrapper(sname, h, method, path, &oidc));
        }
        bynk_model::ActorSeamIr::SignedCookie(seam) if identity_limited => {
            return_entries.extend(signed_cookie_identity_split(
                service, sname, h, method, path, &seam,
            ));
        }
        bynk_model::ActorSeamIr::SignedCookie(seam) => {
            return_entries.push(signed_cookie_entry(service, sname, h, method, path, &seam))
        }
        // #3939: `key: identity` admits `ApiKey` from slice 2 (track §12); the checker refuses it
        // until then, so an `ApiKey` route is always the one wrapper.
        bynk_model::ActorSeamIr::ApiKey(seam) => {
            return_entries.push(crate::emitter::workers_api_key::emit_http_api_key_wrapper(
                sname, h, method, path, &seam,
            ));
        }
        bynk_model::ActorSeamIr::Sum(members) => {
            return_entries.push(emit_http_sum_wrapper(
                sname,
                h,
                method,
                path,
                &members,
                service.limits.as_ref().and_then(|p| p.max_body()),
                &csrf_origins(service),
                runtime_use,
            ));
        }
        bynk_model::ActorSeamIr::Bearer(seam) if identity_limited => {
            return_entries.extend(bearer_identity_split(sname, h, method, path, &seam));
        }
        bynk_model::ActorSeamIr::Bearer(seam) => {
            return_entries.push(emit_http_wrapper(sname, h, method, path, Some(&seam)));
        }
        bynk_model::ActorSeamIr::Caller(_)
        | bynk_model::ActorSeamIr::Sender(_)
        | bynk_model::ActorSeamIr::CallSum(_)
        | bynk_model::ActorSeamIr::None => {
            return_entries.push(emit_http_wrapper(sname, h, method, path, None));
        }
    }
    return_entries
}

#[allow(clippy::too_many_arguments)]
pub fn emit_worker_compose(
    context: &str,
    plan: &ComposePlan,
    layout: &bynk_project::worker_layout::WorkerLayout,
    table: &UnitTable,
    unit_tables: &HashMap<String, UnitTable>,
    // v0.17: adapter unit name → its binding module path (relative to the out
    // root, `.js`). An adapter's external provider class lives in this module,
    // not in a `handlers.ts`.
    binding_modules: &HashMap<String, String>,
    // v0.18: the whole-project consume/alias/flattening maps, so external
    // provider deps recurse across adapters (spec §4.5).
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    // Locale capability track, slice 2 (#882): this context's uniquely-
    // detected message bundle, if any (`None` when zero or 2+ were found —
    // the caller already resolved the cardinality, this function only acts).
    locale_bundle: Option<&MessageBundleInfo>,
    import_ext: ImportExt,
    // #1944: every binding this Worker declares (`WorkerBindings`),
    // the list its `wrangler.toml` is written from; `Env` types exactly it.
    bindings: &[WorkerBinding],
    // #2137: the test build of a context a `system` harness stands up — its
    // first-party `Clock` reads the harness clock (`ProviderNamespaces::Workers`).
    harness_clock: bool,
    // #2967: the commons this context `uses`, whose public constants can key a
    // WebSocket `on open`'s transfer (`bynk_check::consts::visible_consts`).
    uses: &[String],
    // #2969: used commons → the file declaring each of its functions and
    // constants (`FileDeclIndex::fns`), for a commons split across files.
    used_decl_paths: &HashMap<String, HashMap<String, std::path::PathBuf>>,
) -> (TsProgram, bool) {
    // #2967: the constants visible in this context, as the checker's
    // `phase_workers_ws_open_key` classifies a transfer key against them.
    let visible_consts = bynk_check::consts::visible_consts(table, uses, |u| unit_tables.get(u));
    // #2967: a used commons' constant keying an `on open`'s transfer, imported
    // into this module under an alias: alias → (owning commons, constant).
    let mut const_imports: BTreeMap<String, (String, String)> = BTreeMap::new();
    let unit = plan
        .units
        .get(context)
        .unwrap_or_else(|| panic!("the compose plan has no unit `{context}`: every context with a checked table is planned in `build_output`"));
    // v0.15: cross-context capabilities this Worker uses (in handlers or in a
    // local provider's `given`) → `deps_key → consumed_context`. Their
    // providers are instantiated locally (model A1). Divergence 3 of #3928,
    // the cross-capability set: the Workers `compose` lifts the providers'
    // cross-context `given`s to the top level too (the Bundle root, the
    // handlers' only).
    let cross_caps = unit.cross_caps();

    // Locale capability track, slice 2 (#882, Decision C): only when this
    // Worker both consumes `bynk`'s `Locale` and has a uniquely-detected
    // message bundle do we import its declared-locale constants and pass
    // `request` + them into `LocaleProvider`'s construction below.
    let mut locale_import: Option<TsStmt> = None;
    let mut locale_negotiation_args: Option<LocaleNegotiationArgs> = None;
    if cross_caps
        .get("Locale")
        .is_some_and(|c| c == bynk_check::firstparty::BYNK_UNIT)
        && let Some(bundle) = locale_bundle
    {
        let import = crate::emitter::cross_commons_import_specifier_for_path(
            &layout.handlers_source_path(context),
            &bundle.source_path,
            import_ext,
        );
        // #1321: a renamed named import (`X as Y`) is one opaque `String`
        // entry in `TsDecl::Import.names` — see that field's own doc.
        locale_import = Some(TsStmt::decl(
            TsDecl::Import {
                type_only: false,
                names: vec![
                    "__messagesLocales as __locale_declaredLocales".to_string(),
                    "__messagesReferenceLocale as __locale_referenceLocale".to_string(),
                ],
                from: import,
            },
            None,
        ));
        locale_negotiation_args = Some(LocaleNegotiationArgs {
            request_expr: "request".to_string(),
            declared_locales_expr: "__locale_declaredLocales".to_string(),
            reference_locale_expr: "__locale_referenceLocale".to_string(),
        });
    }
    let needs_request = locale_negotiation_args.is_some();

    // v0.18: build each cross-cap provider expression up front, recording the
    // units it references — an external provider's `given` may pull in another
    // adapter's binding (the transitive given-closure), which must be imported.
    let mut referenced_units: BTreeSet<String> = BTreeSet::new();
    let mut cross_cap_exprs: Vec<(String, TsExpr)> = Vec::new();
    for (key, cctx) in &cross_caps {
        // #1321: the `TsExpr`-returning twin of `instantiate_provider_expr`
        // (`project.rs`'s own Decision-B-style addition) — see its own doc
        // for why the `String` version stays untouched.
        let expr = crate::emit_model::instantiate_provider_ts_expr(
            cctx,
            key,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            crate::emit_model::ProviderNamespaces::Workers { harness_clock },
            Some("env"),
            locale_negotiation_args.as_ref(),
            &mut referenced_units,
        );
        cross_cap_exprs.push((key.clone(), expr));
    }
    // S4a (#1966): each derived capability's instance, its base's platform
    // provider bound to the instance's own Worker binding (which also imports
    // the binding module).
    for cap in &unit.derived {
        let expr = crate::emit_model::instantiate_provider_ts_expr(
            context,
            cap,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            crate::emit_model::ProviderNamespaces::Workers { harness_clock },
            Some("env"),
            locale_negotiation_args.as_ref(),
            &mut referenced_units,
        );
        cross_cap_exprs.push((cap.clone(), expr));
    }

    let runtime_use = RuntimeUse::default();
    let binds = |kind: fn(&BindingSource) -> bool| bindings.iter().any(|b| kind(&b.source));

    // Only consumed *contexts* become Service Bindings — a consumed adapter is
    // not a Worker (its capability is provided in-process via the binding).
    let consumes_a_context = unit
        .consumes
        .iter()
        .any(|t| !binding_modules.contains_key(t));

    // v0.79: if any handler in this context uses `detach`, `compose` also takes the
    // request's execution context and threads its `waitUntil` into `deps.__exec`.

    // -- Build the `compose` function's own body (Decision A/B: real nodes
    // throughout) before assembling the header: the runtime import is read off the
    // finished tree (below). --

    let mut body: Vec<TsStmt> = Vec::new();

    // v0.15: instantiate consumed-unit capability providers locally first, so
    // local providers (and handlers) can depend on them by their deps key.
    // v0.18: the expression recursively wires an external provider's `given`
    // deps (built in the pre-pass above; cross_caps is a BTreeMap, so the
    // pairs are already key-sorted).
    let cross_keys: Vec<&String> = cross_caps.keys().collect();
    for (key, expr) in cross_cap_exprs {
        body.push(const_(key, expr));
    }

    // Capabilities: instantiate each capability's provider. v0.12: providers
    // are emitted in dependency order (a composed provider's `given` deps must
    // exist first) as local `const` bindings, injecting each provider's deps;
    // then assembled into the `deps` object. v0.15: a provider's `given` may
    // include cross-context capability keys (instantiated above).
    // Divergence 2 of #3928, provider spelling: one `const` per provider, in the
    // plan's dependency order (the Bundle root builds each inline).
    let order = &unit.providers;
    for cap in order {
        let provider = table.providers.get(cap).unwrap();
        let provider_ts = provider.provider_name.name.clone();
        // #2355: a provider whose ops construct a local agent takes `env` in
        // its deps, for the agent factory (`emit::emit_provider`).
        let constructs_agent =
            bynk_check::project_model::provider_constructs_agent(provider, |n| {
                table.agents.contains_key(n)
            });
        let args = if provider.given.is_empty() && !constructs_agent {
            vec![]
        } else {
            let mut entries: Vec<TsObjectEntry> = provider
                .given
                .iter()
                .map(|c| TsObjectEntry::Shorthand(c.key().to_string()))
                .collect();
            if constructs_agent {
                entries.push(TsObjectEntry::Shorthand("env".to_string()));
            }
            vec![TsExpr::object_entries(entries)]
        };
        let init = TsExpr::New {
            callee: Box::new(member(ident("handlers"), provider_ts)),
            args,
        };
        body.push(const_(cap.clone(), init));
    }
    // Divergence 1 of #3928, the entry order: capabilities by a plain sort,
    // then `env`, the session, `__exec` and `__eventsDispatch` appended in that
    // fixed order (the Bundle sorts every entry by `key + ":"`).
    let mut deps_entries: Vec<TsObjectEntry> = {
        let mut caps: Vec<String> = order.clone();
        caps.extend(cross_keys.iter().map(|k| (*k).clone()));
        caps.extend(unit.derived.iter().cloned());
        deps_caps_in_order(caps)
            .into_iter()
            .map(TsObjectEntry::Shorthand)
            .collect()
    };
    // Divergence 5 of #3928, `env`: it passes through for cross-context calls
    // (Service Bindings) and agent instantiations (DOs); the Bundle threads it
    // as a `composeApp` parameter into providers instead.
    if consumes_a_context || unit.has_agents {
        deps_entries.push(TsObjectEntry::Shorthand("env".to_string()));
    }
    // Divergence 6 of #3928: the session entry is the Workers' alone.
    deps_entries.extend(session_deps_entry(table));
    // v0.79: the execution context rides in `deps.__exec` for `detach` sends.
    // Divergence 4 of #3928, the `__exec` gate and value: any handler that
    // needs it, and the request's own `exec` (the Bundle: `call` handlers only,
    // and `__bynkProcessExec()`).
    let ctx_uses_send = unit.exec.any_handler;
    if ctx_uses_send {
        deps_entries.push(TsObjectEntry::prop("__exec".to_string(), ident("exec")));
    }
    if unit.publishes {
        deps_entries.push(events_dispatch_entry(context, layout));
    }
    // `{  }` for no entries: see `compose::empty_deps`.
    let deps_init = if deps_entries.is_empty() {
        empty_deps()
    } else {
        TsExpr::object_entries(deps_entries)
    };
    body.push(const_("deps", deps_init));

    // Local-surface object: one async wrapper per service operation plus
    // one wrapper per `on http` handler.
    let mut service_names: Vec<&String> = table.services.keys().collect();
    service_names.sort();
    let mut return_entries: Vec<TsObjectEntry> = Vec::new();
    for sname in &service_names {
        let service = table.services.get(*sname).unwrap();
        let mut cron_idx = 0usize;
        let mut queue_idx = 0usize;
        for h in &service.handlers {
            // P6.30 (design/tracks/the-ir.md §6a): dispatches on the checker-
            // classified `IrHandlerKind` (P6.24a's own pure, unconditional
            // mirror) rather than the raw AST `HandlerKind` directly. Slice 1
            // of `#1542` (the IR cutover, `design/archive/retired-tracks.md`): the `Http`
            // arm now reads `method`/`path` straight off that same
            // `IrHandlerKind::Http` payload instead of re-deriving them from
            // `h.kind` — un-deferred from Q7's original "wrapper signatures
            // stay AST-typed until phase 7's printer" (phase 7 shipped; see
            // the front-loading ADR).
            match bynk_model::lower_handler_kind_ir(&h.kind) {
                bynk_model::IrHandlerKind::Call => {
                    return_entries.push(emit_call_wrapper(sname, h, &table.actors));
                }
                bynk_model::IrHandlerKind::Http { method, path } => {
                    return_entries.extend(http_route_wrappers(
                        service,
                        sname,
                        h,
                        (method, &path),
                        table,
                        &runtime_use,
                    ));
                }
                bynk_model::IrHandlerKind::Cron { .. } => {
                    return_entries.push(emit_cron_wrapper(sname, cron_idx, h));
                    cron_idx += 1;
                }
                bynk_model::IrHandlerKind::Message => {
                    let Some(entry) = message_wrapper(service, sname, queue_idx, h, &table.actors)
                    else {
                        continue;
                    };
                    return_entries.push(entry);
                    queue_idx += 1;
                }
                bynk_model::IrHandlerKind::Open => {
                    // An actor sum (`by who: A | B`) resolves member by member at the
                    // edge; `bearer_seam_for` would see only its first member.
                    let sum = bynk_check::actors::sum_members_for(h, &table.actors);
                    let credential = if sum.is_some() {
                        None
                    } else {
                        WsCredential::for_handler(h, service, &table.actors)
                    };
                    let local_agents: HashSet<String> = table.agents.keys().cloned().collect();
                    // #1818: hosting agents that read their key are forwarded it.
                    let key_readers: HashSet<String> = table
                        .agents
                        .iter()
                        .filter(|(_, a)| crate::emitter::emit::agent_reads_key(a))
                        .map(|(n, _)| n.clone())
                        .collect();
                    let jurisdictions: HashMap<String, String> = table
                        .agents
                        .iter()
                        .filter_map(|(n, a)| {
                            bynk_check::agent_jurisdiction::jurisdiction_of(a)
                                .map(|j| (n.clone(), j.to_string()))
                        })
                        .collect();
                    return_entries.push(emit_websocket_upgrade(
                        sname,
                        h,
                        credential.as_ref(),
                        sum.as_deref(),
                        &local_agents,
                        &key_readers,
                        &jurisdictions,
                        &visible_consts,
                        &mut const_imports,
                    ));
                }
                // v0.106 (slice 3b-iii): `on close` runs in the DO (`webSocketClose`),
                // not at the edge — no compose wrapper.
                bynk_model::IrHandlerKind::Close => {}
                // Events track, slice 0 (spine #936): unlike the WS
                // lifecycle handlers above, an `on event` handler's body
                // *does* need a compose-surface wrapper — it is reached from
                // `/_bynk/event/<sname>` (`emit_worker_entry`), the route a
                // *subscriber* Worker's own fan-out delivery lands on (not
                // this context's edge traffic, and not HTTP-routable, hence
                // no route table entry — just a plain wrapper like `on
                // call`'s).
                bynk_model::IrHandlerKind::Event => {
                    return_entries.push(emit_event_wrapper(
                        sname,
                        unit.hosted_wants_envelope(context, sname),
                    ));
                }
                // #2341 (decision C): the router calls `rejected_<svc>(why)`
                // to render a rejection of one of this service's requests.
                bynk_model::IrHandlerKind::Rejected => {
                    return_entries.push(emit_rejected_wrapper(sname));
                }
            }
        }
    }
    body.push(return_(Some(TsExpr::multiline_object_entries(
        return_entries,
    ))));

    let mut compose_params = vec![TsParam::new("env".to_string(), Some(TsType::named("Env")))];
    // Locale capability track, slice 2 (#882): `request` is threaded
    // independently of `exec` — the two-way independence matters because
    // `scheduled`/`queue` (which never have a `request`) may still use
    // `detach` (`exec`), and a `fetch`-only Worker with no message bundle needs
    // neither.
    if needs_request {
        compose_params.push(TsParam::optional(
            "request".to_string(),
            Some(TsType::named("globalThis.Request")),
        ));
    }
    // L7 (#2414, Decision E): an `EdgeCache` instance is built from the
    // entry's `ctx` (`new WorkersEdgeCache(exec)`), so compose takes it even
    // when nothing sends; optional then, as the entry's own `ctx` is.
    let edge_cache = table.derived.values().any(|d| d.is_edge_cache());
    if edge_cache && !ctx_uses_send {
        compose_params.push(TsParam::optional(
            "exec".to_string(),
            Some(TsType::named("unknown")),
        ));
    }
    if ctx_uses_send {
        compose_params.push(TsParam {
            name: "exec".to_string(),
            // #1321: a fixed, non-generative type-literal-with-a-method
            // constant — no per-call variation, so it's carried as opaque
            // `Named` text rather than a general method-signature-in-a-
            // type-literal shape, the same "opaque `Named` for one real,
            // unvarying shape" precedent `ts_type_ref_to_ts_type`'s own
            // `TypeRef::Query` arm already established (`emitter.rs`).
            ty: Some(TsType::named(
                "{ waitUntil(promise: globalThis.Promise<unknown>): void }",
            )),
            optional: false,
            rest: false,
            array_pattern: None,
            default: None,
        });
    }

    // -- Assemble the whole document in its real, final order. --
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynk — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::comment(
        format!("composition root for `{context}` Worker."),
        None,
    ));
    // The runtime import goes here, once the rest of the module is built (below).
    let runtime_import_at = program.stmts.len();
    program.push(TsStmt::decl(
        TsDecl::ImportNamespace {
            type_only: false,
            alias: "handlers".to_string(),
            from: layout.own_handlers_specifier(context),
        },
        None,
    ));
    // Import each referenced unit's provider classes. A *context*'s providers
    // live in its Worker's `handlers.js`; an *adapter*'s external provider
    // classes live in its binding module at the out root.
    for cctx in &referenced_units {
        let ns = cctx.replace('.', "_");
        if let Some(module) = binding_modules.get(cctx) {
            program.push(TsStmt::decl(
                TsDecl::ImportNamespace {
                    type_only: false,
                    alias: format!("{ns}__binding"),
                    from: format!("../../{module}"),
                },
                None,
            ));
        } else {
            program.push(TsStmt::decl(
                TsDecl::ImportNamespace {
                    type_only: false,
                    alias: format!("handlers_{ns}"),
                    from: layout.handlers_specifier(context, cctx),
                },
                None,
            ));
        }
    }
    if let Some(li) = locale_import {
        program.push(li);
    }
    // #2967: `compose.ts` imports a used commons' constant from the commons
    // itself, as the context's own modules do: `handlers.ts` imports it without
    // re-exporting it. Aliased, so a constant named like a binding of this
    // module (`Env`, `handlers`) cannot shadow it.
    // #2969: grouped by the module that declares each constant, which for a
    // commons split across files is one of its files, not the commons path.
    let mut by_module: BTreeMap<std::path::PathBuf, Vec<String>> = BTreeMap::new();
    for (alias, (owner, name)) in &const_imports {
        let module = used_decl_paths
            .get(owner)
            .and_then(|paths| paths.get(name))
            .cloned()
            .unwrap_or_else(|| crate::emit_model::EmitProjectCtx::commons_path(owner));
        by_module.entry(module).or_default().push(format!(
            "{} as {alias}",
            bynk_check::emitted_names::ts_ident(name)
        ));
    }
    for (module, names) in by_module {
        program.push(TsStmt::decl(
            TsDecl::Import {
                type_only: false,
                names,
                from: crate::emitter::cross_commons_import_specifier_for_path(
                    &layout.handlers_source_path(context),
                    &module,
                    import_ext,
                ),
            },
            None,
        ));
    }

    // Env shape: one member per binding the Worker's `wrangler.toml`
    // declares, from the same list (#1944), so the two cannot drift.
    let env_members: Vec<TsTypeMember> = bindings
        .iter()
        .map(|b| TsTypeMember::prop(b.name.clone(), TsType::named(binding_ts_type(&b.source))))
        .collect();
    program.push(TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::Interface {
            name: "Env".to_string(),
            type_params: Vec::new(),
            members: env_members,
        })),
        None,
    ));

    if binds(|s| {
        matches!(
            s,
            BindingSource::Agent { .. }
                | BindingSource::EventsFanout { .. }
                | BindingSource::ContainerInstance { .. }
        )
    }) {
        // P7.2: deferred, not narrowed — this settling attempt broke real
        // `tsc --strict` fixtures. A real, differently-shaped imported
        // `DurableObjectNamespace` (from `./runtime`, `fetch(input: string,
        // init?: unknown)`) can be in scope in the same file as this local
        // fallback (used with `fetch(new Request(...))` at this file's own
        // `__stub.fetch` call site) — the two share a name but not a shape, and
        // reconciling them needs more than a same-line fix (a real import/alias,
        // or renaming the local fallback), not a guessed structural type.
        program.push(TsStmt::decl(
            TsDecl::TypeAlias {
                name: "__DurableObjectNamespace".to_string(),
                type_params: Vec::new(),
                ty: TsType::named(
                    "{ idFromName(name: string): { toString(): string }; idFromString(id: string): { toString(): string }; get(id: any): any }",
                ),
            },
            None,
        ));
    }

    program.push(TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::Function {
            name: "compose".to_string(),
            generics: Vec::new(),
            params: compose_params,
            return_type: None,
            body,
            is_async: false,
            inline: false,
        })),
        None,
    ));

    // 1A.36b-2: the runtime import is what the module's tree uses (`derived_worker_runtime_names`);
    // its position is after the banner, before the other imports.
    let names = crate::emitter::runtime_header::derived_worker_runtime_names(
        &program.stmts[runtime_import_at..],
    );
    for (i, stmt) in crate::emitter::runtime_files::runtime_import_stmts(
        false,
        &names,
        COMPOSE_RUNTIME_SPECIFIER,
    )
    .into_iter()
    .enumerate()
    {
        program.stmts.insert(runtime_import_at + i, stmt);
    }
    // Each fixed import (`handlers`, a provider's namespace) is kept only if the tree uses it.
    program.prune_unused_imports();

    (program, needs_request)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Divergence 1 of #3928: the Workers sort capabilities plainly. A `Db`/`Db2`
    /// pair therefore orders the other way round from the Bundle's
    /// `deps_entry_sort_key` order, and unifying the two would change output.
    #[test]
    fn workers_order_capabilities_by_a_plain_sort() {
        assert_eq!(
            deps_caps_in_order(vec!["Db2".to_string(), "Db".to_string()]),
            ["Db", "Db2"]
        );
    }
}
