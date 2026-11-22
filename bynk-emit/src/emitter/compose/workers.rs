//! The Cloudflare Workers composition root (v0.8 §4.5, v0.9 §5.1).
//!
//! Each Worker's `compose.ts` exports a `compose(env)` function that
//! assembles the context's deps and returns the surface the entry point
//! invokes — `on call` services for the internal Service Binding protocol
//! plus `on http` route wrappers for the external HTTP router.
//!
//! Arc C slice 3 (#1321): `emit_worker_compose` and its private helpers build
//! a real `bynk_ts::TsProgram` directly instead of `writeln!`-ing a `String`.
//! Back-half paydown 1B.1a moved them here from `workers.rs`, unchanged.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::emit_model::{ImportExt, LocaleNegotiationArgs};
use crate::emitter::RuntimeUse;
use crate::emitter::workers::*;
use crate::emitter::wrangler::{
    BindingSource, WorkerBinding, binding_ts_type, events_fanout_binding,
};
use bynk_check::symbols::MessageBundleInfo;
use bynk_check::symbols::UnitTable;
use bynk_ts::{
    TsArrowBody, TsDecl, TsExpr, TsObjectEntry, TsParam, TsProgram, TsStmt, TsType, TsTypeMember,
};

/// Where `compose.ts` imports the runtime from — it sits two levels below the
/// out root (`<out>/workers/<worker>/compose.ts`).
///
/// Named because the emitted import line and the post-pass that injects into it
/// (#914) must agree exactly: [`crate::emitter::inject_runtime_imports`]
/// (the still-`String`-based post-pass every other module uses) anchors on
/// the specifier verbatim — #1321's own [`append_missing_bindings`] below
/// reimplements the same dedup directly over the built `TsDecl::Import`
/// node's `names`, since this file's own import line is now a tree node,
/// not text to splice into. Two drifting literals would silently stop
/// injecting rather than fail loudly — the anchor-drift failure v0.176
/// (#642) already hit once.
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

#[allow(clippy::too_many_arguments)]
pub fn emit_worker_compose(
    context: &str,
    layout: &bynk_project::worker_layout::WorkerLayout,
    table: &UnitTable,
    consumes: &[String],
    aliases: &HashMap<String, String>,
    unit_tables: &HashMap<String, UnitTable>,
    // v0.17: adapter unit name → its binding module path (relative to the out
    // root, `.js`). An adapter's external provider class lives in this module,
    // not in a `handlers.ts`.
    binding_modules: &HashMap<String, String>,
    // v0.17: bare flattened capability → the unit it was flattened from.
    flattened: &HashMap<String, String>,
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
    // #1187's slice 6 plumbing: `unit_table_uses_emit(table, callees)`,
    // precomputed by the caller (`crate::project::RunChecks::Checked::
    // unit_callees`'s own doc comment has the full grounding for what feeds
    // it) — reads the checker's own already-resolved `Callee` classification
    // instead of re-deriving `Events.emit` detection from raw AST syntax. A
    // bare `bool`, not the `Callee` map itself: this function has exactly
    // one use for it.
    uses_emit: bool,
    // #1944: every binding this Worker declares (`wrangler::worker_bindings`),
    // the list its `wrangler.toml` is written from; `Env` types exactly it.
    bindings: &[WorkerBinding],
    // #2137: the test build of a context a `system` harness stands up — its
    // first-party `Clock` reads the harness clock (`ProviderNamespaces::Workers`).
    harness_clock: bool,
    // #2719: whether any of this context's service handlers hands work to the
    // execution context (`detach`, `within` or `race`, directly or through the
    // functions it calls), so `compose` takes the request's and threads its
    // `waitUntil` into `deps.__exec`. From the checker's needs-context set.
    ctx_uses_send: bool,
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
    // v0.15: cross-context capabilities this Worker uses (in handlers or in a
    // local provider's `given`) → `deps_key → consumed_context`. Their
    // providers are instantiated locally (model A1).
    let cross_caps = worker_cross_caps(context, table, consumes, aliases, flattened);

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
    let mut derived_keys: Vec<String> = table.derived.keys().cloned().collect();
    derived_keys.sort();
    for cap in &derived_keys {
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
    let mut sorted_consumes: Vec<&String> = consumes
        .iter()
        .filter(|t| !binding_modules.contains_key(*t))
        .collect();
    sorted_consumes.sort();

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
    let order = crate::emitter::topo_order_providers(&table.providers);
    for cap in &order {
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
    let mut deps_entries: Vec<TsObjectEntry> = {
        let mut caps: Vec<String> = order.clone();
        caps.extend(cross_keys.iter().map(|k| (*k).clone()));
        caps.extend(derived_keys.iter().cloned());
        caps.sort();
        caps.into_iter().map(TsObjectEntry::Shorthand).collect()
    };
    // env passes through for cross-context calls (Service Bindings) and agent instantiations (DOs).
    if !sorted_consumes.is_empty() || !table.agents.is_empty() {
        deps_entries.push(TsObjectEntry::Shorthand("env".to_string()));
    }
    deps_entries.extend(session_deps_entry(table));
    // v0.79: the execution context rides in `deps.__exec` for `detach` sends.
    if ctx_uses_send {
        deps_entries.push(TsObjectEntry::prop("__exec".to_string(), ident("exec")));
    }
    // Events track, slice 0 (spine #936, ADR 0284): a publishing handler's
    // release-at-commit event batch is handed to this context's own fan-out
    // DO — `env.<bind>` is typed by the `Env` interface built above, one
    // instance per publishing context.
    if uses_emit {
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
        deps_entries.push(TsObjectEntry::prop("__eventsDispatch".to_string(), arrow));
    }
    // #1321: the pre-conversion `writeln!(out, "  const deps = {{ {} }};",
    // deps_entries.join(", "))` template always has a space on each side of
    // its `{}` slot — with zero entries that literally produces `"{  }"`
    // (a *double* space, not the tight `"{}"` the ordinary single-line
    // `TsExpr::Object` empty-entries shortcut renders), a real, reachable
    // shape (a Worker with no providers/cross-caps/consumed services/agents,
    // no `detach`, no events — `941_http_result_float_bytes_serialise` and
    // several other real fixtures hit exactly this). The shortcut itself is
    // correct and stays unchanged for every *other* real call site (e.g.
    // `events_fanout.rs`'s own `(env ?? {})`, which genuinely wants the
    // tight form) — this one site's own historical quirk is carried as
    // opaque pre-rendered text instead, the same "existing textual variant
    // for a fixed, non-generalizable literal shape" precedent used
    // elsewhere in this file (`exec`'s type, `DurableObjectNamespace`'s
    // type).
    let deps_init = if deps_entries.is_empty() {
        ident("{  }")
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
                    match bynk_model::lower_actor_seam_ir(h, &table.actors) {
                        bynk_model::ActorSeamIr::Oidc(oidc) => {
                            return_entries
                                .push(emit_http_oidc_wrapper(sname, h, method, &path, &oidc));
                        }
                        bynk_model::ActorSeamIr::SignedCookie(seam) => return_entries
                            .push(signed_cookie_entry(service, sname, h, method, &path, &seam)),
                        bynk_model::ActorSeamIr::Sum(members) => {
                            return_entries.push(emit_http_sum_wrapper(
                                sname,
                                h,
                                method,
                                &path,
                                &members,
                                service.limits.as_ref().and_then(|p| p.max_body()),
                                &runtime_use,
                            ));
                        }
                        bynk_model::ActorSeamIr::Bearer(seam) => {
                            return_entries.push(emit_http_wrapper(
                                sname,
                                h,
                                method,
                                &path,
                                Some(&seam),
                            ));
                        }
                        bynk_model::ActorSeamIr::Caller(_)
                        | bynk_model::ActorSeamIr::Sender(_)
                        | bynk_model::ActorSeamIr::CallSum(_)
                        | bynk_model::ActorSeamIr::None => {
                            return_entries.push(emit_http_wrapper(sname, h, method, &path, None));
                        }
                    }
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
                    let seam = if sum.is_some() {
                        None
                    } else {
                        bynk_check::actors::bearer_seam_for(h, &table.actors)
                    };
                    let local_agents: HashSet<String> = table.agents.keys().cloned().collect();
                    // #1818: hosting agents that read their key are forwarded it.
                    let key_readers: HashSet<String> = table
                        .agents
                        .iter()
                        .filter(|(_, a)| crate::emitter::emit::agent_reads_key(a))
                        .map(|(n, _)| n.clone())
                        .collect();
                    return_entries.push(emit_websocket_upgrade(
                        sname,
                        h,
                        seam.as_ref(),
                        sum.as_deref(),
                        &local_agents,
                        &key_readers,
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
                    return_entries.push(emit_event_wrapper(sname, h, &service.protocol));
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

/// v0.15: cross-context capabilities this Worker references in handlers or in
/// a local provider's `given`, as `deps_key → consumed_context`.
fn worker_cross_caps(
    context: &str,
    table: &UnitTable,
    consumes: &[String],
    aliases: &HashMap<String, String>,
    flattened: &HashMap<String, String>,
) -> std::collections::BTreeMap<String, String> {
    fn resolve(
        prefix: &str,
        consumes: &[String],
        aliases: &HashMap<String, String>,
    ) -> Option<String> {
        if let Some(q) = aliases.get(prefix) {
            return Some(q.clone());
        }
        if consumes.iter().any(|c| c == prefix) {
            return Some(prefix.to_string());
        }
        None
    }
    let mut out = std::collections::BTreeMap::new();
    let mut givens: Vec<bynk_model::CapRefIr> = Vec::new();
    for s in table.services.values() {
        for h in &s.handlers {
            givens.extend(bynk_model::lower_handler_given_ir(h));
        }
    }
    for a in table.agents.values() {
        for h in &a.handlers {
            givens.extend(bynk_model::lower_handler_given_ir(h));
        }
    }
    for p in table.providers.values() {
        givens.extend(bynk_model::lower_provider_given_ir(p));
    }
    for c in &givens {
        // Events track, slice 0 (spine #936): `Events.emit` is
        // intercepted entirely at the call site (release-at-commit
        // buffering) and never calls through a constructed provider —
        // see the matching skip in `bynk-emit/src/project.rs`'s
        // `handler_cross_caps`. Without this, Workers-mode compose
        // tries to construct a `bynk__binding.EventsProvider` that
        // does not exist.
        if bynk_check::firstparty::resolve(&c.name, context, flattened)
            == Some(bynk_check::firstparty::FirstPartyCap::Events)
        {
            continue;
        }
        if let Some(p) = &c.context {
            if let Some(ctx) = resolve(p, consumes, aliases) {
                out.entry(c.name.clone()).or_insert(ctx);
            }
        } else if let Some(unit) = flattened.get(&c.name) {
            // v0.17: a bare flattened capability is provided by its source unit.
            out.entry(c.name.clone()).or_insert_with(|| unit.clone());
        }
    }
    out
}
