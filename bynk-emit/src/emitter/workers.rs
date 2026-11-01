//! Per-Worker composition root generation (v0.8 §4.5, v0.9 §5.1).
//!
//! Each Worker's `compose.ts` exports a `compose(env)` function that
//! assembles the context's deps and returns the surface the entry point
//! invokes — `on call` services for the internal Service Binding protocol
//! plus `on http` route wrappers for the external HTTP router.
//!
//! Arc C slice 3 (#1321): `emit_worker_compose` and its nine private
//! helpers build a real `bynk_ts::TsProgram` directly instead of
//! `writeln!`-ing a `String` — this file's own conversion, closing the
//! third Arc C slice (`events_fanout.rs` was the first, #1317).

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::emitter::http_handler_method_name_ir;
use crate::emitter::ts_ident;
use crate::emitter::wrangler::{
    BindingSource, WorkerBinding, agent_binding, binding_ts_type, events_fanout_binding,
};
use crate::emitter::{
    BOUNDARY_CODEC_RUNTIME_IMPORTS, BYTES_RUNTIME_IMPORTS, JSON_CODEC_RUNTIME_IMPORTS, RuntimeUse,
};
use crate::project::{ImportExt, LocaleNegotiationArgs, UnitTable};
use bynk_check::symbols::MessageBundleInfo;
use bynk_syntax::ast::{ActorDecl, ExprKind, Handler, ServiceProtocol, TypeRef};
use bynk_ts::{
    TsArrowBody, TsBindingName, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsProgram, TsStmt,
    TsType, TsTypeMember,
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

// -- Small tree-construction helpers (#1321) -----------------------------
//
// `workers.rs` is by far the largest single Arc C conversion so far (13
// wrapper/helper functions, ~1000 lines of `writeln!` before this slice) —
// these exist purely to keep the conversion below readable and to avoid
// repeating the same `Box::new`/field-name boilerplate at dozens of call
// sites, the same reason `bynk-ts`'s own `TsExpr`/`TsStmt` associated
// constructors exist. Not part of the public node algebra — `bynk-ts` still
// owns every real constructor; these just compose them for this file's own
// repeated shapes.

fn ident(s: impl Into<String>) -> TsExpr {
    TsExpr::Ident(s.into())
}

fn str_lit(s: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Str(s.into()))
}

fn num_lit(n: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Num(n.into()))
}

fn null_lit() -> TsExpr {
    TsExpr::Lit(TsLit::Null)
}

fn member(object: TsExpr, property: impl Into<String>) -> TsExpr {
    TsExpr::Member {
        object: Box::new(object),
        property: property.into(),
    }
}

/// `base.a.b. …` — a left-associative chain of plain member accesses, e.g.
/// `member_chain(ident("handlers"), &[sname, "call"])` for
/// `handlers.<sname>.call`.
fn member_chain(base: TsExpr, properties: &[&str]) -> TsExpr {
    properties.iter().fold(base, |acc, p| member(acc, *p))
}

fn call(callee: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(callee),
        args,
    }
}

fn method_call(object: TsExpr, method: &str, args: Vec<TsExpr>) -> TsExpr {
    call(member(object, method), args)
}

fn new_expr(callee: &str, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::New {
        callee: Box::new(ident(callee)),
        args,
    }
}

fn as_expr(expr: TsExpr, ty: TsType) -> TsExpr {
    TsExpr::As {
        expr: Box::new(expr),
        ty,
    }
}

fn await_expr(expr: TsExpr) -> TsExpr {
    TsExpr::Await(Box::new(expr))
}

fn not_expr(expr: TsExpr) -> TsExpr {
    TsExpr::Unary {
        op: bynk_ts::TsUnaryOp::Not,
        expr: Box::new(expr),
    }
}

fn typeof_expr(expr: TsExpr) -> TsExpr {
    TsExpr::Unary {
        op: bynk_ts::TsUnaryOp::Typeof,
        expr: Box::new(expr),
    }
}

fn binary(op: bynk_ts::TsBinaryOp, left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn strict_eq(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(bynk_ts::TsBinaryOp::StrictEq, left, right)
}

fn strict_neq(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(bynk_ts::TsBinaryOp::StrictNotEq, left, right)
}

fn or_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(bynk_ts::TsBinaryOp::Or, left, right)
}

fn and_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(bynk_ts::TsBinaryOp::And, left, right)
}

fn nullish(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(bynk_ts::TsBinaryOp::NullishCoalescing, left, right)
}

fn return_(expr: Option<TsExpr>) -> TsStmt {
    TsStmt::return_stmt(expr, None)
}

fn const_(name: impl Into<String>, init: TsExpr) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.into()), None, init, None)
}

fn let_(name: impl Into<String>, ty: TsType) -> TsStmt {
    TsStmt::let_stmt(TsBindingName::Ident(name.into()), Some(ty), None, None)
}

fn expr_stmt(expr: TsExpr) -> TsStmt {
    TsStmt::expr_stmt(expr, None)
}

fn if_(cond: TsExpr, then_branch: TsStmt) -> TsStmt {
    TsStmt::if_stmt(cond, then_branch, None)
}

fn block(stmts: Vec<TsStmt>) -> TsStmt {
    TsStmt::block(stmts, None)
}

fn status_object(code: &str) -> TsExpr {
    TsExpr::object(vec![("status".to_string(), num_lit(code))])
}

fn new_response(args: Vec<TsExpr>) -> TsExpr {
    new_expr("globalThis.Response", args)
}

/// `{ ...deps, <key>: <value> }` — the three real "spread `deps`, override
/// one identity-shaped field" sites (Decision A, gap 2): `emit_call_wrapper`'s
/// `identity`, `emit_http_wrapper`/`emit_http_oidc_wrapper`'s `identity`,
/// `emit_http_sum_wrapper`'s `who`.
fn deps_spread_with(key: &str, value: TsExpr) -> TsExpr {
    TsExpr::object_entries(vec![
        TsObjectEntry::Spread(ident("deps")),
        TsObjectEntry::Prop(key.to_string(), value),
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
fn secret_probe_expr(secret: &str) -> TsExpr {
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
    nullish(explicit, global_probe)
}

/// `missing_bindings`'s own dedup logic (`crate::emitter`), reimplemented
/// over a `Vec<String>` of already-real import names instead of over
/// already-printed text — #1321's own real need: `compose.ts`'s header
/// import is now a `TsDecl::Import` node built once, so the post-pass that
/// appends codec-family runtime names (#914) can no longer splice into
/// printed text (`crate::emitter::inject_runtime_imports`'s own mechanism)
/// and instead mutates the node's `names` directly, before `emit_worker_compose`
/// pushes it into the program. Same "strip an optional `type ` prefix,
/// compare bare names" dedup rule, so a name already present (bare or
/// `type`-prefixed) is never added twice — `sum_parses_body`'s own `"type
/// JsonValue"` colliding with `JSON_CODEC_RUNTIME_IMPORTS`'s own `"type
/// JsonValue"` is the real, reachable case this guards.
fn append_missing_bindings(names: &mut Vec<String>, extra: &str) {
    fn bare(binding: &str) -> &str {
        binding
            .trim()
            .strip_prefix("type ")
            .unwrap_or(binding.trim())
    }
    let present: HashSet<&str> = names.iter().map(|n| bare(n)).collect();
    let wanted: Vec<String> = extra
        .split(',')
        .map(str::trim)
        .filter(|b| !b.is_empty() && !present.contains(bare(b)))
        .map(str::to_string)
        .collect();
    names.extend(wanted);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_worker_compose(
    context: &str,
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
    // execution context (`~>`, `within` or `race`, directly or through the
    // functions it calls), so `compose` takes the request's and threads its
    // `waitUntil` into `deps.__exec`. From the checker's needs-context set.
    ctx_uses_send: bool,
) -> (TsProgram, bool) {
    // v0.15: cross-context capabilities this Worker uses (in handlers or in a
    // local provider's `given`) → `deps_key → consumed_context`. Their
    // providers are instantiated locally (model A1).
    let cross_caps = worker_cross_caps(table, consumes, aliases, flattened);

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
            &crate::project::worker_handlers_source_path(context),
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
        let expr = crate::project::instantiate_provider_ts_expr(
            cctx,
            key,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            crate::project::ProviderNamespaces::Workers { harness_clock },
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
        let expr = crate::project::instantiate_provider_ts_expr(
            context,
            cap,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            crate::project::ProviderNamespaces::Workers { harness_clock },
            Some("env"),
            locale_negotiation_args.as_ref(),
            &mut referenced_units,
        );
        cross_cap_exprs.push((cap.clone(), expr));
    }

    // v0.47/v0.52: a context with a Bearer handler — or a sum with a Bearer
    // member — imports the JWT verifier; a sum with a Signature member imports
    // the HMAC verifier. Any verifying wrapper returns `HttpResult` (the 401/400
    // shaping the entry maps).
    let sum_handlers: Vec<Vec<bynk_check::actors::SumMember>> = table
        .services
        .values()
        .flat_map(|s| s.handlers.iter())
        .filter_map(|h| bynk_check::actors::sum_members_for(h, &table.actors))
        .collect();
    use bynk_check::actors::SumMemberSeam;
    let has_bearer = table.services.values().any(|s| {
        s.handlers
            .iter()
            .any(|h| bynk_check::actors::bearer_seam_for(h, &table.actors).is_some())
    }) || sum_handlers
        .iter()
        .flatten()
        .any(|m| matches!(m.seam, SumMemberSeam::Bearer { .. }));
    let has_sum_signature = sum_handlers
        .iter()
        .flatten()
        .any(|m| matches!(m.seam, SumMemberSeam::Signature(_)));
    let has_sum = !sum_handlers.is_empty();
    // v0.151: a single-actor `Oidc` handler's HTTP wrapper fetches the JWKS and
    // verifies an RS256/ES256 JWT before the body runs (fail-closed → 401).
    let has_oidc = table.services.values().any(|s| {
        s.handlers
            .iter()
            .any(|h| bynk_check::actors::oidc_seam_for(h, &table.actors).is_some())
    });
    // A sum wrapper references `JsonValue` only when it parses a `body`.
    let sum_parses_body = sum_handlers.iter().flatten().any(|m| m.needs_body())
        || table
            .services
            .values()
            .flat_map(|s| s.handlers.iter())
            .any(|h| {
                bynk_check::actors::sum_members_for(h, &table.actors).is_some()
                    && h.params.iter().any(|p| p.name.name == "body")
            });
    // #914: the sum wrapper's body codec is emitted *below* the import line, and
    // for anything but a named type it inlines `Ok`/`Err`/`Result`/`BoundaryError`
    // (plus the base64 helpers for a `Bytes`). Those cannot be predicted from the
    // handler signature without restating the codec's own arm table, so they are
    // recorded as the codec emits and injected into the import line as a post-pass
    // — the same shape `emit_project` and `emit_worker_entry` already use.
    let runtime_use = RuntimeUse::default();
    let mut runtime_imports: Vec<String> = Vec::new();
    // #1944: each type an `Env` member names is declared from the same
    // binding list the members come from, so a binding can never be typed
    // by a name this file does not import or define.
    let binds = |kind: fn(&BindingSource) -> bool| bindings.iter().any(|b| kind(&b.source));
    if binds(|s| matches!(s, BindingSource::Kv | BindingSource::KvInstance { .. })) {
        runtime_imports.push("type __KVNamespace".to_string());
    }
    if binds(|s| matches!(s, BindingSource::R2Instance { .. })) {
        runtime_imports.push("type __R2Bucket".to_string());
    }
    if binds(|s| matches!(s, BindingSource::RateLimitInstance { .. })) {
        runtime_imports.push("type __RateLimiter".to_string());
    }
    if binds(|s| matches!(s, BindingSource::QueueProducer { .. })) {
        runtime_imports.push("type __QueueProducer".to_string());
    }
    if binds(|s| matches!(s, BindingSource::AnalyticsDataset { .. })) {
        runtime_imports.push("type __AnalyticsDataset".to_string());
    }
    if binds(|s| matches!(s, BindingSource::D1Database { .. })) {
        runtime_imports.push("type __D1Database".to_string());
    }
    runtime_imports.push("type __ServiceBinding".to_string());
    if has_bearer || has_sum || has_oidc {
        runtime_imports.push("HttpResult".to_string());
    }
    if has_bearer {
        runtime_imports.push("__verifyBearerJwtHs256".to_string());
    }
    if has_oidc {
        runtime_imports.push("__verifyOidcJwt".to_string());
    }
    // #2380 (L5): a capturing `Oidc` seam passes the token's claims on; one
    // with `trustFrom` reads its trust roots from the environment first.
    let oidc_seams: Vec<bynk_check::actors::OidcSeam> = table
        .services
        .values()
        .flat_map(|s| s.handlers.iter())
        .filter_map(|h| bynk_check::actors::oidc_seam_for(h, &table.actors))
        .collect();
    if oidc_seams.iter().any(|seam| seam.binder.is_some()) {
        runtime_imports.push("__tokenClaims".to_string());
    }
    if oidc_seams.iter().any(|seam| seam.trust_from.is_some()) {
        runtime_imports.push("__oidcTrust".to_string());
    }
    if has_sum_signature {
        runtime_imports.push("__verifySignatureHmacSha256".to_string());
    }
    if sum_parses_body {
        runtime_imports.push("type __JsonValue".to_string());
    }
    // A capped sum route reads its body through the counting reader.
    let sum_reads_capped = table.services.values().any(|svc| {
        svc.handlers.iter().any(|h| {
            bynk_check::actors::sum_members_for(h, &table.actors).is_some()
                && bynk_lower::lower_route_limit_ir(h)
                    .or(svc.limits.as_ref().and_then(|p| p.max_body()))
                    .is_some()
                && (h.params.iter().any(|p| p.name.name == "body")
                    || bynk_check::actors::sum_members_for(h, &table.actors)
                        .is_some_and(|ms| ms.iter().any(|m| m.needs_body())))
        })
    });
    if sum_reads_capped {
        runtime_imports.push("__readBodyCapped".to_string());
    }
    // v0.104 (real-time track slice 3b): a `from websocket` upgrade route resolves
    // the hosting Durable Object by serialising the transfer key, exactly as agent
    // call sites do. P6.30 (design/tracks/the-ir.md §6a): reads the checker-
    // classified `IrHandlerKind` (P6.24a's own pure, unconditional mirror) rather
    // than matching the raw AST `HandlerKind` directly.
    let has_ws_open = table.services.values().any(|s| {
        s.handlers.iter().any(|h| {
            matches!(
                bynk_lower::lower_handler_kind_ir(&h.kind),
                bynk_ir::IrHandlerKind::Open
            )
        })
    });
    if has_ws_open {
        runtime_imports.push("__serialiseAgentKey".to_string());
    }
    // Events track, slice 0 (spine #936, ADR 0284): a context whose handlers
    // emit gets its own fan-out DO binding (`emitter::events_fanout`) and a
    // `deps.__eventsDispatch` that calls into it — mirrors `unit_table_uses_
    // emit`'s Bundle-mode gate on `composeApp`'s `__eventsDispatch` closure,
    // so the two targets agree on when the field exists.
    if uses_emit {
        runtime_imports.push("__dispatchToEventsFanout".to_string());
    }
    // #1655 (runtime-semantics track S6): `compose.ts` re-declares each
    // handler's parameters (not its return type), qualifying user types with
    // `handlers.` but naming the runtime's own generic and error types bare.
    // Before, nothing imported them, so an `Option[…]`/`Result[…]` parameter
    // failed `tsc` on this target (TS2749/TS2304) while bundle was clean.
    // Import every runtime type a parameter mentions, as a type; a name already
    // imported as a value (`HttpResult`) covers its type too.
    let mut signature_types: std::collections::BTreeSet<&'static str> = Default::default();
    for service in table.services.values() {
        for h in &service.handlers {
            for p in &h.params {
                collect_runtime_type_names(&p.type_ref, &mut signature_types);
            }
        }
    }
    for name in signature_types {
        if !runtime_imports
            .iter()
            .any(|i| i.trim_start_matches("type ") == name)
        {
            runtime_imports.push(format!("type {name}"));
        }
    }

    // Only consumed *contexts* become Service Bindings — a consumed adapter is
    // not a Worker (its capability is provided in-process via the binding).
    let mut sorted_consumes: Vec<&String> = consumes
        .iter()
        .filter(|t| !binding_modules.contains_key(*t))
        .collect();
    sorted_consumes.sort();

    // v0.79: if any handler in this context uses `~>`, `compose` also takes the
    // request's execution context and threads its `waitUntil` into `deps.__exec`.

    // -- Build the `compose` function's own body (Decision A/B: real nodes
    // throughout) before assembling the header — `runtime_imports`' own
    // post-pass (below) needs `runtime_use`'s final state, which only
    // exists once every wrapper has been built. --

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
    // env passes through so handlers' cross-context calls (Service Bindings)
    // and agent instantiations (Durable Object namespaces) can reach it.
    if !sorted_consumes.is_empty() || !table.agents.is_empty() {
        deps_entries.push(TsObjectEntry::Shorthand("env".to_string()));
    }
    // v0.79: the execution context rides in `deps.__exec` for `~>` sends.
    if ctx_uses_send {
        deps_entries.push(TsObjectEntry::Prop("__exec".to_string(), ident("exec")));
    }
    // Events track, slice 0 (spine #936, ADR 0284): a publishing handler's
    // release-at-commit event batch is handed to this context's own fan-out
    // DO — `env.<bind>` is typed by the `Env` interface built above, one
    // instance per publishing context.
    if uses_emit {
        let bind = events_fanout_binding().name;
        let arrow = TsExpr::Arrow {
            params: vec![TsParam {
                name: "events".to_string(),
                ty: Some(TsType::named_with_args(
                    "globalThis.Array",
                    vec![TsType::named(crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE)],
                )),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(call(
                ident("__dispatchToEventsFanout"),
                vec![member(ident("env"), bind), ident("events")],
            )))),
        };
        deps_entries.push(TsObjectEntry::Prop("__eventsDispatch".to_string(), arrow));
    }
    // #1321: the pre-conversion `writeln!(out, "  const deps = {{ {} }};",
    // deps_entries.join(", "))` template always has a space on each side of
    // its `{}` slot — with zero entries that literally produces `"{  }"`
    // (a *double* space, not the tight `"{}"` the ordinary single-line
    // `TsExpr::Object` empty-entries shortcut renders), a real, reachable
    // shape (a Worker with no providers/cross-caps/consumed services/agents,
    // no `~>`, no events — `941_http_result_float_bytes_serialise` and
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
            match bynk_lower::lower_handler_kind_ir(&h.kind) {
                bynk_ir::IrHandlerKind::Call => {
                    return_entries.push(emit_call_wrapper(sname, h, &table.actors));
                }
                bynk_ir::IrHandlerKind::Http { method, path } => {
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
                    match bynk_lower::lower_actor_seam_ir(h, &table.actors) {
                        bynk_ir::ActorSeamIr::Oidc(oidc) => {
                            return_entries
                                .push(emit_http_oidc_wrapper(sname, h, method, &path, &oidc));
                        }
                        bynk_ir::ActorSeamIr::Sum(members) => {
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
                        bynk_ir::ActorSeamIr::Bearer(seam) => {
                            return_entries.push(emit_http_wrapper(
                                sname,
                                h,
                                method,
                                &path,
                                Some(&seam),
                            ));
                        }
                        bynk_ir::ActorSeamIr::Caller(_)
                        | bynk_ir::ActorSeamIr::CallSum(_)
                        | bynk_ir::ActorSeamIr::None => {
                            return_entries.push(emit_http_wrapper(sname, h, method, &path, None));
                        }
                    }
                }
                bynk_ir::IrHandlerKind::Cron { .. } => {
                    return_entries.push(emit_cron_wrapper(sname, cron_idx, h));
                    cron_idx += 1;
                }
                bynk_ir::IrHandlerKind::Message => {
                    // v0.106 (slice 3b-iii): a `from websocket` `on message` is an
                    // *inbound* handler that runs in the connection-hosting Durable
                    // Object (`webSocketMessage`), not at the edge — no compose
                    // wrapper. A `from queue` `on message` is the queue consumer.
                    // Still a raw `ServiceProtocol` match (not `ProtocolIr`,
                    // P6.30): `emit_worker_compose` has no `TypedCommons` in
                    // scope to lower it with, unlike `lower_handler_kind_ir`
                    // above, which needs none.
                    if matches!(service.protocol, ServiceProtocol::WebSocket { .. }) {
                        continue;
                    }
                    return_entries.push(emit_queue_wrapper(sname, queue_idx, h));
                    queue_idx += 1;
                }
                bynk_ir::IrHandlerKind::Open => {
                    let seam = bynk_check::actors::bearer_seam_for(h, &table.actors);
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
                        &local_agents,
                        &key_readers,
                    ));
                }
                // v0.106 (slice 3b-iii): `on close` runs in the DO (`webSocketClose`),
                // not at the edge — no compose wrapper.
                bynk_ir::IrHandlerKind::Close => {}
                // Events track, slice 0 (spine #936): unlike the WS
                // lifecycle handlers above, an `on event` handler's body
                // *does* need a compose-surface wrapper — it is reached from
                // `/_bynk/event/<sname>` (`emit_worker_entry`), the route a
                // *subscriber* Worker's own fan-out delivery lands on (not
                // this context's edge traffic, and not HTTP-routable, hence
                // no route table entry — just a plain wrapper like `on
                // call`'s).
                bynk_ir::IrHandlerKind::Event => {
                    return_entries.push(emit_event_wrapper(sname, h, &service.protocol));
                }
                // #2341 (decision C): the router calls `rejected_<svc>(why)`
                // to render a rejection of one of this service's requests.
                bynk_ir::IrHandlerKind::Rejected => {
                    return_entries.push(emit_rejected_wrapper(sname));
                }
            }
        }
    }
    body.push(return_(Some(TsExpr::multiline_object_entries(
        return_entries,
    ))));

    // #914: fold in whatever the wrappers' codecs actually reached for —
    // the tree-node analogue of the old text post-pass (this file's own
    // `append_missing_bindings`'s own doc explains why it can't reuse
    // `crate::emitter::inject_runtime_imports` unchanged).
    if runtime_use.boundary_codec() {
        append_missing_bindings(&mut runtime_imports, BOUNDARY_CODEC_RUNTIME_IMPORTS);
    }
    if runtime_use.json_codec() {
        append_missing_bindings(&mut runtime_imports, JSON_CODEC_RUNTIME_IMPORTS);
    }
    if runtime_use.bytes() {
        append_missing_bindings(&mut runtime_imports, BYTES_RUNTIME_IMPORTS);
    }
    if runtime_use.bytes_ops() {
        append_missing_bindings(
            &mut runtime_imports,
            crate::emitter::BYTES_OPS_RUNTIME_IMPORTS,
        );
    }
    if runtime_use.time_ops() {
        append_missing_bindings(
            &mut runtime_imports,
            crate::emitter::TIME_OPS_RUNTIME_IMPORTS,
        );
    }
    if runtime_use.settle_all() {
        append_missing_bindings(
            &mut runtime_imports,
            crate::emitter::SETTLE_ALL_RUNTIME_IMPORTS,
        );
    }
    if runtime_use.within() {
        append_missing_bindings(&mut runtime_imports, crate::emitter::WITHIN_RUNTIME_IMPORTS);
    }
    if runtime_use.race() {
        append_missing_bindings(&mut runtime_imports, crate::emitter::RACE_RUNTIME_IMPORTS);
    }
    if runtime_use.fault() {
        append_missing_bindings(&mut runtime_imports, crate::emitter::FAULT_RUNTIME_IMPORTS);
    }
    if runtime_use.url_ops() {
        append_missing_bindings(
            &mut runtime_imports,
            crate::emitter::URL_OPS_RUNTIME_IMPORTS,
        );
    }
    if runtime_use.request_headers() {
        append_missing_bindings(
            &mut runtime_imports,
            crate::emitter::REQUEST_HEADERS_RUNTIME_IMPORTS,
        );
    }
    if runtime_use.eq() {
        append_missing_bindings(&mut runtime_imports, crate::emitter::EQ_RUNTIME_IMPORTS);
    }
    if runtime_use.int() {
        append_missing_bindings(&mut runtime_imports, crate::emitter::INT_RUNTIME_IMPORTS);
    }
    append_missing_bindings(&mut runtime_imports, &runtime_use.cmp_imports());

    let mut compose_params = vec![TsParam {
        name: "env".to_string(),
        ty: Some(TsType::named("Env")),
        optional: false,
    }];
    // Locale capability track, slice 2 (#882): `request` is threaded
    // independently of `exec` — the two-way independence matters because
    // `scheduled`/`queue` (which never have a `request`) may still use
    // `~>` (`exec`), and a `fetch`-only Worker with no message bundle needs
    // neither.
    if needs_request {
        compose_params.push(TsParam {
            name: "request".to_string(),
            ty: Some(TsType::named("globalThis.Request")),
            optional: true,
        });
    }
    // L7 (#2414, Decision E): an `EdgeCache` instance is built from the
    // entry's `ctx` (`new WorkersEdgeCache(exec)`), so compose takes it even
    // when nothing sends; optional then, as the entry's own `ctx` is.
    let edge_cache = table.derived.values().any(|d| d.is_edge_cache());
    if edge_cache && !ctx_uses_send {
        compose_params.push(TsParam {
            name: "exec".to_string(),
            ty: Some(TsType::named("unknown")),
            optional: true,
        });
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
        });
    }

    // -- Assemble the whole document in its real, final order. --
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynkc — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::comment(
        format!("composition root for `{context}` Worker."),
        None,
    ));
    for stmt in crate::emitter::runtime_files::runtime_import_stmts(
        false,
        &runtime_imports,
        COMPOSE_RUNTIME_SPECIFIER,
    ) {
        program.push(stmt);
    }
    program.push(TsStmt::decl(
        TsDecl::ImportNamespace {
            type_only: false,
            alias: "handlers".to_string(),
            from: "./handlers.js".to_string(),
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
            let dir = crate::project::worker_dir_name(cctx);
            program.push(TsStmt::decl(
                TsDecl::ImportNamespace {
                    type_only: false,
                    alias: format!("handlers_{ns}"),
                    from: format!("../{dir}/handlers.js"),
                },
                None,
            ));
        }
    }
    if let Some(li) = locale_import {
        program.push(li);
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
            BindingSource::Agent { .. } | BindingSource::EventsFanout { .. }
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

    (program, needs_request)
}

/// v0.15: cross-context capabilities this Worker references in handlers or in
/// a local provider's `given`, as `deps_key → consumed_context`.
fn worker_cross_caps(
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
    let mut givens: Vec<bynk_ir::CapRefIr> = Vec::new();
    for s in table.services.values() {
        for h in &s.handlers {
            givens.extend(bynk_lower::lower_handler_given_ir(h));
        }
    }
    for a in table.agents.values() {
        for h in &a.handlers {
            givens.extend(bynk_lower::lower_handler_given_ir(h));
        }
    }
    for p in table.providers.values() {
        givens.extend(bynk_lower::lower_provider_given_ir(p));
    }
    for c in &givens {
        // Events track, slice 0 (spine #936): `Events.emit` is
        // intercepted entirely at the call site (release-at-commit
        // buffering) and never calls through a constructed provider —
        // see the matching skip in `bynk-emit/src/project.rs`'s
        // `handler_cross_caps`. Without this, Workers-mode compose
        // tries to construct a `bynk__binding.EventsProvider` that
        // does not exist.
        if c.name == "Events" && flattened.get(&c.name).map(String::as_str) == Some("bynk") {
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
fn emit_call_wrapper(
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
        .map(|p| TsParam {
            name: ts_ident(&p.name.name),
            ty: Some(crate::emitter::ts_type_ref_qualified_ts_type(
                &p.type_ref,
                &scope,
                "handlers",
            )),
            optional: false,
        })
        .collect();
    let param_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(ts_ident(&p.name.name)))
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
            TsParam {
                name: "__who".to_string(),
                ty: Some(crate::emitter::emit::call_sum_ts_type(members)),
                optional: false,
            },
        );
        deps_spread_with("who", ident("__who"))
    } else if bynk_check::actors::caller_binder_for(h, actors).is_some() {
        params.insert(
            0,
            TsParam {
                name: "__caller".to_string(),
                ty: Some(TsType::named("string")),
                optional: false,
            },
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
fn emit_rejected_wrapper(sname: &str) -> TsObjectEntry {
    TsObjectEntry::Method {
        name: format!("rejected_{sname}"),
        is_async: true,
        generics: Vec::new(),
        params: vec![TsParam {
            name: "why".to_string(),
            ty: Some(TsType::named(
                "{ readonly tag: string; readonly detail: string }",
            )),
            optional: false,
        }],
        return_type: None,
        doc: None,
        inline: false,
        body: vec![return_(Some(call(
            member_chain(ident("handlers"), &[sname, "rejected"]),
            vec![ident("why"), ident("deps")],
        )))],
    }
}

fn emit_event_wrapper(sname: &str, h: &Handler, protocol: &ServiceProtocol) -> TsObjectEntry {
    let wants_envelope = h.params.len() == 2
        || matches!(
            protocol,
            ServiceProtocol::Events {
                schema_dispatch: Some(_),
                ..
            }
        );
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
            TsParam {
                name: "payload".to_string(),
                ty: Some(TsType::named("unknown")),
                optional: false,
            },
            TsParam {
                name: "envelope".to_string(),
                ty: Some(TsType::named("unknown")),
                optional: false,
            },
        ],
        return_type: None,
        doc: None,
        inline: false,
        body: vec![return_(Some(call(
            member_chain(ident("handlers"), &[sname, "event"]),
            args,
        )))],
    }
}

fn emit_cron_wrapper(sname: &str, cron_idx: usize, h: &Handler) -> TsObjectEntry {
    let method_key = crate::emitter::cron_handler_method_name(sname, cron_idx);
    // A cron handler takes an optional scheduled-time parameter; forward it (if
    // any) to the bound handler, with deps trailing. P7.2: the checker requires
    // this parameter be `Int`, and `workers_entry.rs`'s scheduled() dispatch hands
    // it Cloudflare's own `event.scheduledTime: number` directly — codec-matched,
    // safe to type for real.
    let params: Vec<TsParam> = h
        .params
        .iter()
        .map(|p| TsParam {
            name: ts_ident(&p.name.name),
            ty: Some(qualified_ts_type_ref(&p.type_ref)),
            optional: false,
        })
        .collect();
    let mut call_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(ts_ident(&p.name.name)))
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
    }
}

fn emit_queue_wrapper(sname: &str, queue_idx: usize, h: &Handler) -> TsObjectEntry {
    let method_key = crate::emitter::queue_handler_method_name(sname, queue_idx);
    // The queue handler takes its message parameter; forward it with deps. P7.2:
    // `workers_entry.rs`'s queue() dispatch always calls `deserialise_call` against
    // this same declared type before forwarding `__r.value` here — codec-matched,
    // safe to type for real. (No param at all when the handler declares none, in
    // which case this list — and the corresponding entry-side msg_type — is empty.)
    let params: Vec<TsParam> = h
        .params
        .iter()
        .map(|p| TsParam {
            name: ts_ident(&p.name.name),
            ty: Some(qualified_ts_type_ref(&p.type_ref)),
            optional: false,
        })
        .collect();
    let mut call_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(ts_ident(&p.name.name)))
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
fn emit_websocket_upgrade(
    sname: &str,
    h: &Handler,
    seam: Option<&bynk_check::actors::BearerSeam>,
    local_agents: &HashSet<String>,
    key_readers: &HashSet<String>,
) -> TsObjectEntry {
    use crate::emitter::websocket::{WsOpenShape, analyse_open_shape};
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
    let mut params: Vec<TsParam> = vec![TsParam {
        name: "request".to_string(),
        ty: Some(TsType::named("globalThis.Request")),
        optional: false,
    }];
    params.extend(h.params.iter().map(|p| TsParam {
        name: ts_ident(&p.name.name),
        ty: Some(TsType::named("string")),
        optional: false,
    }));
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
            let js = bynk_check::actors::claim_predicate_to_js(pred, "__claims.value.claims");
            // #1321: `claim_predicate_to_js` is a `bynk_check`-crate helper
            // that returns already-built JS boolean-expression *text* —
            // out of this slice's own scope (a different crate; `bynk-ts`'s
            // own crate-boundary invariant, `bynk-syntax` only, forbids
            // `bynk-check` depending on it to return a real `TsExpr`
            // instead). Carried as an opaque `TsExpr::Ident` — the same
            // "existing textual variant carries pre-rendered text this
            // crate structurally can't build a shape for" precedent
            // `ts_type_ref_to_ts_type`'s own `TypeRef::Query` arm already
            // established for `TsType::Named` (`emitter.rs`), applied here
            // to `TsExpr::Ident` — not a new pattern. The literal parens
            // are baked into the text (not left to `Unary::Not`'s own
            // operand rules) since `js`'s own precedence isn't guaranteed
            // compatible with a bare `!`, matching the original `writeln!`
            // text's own explicit `!({js})`.
            stmts.push(if_(
                ident(format!("!({js})")),
                return_(Some(new_response(vec![
                    str_lit("Forbidden"),
                    status_object("403"),
                ]))),
            ));
        }
    }

    // DECISION B: resolve the hosting Durable Object from the single connection
    // transfer (`Room(roomId)` → the `ROOM` namespace, keyed by `roomId`). The
    // shape constraint guarantees exactly one routable target.
    let target = match analyse_open_shape(&h.body, local_agents) {
        WsOpenShape::One(t) => t,
        // The checker rejects zero / multiple / non-routable shapes
        // (`bynk.ws.open_transfer_shape`); this arm is defensive.
        _ => {
            stmts.push(return_(Some(new_response(vec![
                str_lit("Internal Server Error"),
                status_object("500"),
            ]))));
            return TsObjectEntry::Method {
                name: method_name,
                is_async: true,
                generics: Vec::new(),
                params,
                return_type: None,
                doc: None,
                inline: false,
                body: stmts,
            };
        }
    };
    let binding = agent_binding(target.agent).name;
    let key_js = match &target.key.kind {
        ExprKind::Ident(id) => id.name.clone(),
        // v1 keys are request-derivable param idents (DECISION B); a non-ident key
        // falls back to the first route param so the route stays valid TS.
        _ => h
            .params
            .first()
            .map(|p| p.name.name.clone())
            .unwrap_or_else(|| "\"default\"".to_string()),
    };
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
        let jn = ts_ident(pn);
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
                            vec![TsExpr::object(vec![
                                ("kind".to_string(), str_lit("RefinementViolation")),
                                ("path".to_string(), str_lit(format!("param.{pn}"))),
                                (
                                    "violation".to_string(),
                                    member(ident(r_name.clone()), "error"),
                                ),
                            ])],
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
            _ => {
                validated.insert(pn.clone(), ident(jn));
            }
        }
    }
    let key_ref = validated.get(&key_js).cloned().unwrap_or(ident(key_js));
    let args_json: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| {
            validated
                .get(&p.name.name)
                .cloned()
                .unwrap_or_else(|| ident(ts_ident(&p.name.name)))
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
        new_expr(
            "globalThis.Headers",
            vec![member(ident("request"), "headers")],
        ),
    ));
    let mut fwd_entries = vec![("args".to_string(), TsExpr::array(args_json))];
    if has_identity {
        fwd_entries.push(("identity".to_string(), member(ident("__id"), "value")));
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
        vec![new_expr(
            "globalThis.Request",
            vec![
                str_lit(format!("https://_bynk/_bynk/ws/open/{sname}")),
                TsExpr::object(vec![
                    ("method".to_string(), member(ident("request"), "method")),
                    ("headers".to_string(), ident("__fwd")),
                ]),
            ],
        )],
    ))));

    TsObjectEntry::Method {
        name: method_name,
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: None,
        doc: None,
        inline: false,
        body: stmts,
    }
}

// `stmts`/`secret_body` in the functions below are built incrementally —
// unconditional pushes followed by later conditional ones, not a fixed
// literal clippy's suggested `vec![]` could replace.
#[allow(clippy::vec_init_then_push)]
fn emit_http_wrapper(
    sname: &str,
    h: &Handler,
    method: bynk_ir::IrHttpMethod,
    path: &str,
    seam: Option<&bynk_check::actors::BearerSeam>,
) -> TsObjectEntry {
    let method_key = http_handler_method_name_ir(method, path);
    // Route params (and the `body`) forward positionally; `ts_ident` keeps a
    // reserved-word param name from emitting an invalid binder (#723).
    let param_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(ts_ident(&p.name.name)))
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
        let mut params: Vec<TsParam> = vec![TsParam {
            name: "request".to_string(),
            ty: Some(TsType::named("globalThis.Request")),
            optional: false,
        }];
        params.extend(h.params.iter().map(|p| TsParam {
            name: ts_ident(&p.name.name),
            ty: Some(qualified_ts_type_ref(&p.type_ref)),
            optional: false,
        }));
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
            return_(Some(member(ident("HttpResult"), "Unauthorized"))),
        ));
        // Source the signing secret from the same env the `Secrets` capability
        // reads (explicit env first, then a `process.env` probe).
        stmts.push(const_("__secret", secret_probe_expr(&secret)));
        stmts.push(if_(
            strict_neq(typeof_expr(ident("__secret")), str_lit("string")),
            return_(Some(member(ident("HttpResult"), "Unauthorized"))),
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
            return_(Some(member(ident("HttpResult"), "Unauthorized"))),
        ));
        // v0.53: a refinement actor's authorisation invariant — the scheme
        // verified (else 401 above), so a failed claim predicate is 403, not
        // 401. Checked against the *verified* claims, before the identity mints
        // or the body runs.
        if let Some(pred) = &seam.authorization {
            let js = bynk_check::actors::claim_predicate_to_js(pred, "__claims.value.claims");
            // #1321: see `emit_websocket_upgrade`'s own identical construction
            // for why this is an opaque `Ident` carrying pre-built text.
            stmts.push(if_(
                ident(format!("!({js})")),
                return_(Some(member(ident("HttpResult"), "Forbidden"))),
            ));
        }
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
                return_(Some(member(ident("HttpResult"), "Unauthorized"))),
            ));
            let mut call_args = param_args.clone();
            call_args.push(deps_spread_with("identity", member(ident("__id"), "value")));
            stmts.push(return_(Some(call(
                member_chain(ident("handlers"), &[sname, &method_key]),
                call_args,
            ))));
        } else {
            // Binder-less: the token is verified (fail-closed above); the
            // identity is not captured, so call the handler with plain deps.
            let mut call_args = param_args.clone();
            call_args.push(ident("deps"));
            stmts.push(return_(Some(call(
                member_chain(ident("handlers"), &[sname, &method_key]),
                call_args,
            ))));
        }
        return TsObjectEntry::Method {
            name: method_key,
            is_async: true,
            generics: Vec::new(),
            params,
            return_type: None,
            doc: None,
            inline: false,
            body: stmts,
        };
    }

    // P7.2: real declared types — same correction as the Bearer branch above.
    let params: Vec<TsParam> = h
        .params
        .iter()
        .map(|p| TsParam {
            name: ts_ident(&p.name.name),
            ty: Some(qualified_ts_type_ref(&p.type_ref)),
            optional: false,
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
    }
}

/// v0.151: the compose wrapper for a single-actor `Oidc` HTTP handler. It reads
/// the `Authorization: Bearer <token>`, verifies the JWT against the provider's
/// JWKS (RS256/ES256, `iss`/`aud`/`exp`/`nbf`) via `verifyOidcJwt`, mints the
/// identity from the verified `sub`, and threads it into `deps` — all before the
/// body runs. Any failure returns `Unauthorized` (401), fail-closed. Unlike the
/// Bearer wrapper it sources **no secret**: the trust parameters are the public
/// `issuer`/`audience`/`jwks` literals from the actor declaration.
#[allow(clippy::vec_init_then_push)]
fn emit_http_oidc_wrapper(
    sname: &str,
    h: &Handler,
    method: bynk_ir::IrHttpMethod,
    path: &str,
    seam: &bynk_check::actors::OidcSeam,
) -> TsObjectEntry {
    let method_key = http_handler_method_name_ir(method, path);
    let param_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| ident(ts_ident(&p.name.name)))
        .collect();
    let issuer = crate::emitter::escape_ts_string(&seam.issuer);
    let audience = crate::emitter::escape_ts_string(&seam.audience);
    let jwks = crate::emitter::escape_ts_string(&seam.jwks);

    // P7.2: real declared types — same correction as `emit_http_wrapper`'s own.
    let mut params: Vec<TsParam> = vec![TsParam {
        name: "request".to_string(),
        ty: Some(TsType::named("globalThis.Request")),
        optional: false,
    }];
    params.extend(h.params.iter().map(|p| TsParam {
        name: ts_ident(&p.name.name),
        ty: Some(qualified_ts_type_ref(&p.type_ref)),
        optional: false,
    }));
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
        return_(Some(member(ident("HttpResult"), "Unauthorized"))),
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
                return_(Some(member(ident("HttpResult"), "Unauthorized"))),
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
        return_(Some(member(ident("HttpResult"), "Unauthorized"))),
    ));
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
            return_(Some(member(ident("HttpResult"), "Unauthorized"))),
        ));
        let mut call_args = param_args;
        // #2380 (L5): the binder's `.claims` read the verified token's claims.
        call_args.push(TsExpr::object_entries(vec![
            TsObjectEntry::Spread(ident("deps")),
            TsObjectEntry::Prop("identity".to_string(), member(ident("__id"), "value")),
            TsObjectEntry::Prop(
                "claims".to_string(),
                call(
                    ident("__tokenClaims"),
                    vec![member(member(ident("__claims"), "value"), "claims")],
                ),
            ),
        ]));
        stmts.push(return_(Some(call(
            member_chain(ident("handlers"), &[sname, &method_key]),
            call_args,
        ))));
    } else {
        let mut call_args = param_args;
        call_args.push(ident("deps"));
        stmts.push(return_(Some(call(
            member_chain(ident("handlers"), &[sname, &method_key]),
            call_args,
        ))));
    }
    TsObjectEntry::Method {
        name: method_key,
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: None,
        doc: None,
        inline: false,
        body: stmts,
    }
}

/// Source a string secret from the same env the `Secrets` capability reads
/// (explicit `env` first, then a `process.env` probe), binding it to `var`.
fn emit_secret_lookup(var: &str, secret: &str) -> TsStmt {
    let secret = crate::emitter::escape_ts_string(secret);
    const_(var, secret_probe_expr(&secret))
}

/// v0.52: the compose wrapper for a **multi-actor sum** handler (`by who: A |
/// B`). Unlike the single-actor wrappers, this one owns the *whole* boundary:
/// it reads the raw body once (when any member needs it or the handler takes a
/// `body`), tries each member's scheme in declared order, binds the first that
/// verifies into a tagged `__who`, and — fail-closed → 401 if none verifies —
/// parses the body and dispatches with `who` threaded through `deps`. The body
/// `match`es on `who`. The entry passes `request` (+ any path params); no body
/// read happens in the entry for a sum route.
#[allow(clippy::too_many_arguments, clippy::vec_init_then_push)]
fn emit_http_sum_wrapper(
    sname: &str,
    h: &Handler,
    method: bynk_ir::IrHttpMethod,
    path: &str,
    members: &[bynk_check::actors::SumMember],
    service_cap: Option<i64>,
    runtime_use: &RuntimeUse,
) -> TsObjectEntry {
    use bynk_check::actors::SumMemberSeam;
    let method_key = http_handler_method_name_ir(method, path);
    // The wrapper takes the request first (it reads the body / headers), then
    // the path params (parsed in the entry and passed through); the `body`
    // param is parsed here, not passed in.
    // `ts_ident`-renamed so a reserved-word path param forwards as a valid binder
    // (#723); the wrapper's decl and its forwarded arg both read from this list.
    // P7.2: carries each param's own `TypeRef` alongside its name — real
    // declared types, not `unknown`, matching the same correction
    // `emit_http_wrapper`'s own path params needed (`workers_entry.rs`'s
    // dispatch validates each one against its declared type before this
    // wrapper ever sees it).
    let path_params: Vec<(String, TypeRef)> = h
        .params
        .iter()
        .filter(|p| p.name.name != "body")
        .map(|p| (ts_ident(&p.name.name), p.type_ref.clone()))
        .collect();
    let mut params: Vec<TsParam> = vec![TsParam {
        name: "request".to_string(),
        ty: Some(TsType::named("globalThis.Request")),
        optional: false,
    }];
    params.extend(path_params.iter().map(|(n, ty)| TsParam {
        name: n.clone(),
        ty: Some(qualified_ts_type_ref(ty)),
        optional: false,
    }));
    let mut stmts: Vec<TsStmt> = Vec::new();

    // Read the raw body once if a member verifies over it (Signature) or the
    // handler takes a `body` param (parsed from the same bytes).
    let has_body = h.params.iter().any(|p| p.name.name == "body");
    let needs_raw = has_body || members.iter().any(|m| m.needs_body());
    // A capped route reads through the counting reader (the entry's
    // `Content-Length` fast-reject stays the cheap first check); a `@raw` route
    // keeps the octets as received for the members to verify and the handler to
    // take, and is always capped.
    let cap = bynk_lower::lower_route_limit_ir(h)
        .or(service_cap)
        .filter(|_| needs_raw);
    let raw = has_body && h.raw_content_type().is_some();
    let bad_body = || {
        return_(Some(call(
            member(ident("HttpResult"), "BadRequest"),
            vec![str_lit("Invalid request body")],
        )))
    };
    if let Some(cap) = cap {
        stmts.push(let_(
            "__bytes",
            TsType::named("globalThis.Uint8Array | undefined"),
        ));
        stmts.push(TsStmt::try_catch(
            block(vec![TsStmt::assign(
                ident("__bytes"),
                await_expr(call(
                    ident("__readBodyCapped"),
                    vec![ident("request"), num_lit(cap.to_string())],
                )),
                None,
            )]),
            None::<String>,
            block(vec![bad_body()]),
            None,
        ));
        stmts.push(if_(
            strict_eq(ident("__bytes"), ident("undefined")),
            return_(Some(call(
                member(ident("HttpResult"), "PayloadTooLarge"),
                vec![str_lit(format!("request body exceeds {cap} bytes"))],
            ))),
        ));
        if !raw {
            stmts.push(const_(
                "__raw",
                method_call(
                    new_expr("globalThis.TextDecoder", vec![]),
                    "decode",
                    vec![ident("__bytes")],
                ),
            ));
        }
    } else if needs_raw {
        stmts.push(let_("__raw", TsType::named("string")));
        stmts.push(TsStmt::try_catch(
            block(vec![TsStmt::assign(
                ident("__raw"),
                await_expr(method_call(ident("request"), "text", vec![])),
                None,
            )]),
            None::<String>,
            block(vec![bad_body()]),
            None,
        ));
    }
    // What a `Signature` member's HMAC covers.
    let signed = if raw { "__bytes" } else { "__raw" };

    // First-wins resolution: try each member in order; the first to verify sets
    // `__who`. A `None` (catch-all) member always succeeds.
    //
    // P7.2: deferred, not narrowed. `__who` is assigned one of two shapes across
    // the loop below — `{ tag }` or `{ tag; identity: <member's own identity type> }`
    // — and is later spread into `deps.who`, which `handlers.ts`'s own generated
    // interface types precisely per member. A correct narrowing needs a real
    // per-member discriminated union built from each `member.seam`'s
    // `identity_type` and checked against what that generated interface actually
    // expects — more than a same-line text change, and risks a `tsc --strict`
    // mismatch if guessed. Left as `any`, named here rather than silently kept.
    stmts.push(TsStmt::let_stmt(
        TsBindingName::Ident("__who".to_string()),
        Some(TsType::named("any")),
        Some(ident("undefined")),
        None,
    ));
    for member_seam in members {
        let tag = crate::emitter::escape_ts_string(&member_seam.actor_name);
        let mut inner: Vec<TsStmt> = Vec::new();
        match &member_seam.seam {
            SumMemberSeam::None => {
                inner.push(TsStmt::assign(
                    ident("__who"),
                    TsExpr::object(vec![("tag".to_string(), str_lit(tag.clone()))]),
                    None,
                ));
            }
            SumMemberSeam::Bearer {
                secret,
                identity_type,
            } => {
                inner.push(const_(
                    "__authz",
                    method_call(
                        member(ident("request"), "headers"),
                        "get",
                        vec![str_lit("Authorization")],
                    ),
                ));
                let mut authz_body: Vec<TsStmt> = Vec::new();
                authz_body.push(emit_secret_lookup("__secret", secret));
                let mut secret_body: Vec<TsStmt> = Vec::new();
                secret_body.push(const_(
                    "__claims",
                    await_expr(call(
                        ident("__verifyBearerJwtHs256"),
                        vec![
                            method_call(ident("__authz"), "slice", vec![num_lit("7")]),
                            ident("__secret"),
                        ],
                    )),
                ));
                secret_body.push(if_(
                    strict_eq(member(ident("__claims"), "tag"), str_lit("Ok")),
                    block(vec![
                        const_(
                            "__id",
                            method_call(
                                member(ident("handlers"), identity_type.clone()),
                                "of",
                                vec![member(member(ident("__claims"), "value"), "sub")],
                            ),
                        ),
                        if_(
                            strict_eq(member(ident("__id"), "tag"), str_lit("Ok")),
                            TsStmt::assign(
                                ident("__who"),
                                TsExpr::object(vec![
                                    ("tag".to_string(), str_lit(tag.clone())),
                                    ("identity".to_string(), member(ident("__id"), "value")),
                                ]),
                                None,
                            ),
                        ),
                    ]),
                ));
                authz_body.push(if_(
                    strict_eq(typeof_expr(ident("__secret")), str_lit("string")),
                    block(secret_body),
                ));
                inner.push(if_(
                    and_expr(
                        strict_neq(ident("__authz"), null_lit()),
                        method_call(ident("__authz"), "startsWith", vec![str_lit("Bearer ")]),
                    ),
                    block(authz_body),
                ));
            }
            SumMemberSeam::Signature(seam) => {
                inner.push(emit_secret_lookup("__secret", &seam.secret));
                let mut secret_body: Vec<TsStmt> = Vec::new();
                let ts_expr = match &seam.timestamp_header {
                    Some(th) => {
                        let th = crate::emitter::escape_ts_string(th);
                        secret_body.push(const_(
                            "__ts",
                            method_call(
                                member(ident("request"), "headers"),
                                "get",
                                vec![str_lit(th)],
                            ),
                        ));
                        ident("__ts")
                    }
                    None => null_lit(),
                };
                let tol = match seam.tolerance_secs {
                    Some(n) => num_lit(n.to_string()),
                    None => null_lit(),
                };
                let header = crate::emitter::escape_ts_string(&seam.header);
                secret_body.push(const_(
                    "__sig_ok",
                    await_expr(call(
                        ident("__verifySignatureHmacSha256"),
                        vec![
                            ident(signed),
                            ident("__secret"),
                            method_call(
                                member(ident("request"), "headers"),
                                "get",
                                vec![str_lit(header)],
                            ),
                            ts_expr,
                            tol,
                        ],
                    )),
                ));
                secret_body.push(if_(
                    ident("__sig_ok"),
                    TsStmt::assign(
                        ident("__who"),
                        TsExpr::object(vec![("tag".to_string(), str_lit(tag.clone()))]),
                        None,
                    ),
                ));
                inner.push(if_(
                    strict_eq(typeof_expr(ident("__secret")), str_lit("string")),
                    block(secret_body),
                ));
            }
        }
        stmts.push(if_(
            strict_eq(ident("__who"), ident("undefined")),
            block(inner),
        ));
    }
    stmts.push(if_(
        strict_eq(ident("__who"), ident("undefined")),
        return_(Some(member(ident("HttpResult"), "Unauthorized"))),
    ));

    // Parse the body param from the raw bytes already read (fail-closed → 400).
    if raw {
        // The octets, untouched: no JSON, no base64.
        stmts.push(const_("body", ident("__bytes")));
    } else if let Some(body_param) = h.params.iter().find(|p| p.name.name == "body") {
        stmts.push(let_("__body_json", TsType::named("__JsonValue")));
        stmts.push(TsStmt::try_catch(
            block(vec![TsStmt::assign(
                ident("__body_json"),
                as_expr(
                    method_call(ident("globalThis.JSON"), "parse", vec![ident("__raw")]),
                    TsType::named("__JsonValue"),
                ),
                None,
            )]),
            None::<String>,
            block(vec![return_(Some(call(
                member(ident("HttpResult"), "BadRequest"),
                vec![str_lit("Invalid request body")],
            )))]),
            None,
        ));
        // #1321: `super::workers_entry::deserialise_call` delegates to
        // `serialisation::deserialise_expr_via` (#1435, Arc E slice 1: a
        // real `bynk_ts::TsExpr` now, not opaque text). Its result feeds the
        // handler directly: since #1704 a `uses`-commons codec's unbranded
        // result fits the context's optionally-branded parameter type.
        let dser = super::workers_entry::deserialise_call(
            &body_param.type_ref,
            "__body_json",
            "$",
            runtime_use,
        );
        stmts.push(const_("__r_body", dser));
        stmts.push(if_(
            strict_eq(member(ident("__r_body"), "tag"), str_lit("Err")),
            return_(Some(call(
                member(ident("HttpResult"), "BadRequest"),
                vec![str_lit("Invalid request body")],
            ))),
        ));
        stmts.push(const_("body", member(ident("__r_body"), "value")));
    }
    // The handler's arguments in its declaration order: `body` (parsed above)
    // wherever it sits, and every other parameter — the path params, and
    // #2341's `headers` — as the entry passed it.
    let mut call_args: Vec<TsExpr> = h
        .params
        .iter()
        .map(|p| {
            if p.name.name == "body" {
                ident("body")
            } else {
                ident(ts_ident(&p.name.name))
            }
        })
        .collect();
    call_args.push(deps_spread_with("who", ident("__who")));
    stmts.push(return_(Some(call(
        member_chain(ident("handlers"), &[sname, &method_key]),
        call_args,
    ))));

    TsObjectEntry::Method {
        name: method_key,
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: None,
        doc: None,
        inline: false,
        body: stmts,
    }
}

/// v0.176 (#642): the user-named types appearing anywhere in a type-ref, including
/// through generic arguments — the set a compose wrapper qualifies against the
/// `handlers` namespace.
fn named_types_in(r: &TypeRef) -> Vec<String> {
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
            _ => {}
        }
    }
    walk(r, &mut out);
    out
}

/// #1655: the runtime types a type reference names unqualified when rendered
/// by [`qualified_ts_type_ref`] (every user type is `handlers.`-qualified; these
/// are the runtime's own). `Stream`/`Query`/`History` render without a runtime
/// name, and `Effect` is peeled.
fn collect_runtime_type_names(r: &TypeRef, out: &mut std::collections::BTreeSet<&'static str>) {
    match r {
        TypeRef::Result(t, e, _) => {
            out.insert("Result");
            collect_runtime_type_names(t, out);
            collect_runtime_type_names(e, out);
        }
        TypeRef::Option(t, _) => {
            out.insert("Option");
            collect_runtime_type_names(t, out);
        }
        TypeRef::HttpResult(t, _) => {
            out.insert("HttpResult");
            collect_runtime_type_names(t, out);
        }
        TypeRef::Connection(t, _) => {
            out.insert("Connection");
            collect_runtime_type_names(t, out);
        }
        TypeRef::QueueResult(_) => {
            out.insert("QueueResult");
        }
        TypeRef::ValidationError(_) => {
            out.insert("ValidationError");
        }
        // #1883, slice 1a: structural (`asset_ts_type`), no runtime import.
        TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_) => {}
        TypeRef::JsonError(_) => {
            out.insert("JsonError");
        }
        TypeRef::Effect(t, _)
        | TypeRef::List(t, _)
        | TypeRef::Set(t, _)
        | TypeRef::Query(t, _)
        | TypeRef::Stream(t, _) => collect_runtime_type_names(t, out),
        TypeRef::Map(k, v, _) => {
            collect_runtime_type_names(k, out);
            collect_runtime_type_names(v, out);
        }
        TypeRef::Fn(params, ret, _) => {
            for p in params {
                collect_runtime_type_names(p, out);
            }
            collect_runtime_type_names(ret, out);
        }
        TypeRef::App { args, .. } => {
            for a in args {
                collect_runtime_type_names(a, out);
            }
        }
        TypeRef::Base(..) | TypeRef::Named(_) | TypeRef::History(..) | TypeRef::Unit(_) => {}
    }
}

/// P7.2: [`crate::emitter::ts_type_ref_qualified_ts_type`] over `r`, scoped
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
/// [`crate::emitter::ts_type_ref_qualified_ts_type`] (Decision B) itself
/// builds, not its printed text. That function originally had a
/// `String`-returning sibling, `ts_type_ref_qualified`, kept for a
/// still-`String`-based caller elsewhere in the crate; Arc C slice 32
/// (#1399) converted that last caller and deleted it — this function's own
/// call site was never affected, since it always used the `TsType`-returning
/// form.
fn qualified_ts_type_ref(r: &TypeRef) -> TsType {
    let scope: HashSet<String> = named_types_in(r).into_iter().collect();
    crate::emitter::ts_type_ref_qualified_ts_type(r, &scope, "handlers")
}

#[cfg(test)]
mod tests {
    use crate::testkit::emit_workers;

    const PAYMENT: &str = "context demo.payment

exports transparent { Receipt }

type Receipt = { id: String }

service authorise {
  on call(id: String) -> Effect[Result[Receipt, Int]] {
    Ok(Receipt { id: id })
  }
}
";

    const CHECKOUT: &str = "context demo.checkout

consumes demo.payment

service pay {
  on call(id: String) -> Effect[Result[Receipt, Int]] {
    demo.payment.authorise(id)
  }
}
";

    fn compose(files: &[(&str, &str)], worker: &str) -> String {
        emit_workers(files)
            .text(&format!("workers/{worker}/compose.ts"))
            .to_string()
    }

    #[test]
    fn each_call_service_gets_a_wrapper_delegating_to_its_handler() {
        let ts = compose(&[("demo/payment.bynk", PAYMENT)], "demo-payment");
        assert!(ts.contains("export function compose(env: Env) {"), "{ts}");
        assert!(
            ts.contains(
                "async authorise(id: string) {\n      return handlers.authorise.call(id, deps);"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_context_consuming_nothing_has_an_empty_env_and_no_env_in_deps() {
        let ts = compose(&[("demo/payment.bynk", PAYMENT)], "demo-payment");
        assert!(ts.contains("export interface Env {\n}"), "{ts}");
        assert!(ts.contains("const deps = {  };"), "{ts}");
    }

    #[test]
    fn a_consumed_context_is_a_service_binding_and_env_reaches_the_handlers() {
        let ts = compose(
            &[
                ("demo/payment.bynk", PAYMENT),
                ("demo/checkout.bynk", CHECKOUT),
            ],
            "demo-checkout",
        );
        assert!(ts.contains("  DEMO_PAYMENT: __ServiceBinding;"), "{ts}");
        assert!(ts.contains("const deps = { env };"), "{ts}");
    }

    /// #1961: a context consumed only for its types is not called, so it gets
    /// no Service Binding and its handlers' `env` does not name one.
    #[test]
    fn a_consumed_context_that_is_never_called_is_not_bound() {
        let ledger = "context demo.ledger

consumes demo.payment

service keep {
  on call(r: Receipt) -> Effect[Int] {
    Effect.pure(1)
  }
}
";
        let out = emit_workers(&[("demo/payment.bynk", PAYMENT), ("demo/ledger.bynk", ledger)]);
        let ts = out.text("workers/demo-ledger/compose.ts");
        assert!(!ts.contains("DEMO_PAYMENT"), "{ts}");
        assert!(ts.contains("export interface Env {\n}"), "{ts}");
        let toml = out.text("workers/demo-ledger/wrangler.toml");
        assert!(!toml.contains("[[services]]"), "{toml}");
    }

    /// #1961: each handler's `deps.env` names only the context it calls, while
    /// the Worker's `Env` binds both, so `compose`'s `env` satisfies each.
    #[test]
    fn each_handler_env_names_only_the_contexts_it_calls() {
        let ledger = "context demo.ledger

service note {
  on call(id: String) -> Effect[Int] {
    Effect.pure(1)
  }
}
";
        let hub = "context demo.hub

consumes demo.payment
consumes demo.ledger

service pay {
  on call(id: String) -> Effect[Result[Receipt, Int]] {
    demo.payment.authorise(id)
  }
}

service log {
  on call(id: String) -> Effect[Int] {
    demo.ledger.note(id)
  }
}
";
        let out = emit_workers(&[
            ("demo/payment.bynk", PAYMENT),
            ("demo/ledger.bynk", ledger),
            ("demo/hub.bynk", hub),
        ]);
        let ts = out.text("workers/demo-hub/compose.ts");
        assert!(ts.contains("  DEMO_LEDGER: __ServiceBinding;"), "{ts}");
        assert!(ts.contains("  DEMO_PAYMENT: __ServiceBinding;"), "{ts}");
        let handlers = out.text("workers/demo-hub/handlers.ts");
        let env_of = |svc: &str| {
            let at = handlers
                .find(&format!("export const {svc} = {{"))
                .unwrap_or_else(|| panic!("no `{svc}`:\n{handlers}"));
            let rest = &handlers[at..];
            let start = rest.find("env: {").expect("an env type");
            let end = rest[start..].find('}').expect("closed");
            rest[start..start + end].to_string()
        };
        assert!(env_of("pay").contains("DEMO_PAYMENT") && !env_of("pay").contains("DEMO_LEDGER"));
        assert!(env_of("log").contains("DEMO_LEDGER") && !env_of("log").contains("DEMO_PAYMENT"));
    }

    /// #1944: `Env` declares exactly the bindings `wrangler.toml` does, in
    /// the same order: services, the KV namespace, then the Durable Objects
    /// by class name, the events fan-out (`__EventsFanout`) after an agent.
    #[test]
    fn env_declares_the_wrangler_bindings_in_order() {
        let hub = "context ops.hub

consumes bynk { Events }
consumes bynk.cloudflare { Kv }
consumes demo.payment

event Pinged = { id: String }

agent Ledger {
  key id: String

  store seen: Cell[Int]

  on call total() -> Effect[Int] {
    Effect.pure(seen)
  }
}

service ping {
  on call(id: String) -> Effect[()] given Events, Kv {
    let _ <- Kv.put(id, id)
    let _ <- demo.payment.authorise(id)
    Events.emit[Pinged](Pinged { id: id })
  }
}
";
        let out = emit_workers(&[("demo/payment.bynk", PAYMENT), ("ops/hub.bynk", hub)]);
        let ts = out.text("workers/ops-hub/compose.ts");
        let env = ts
            .split_once("export interface Env {\n")
            .and_then(|(_, rest)| rest.split_once("}"))
            .map(|(members, _)| members)
            .unwrap_or_else(|| panic!("no Env interface:\n{ts}"));
        assert_eq!(
            env,
            "  DEMO_PAYMENT: __ServiceBinding;
  KV: __KVNamespace;
  LEDGER: __DurableObjectNamespace;
  ___EVENTS_FANOUT: __DurableObjectNamespace;
",
            "{ts}"
        );
        let toml = out.text("workers/ops-hub/wrangler.toml");
        let declared: Vec<&str> = toml
            .lines()
            .filter_map(|l| {
                l.strip_prefix("binding = \"")
                    .or_else(|| l.strip_prefix("name = \""))
            })
            .filter_map(|rest| rest.strip_suffix('"'))
            .filter(|name| *name != "ops-hub")
            .collect();
        let members: Vec<&str> = env
            .lines()
            .filter_map(|l| l.trim().split_once(':').map(|(name, _)| name))
            .collect();
        assert_eq!(members, declared, "{toml}");
    }

    /// #1950: a handler's `deps.env` type names a subset of the Worker's
    /// `Env` members, by the same names and types: the consumed context and
    /// the agent it constructs, never the KV namespace or the events fan-out.
    #[test]
    fn a_handlers_deps_env_is_a_projection_of_env() {
        let hub = "context ops.hub

consumes bynk { Events }
consumes bynk.cloudflare { Kv }
consumes demo.payment

event Pinged = { id: String }

agent Ledger {
  key id: String

  store seen: Cell[Int]

  on call total() -> Effect[Int] {
    Effect.pure(seen)
  }
}

service ping {
  on call(id: String) -> Effect[Int] given Events, Kv {
    let _ <- Kv.put(id, id)
    let _ <- demo.payment.authorise(id)
    let n <- Ledger(id).total()
    let _ <- Events.emit[Pinged](Pinged { id: id })
    Effect.pure(n)
  }
}
";
        let out = emit_workers(&[("demo/payment.bynk", PAYMENT), ("ops/hub.bynk", hub)]);
        let compose = out.text("workers/ops-hub/compose.ts");
        let env: Vec<&str> = compose
            .split_once("export interface Env {\n")
            .and_then(|(_, rest)| rest.split_once("}"))
            .map(|(members, _)| members.lines().map(|l| l.trim().trim_end_matches(';')))
            .unwrap_or_else(|| panic!("no Env interface:\n{compose}"))
            .collect();
        let handlers = out.text("workers/ops-hub/handlers.ts");
        let deps_env: Vec<&str> = handlers
            .split_once("env: { ")
            .and_then(|(_, rest)| rest.split_once(" }"))
            .map(|(members, _)| members.split("; "))
            .unwrap_or_else(|| panic!("no deps.env type:\n{handlers}"))
            .collect();
        assert_eq!(
            deps_env,
            [
                "DEMO_PAYMENT: __ServiceBinding",
                "LEDGER: __DurableObjectNamespace"
            ],
            "{handlers}"
        );
        for member in &deps_env {
            assert!(env.contains(member), "`{member}` is not in Env: {env:?}");
        }
    }

    #[test]
    fn a_user_type_in_a_wrapper_signature_is_qualified_through_handlers() {
        let src = "context demo.shop

type Order = { id: String }

service place {
  on call(order: Order) -> Effect[Option[Order]] {
    Effect.pure(Some(order))
  }
}
";
        let ts = compose(&[("demo/shop.bynk", src)], "demo-shop");
        assert!(ts.contains("async place(order: handlers.Order) {"), "{ts}");
    }

    #[test]
    fn each_http_route_gets_a_wrapper_named_by_method_and_path() {
        let src = "context demo.shop

service api from http {
  on GET(\"/items/:id\") (id: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(id)
  }
}
";
        let ts = compose(&[("demo/shop.bynk", src)], "demo-shop");
        assert!(
            ts.contains("async http_GET_items_Param_id(id: string) {\n      return handlers.api.http_GET_items_Param_id(id, deps);"),
            "{ts}"
        );
    }

    #[test]
    fn a_provided_capability_is_constructed_once_and_passed_in_deps() {
        let src = "context demo.shop

capability Clock {
  fn now() -> Effect[Int]
}

provides Clock = FixedClock {
  fn now() -> Effect[Int] {
    Effect.pure(0)
  }
}

service probe {
  on call() -> Effect[Int] given Clock {
    Clock.now()
  }
}
";
        let ts = compose(&[("demo/shop.bynk", src)], "demo-shop");
        assert!(
            ts.contains("const Clock = new handlers.FixedClock();"),
            "{ts}"
        );
        assert_eq!(ts.matches("new handlers.FixedClock()").count(), 1, "{ts}");
        assert!(ts.contains("const deps = { Clock };"), "{ts}");
    }
}
