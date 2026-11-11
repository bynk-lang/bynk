//! The integration and remote harness, runnable-test records and the attack/property predicates.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
use bynk_emit::emitter::wrangler::{BindingSource, WorkerBinding};
// #1759: every namespace a test module imports, from one definition.
use bynk_check::checker::Types;
use bynk_check::d1_migrations::{D1Migrations, d1_database_name};
use bynk_check::test_suites::{self};
use bynk_check::wire_format;
use bynk_emit::emitter::emit::test_scaffold_ns as test_ns;
use bynk_syntax::ast::{
    BaseType, Block, Commons, CommonsForm, Expr, ExprKind, FnDecl, FnName, Ident, PropertyDecl,
    QualifiedName, Trivia, TypeBody, TypeDecl, TypeRef,
};
use bynk_syntax::span::Span;
use bynk_ts::{TsArrowBody, TsBindingName, TsDecl, TsExpr, TsStmt, TsType};
use std::sync::Arc;

/// Emit the `__makeHarness()` factory: an in-process env per participant whose
/// Service Bindings call the sibling participants' real Worker `fetch` and whose
/// Durable-Object namespaces back the participant's own agents in memory, plus a
/// root env binding every participant (the test cases call in through it). A
/// fresh harness per case gives each case clean agent state.
/// #1479: returns the real [`TsStmt`] itself (was pre-printed `String`) —
/// the one real declaration this function ever built; its caller now prints
/// it directly, the same shape `emit_test_deps`'s own identical conversion
/// used.
///
/// #1959: each participant's `env` holds the bindings its own Worker declares
/// (`bindings_of`, the list its `wrangler.toml` is written from), by the same
/// names: a Service Binding forwards to that participant's Worker, the KV
/// namespace is a fresh in-memory namespace of the participant's own (each
/// deployed Worker gets its own: `bynk deploy` provisions one per Worker), and
/// each agent and the events fan-out is an in-memory Durable Object namespace
/// constructed with the participant's `env`.
///
/// #2136: `__buildHarness` builds it, and `__makeHarness` memoises it per case
/// in `__bynkHarness` (the case resets it before its first call), so every
/// driver a case calls reaches the same
/// participants and agent state. `exposed` names the participants whose own
/// `env` the harness also returns, for a driver that calls that Worker's
/// `scheduled` or `queue` entry directly.
pub(super) fn emit_integration_harness<'a>(
    participants: &[String],
    bindings_of: &dyn Fn(&String) -> &'a Vec<WorkerBinding>,
    sends: &dyn Fn(&String) -> bool,
    foreign_secrets: &dyn Fn(&String) -> Vec<String>,
    exposed: &[String],
    // #2134: put the running case's `env` entries (`__bynkEnv`) on each
    // participant's `env`, first, so the harness's own keys and bindings
    // below are never replaced by one.
    with_env: bool,
    // P4b (#2608): each `D1` database's migrations, run on each participant's
    // fresh database.
    d1_migrations: &D1Migrations,
) -> Vec<TsStmt> {
    let mut body = Vec::new();
    // Declare every participant env first so sibling references resolve.
    //
    // P7.2: deferred, not narrowed. `env_{ns}` is later passed positionally into
    // `worker_{dns}.fetch(req, env_{dns})`, whose own generated `Env` interface
    // (`emitter/workers.rs`'s `export interface Env { ... }`) types each binding
    // precisely (`ServiceBinding`, `DurableObjectNamespace`, ...) — `Record<string,
    // unknown>` would not structurally satisfy that on assignment, and correctly
    // referencing each participant's own generated `Env` type from this shared
    // harness scope needs its own naming/qualification scheme this slice hasn't
    // worked out. Left as `any`, named here rather than guessed at.
    for p in participants {
        let ns = test_ns(p);
        body.push(TsStmt::const_stmt(
            TsBindingName::Ident(format!("env_{ns}")),
            Some(TsType::named("any")),
            TsExpr::object(vec![]),
            None,
        ));
    }
    for p in participants {
        let ns = test_ns(p);
        let env = format!("env_{ns}");
        if with_env {
            body.push(expr_stmt(call(
                ident("globalThis.Object.assign"),
                vec![ident(env.clone()), ident("__bynkEnv")],
            )));
        }
        // #2137: the harness clock the participant's test-build `Clock` reads.
        body.push(TsStmt::assign(
            member(ident(env.clone()), "__bynkClock"),
            ident("__bynkClock"),
            None,
        ));
        // I2 (#1971): every participant holds the local key for each edge it
        // is on, as under `bynk dev`: from the test cases' caller, from each
        // participant that binds to it (a caller or an event publisher), and
        // to each participant it binds to.
        let callers = std::iter::once(HARNESS_CALLER).chain(
            participants
                .iter()
                .filter(|q| {
                    bindings_of(q).iter().any(
                        |b| matches!(&b.source, BindingSource::Service { context } if context == p),
                    )
                })
                .map(String::as_str),
        );
        let callees = bindings_of(p).iter().filter_map(|b| match &b.source {
            BindingSource::Service { context } if participants.contains(context) => {
                Some(context.as_str())
            }
            _ => None,
        });
        for key in callers
            .filter_map(wire_format::edge_key_from)
            .chain(callees.filter_map(wire_format::edge_key_to))
            .chain(foreign_secrets(p))
        {
            body.push(TsStmt::assign(
                member(ident(env.clone()), &key),
                str_lit(bynk_emit::LOCAL_CALL_KEY),
                None,
            ));
        }
        for binding in bindings_of(p) {
            let value = match &binding.source {
                // #1856: a context that is no participant has no Worker here.
                BindingSource::Service { context } => {
                    if !participants.contains(context) {
                        continue;
                    }
                    let dns = test_ns(context);
                    service_binding_forward(
                        &format!("worker_{dns}"),
                        &format!("env_{dns}"),
                        sends(context),
                    )
                }
                // S4a (#1966): each instance its own fresh namespace.
                BindingSource::Kv | BindingSource::KvInstance { .. } => {
                    call(ident("__makeIntegrationKv"), vec![])
                }
                // P5 (#2323): each instance its own fresh bucket.
                BindingSource::R2Instance { .. } => call(ident("__makeIntegrationR2"), vec![]),
                // P3 (#2342): each instance its own fresh limiter, counting on
                // the harness clock this module always carries, so a case can
                // recover one with `advance Clock by`.
                BindingSource::RateLimitInstance { limit, period, .. } => call(
                    ident("__makeIntegrationRateLimit"),
                    vec![
                        num_lit(limit.to_string()),
                        num_lit(period.to_string()),
                        TsExpr::Arrow {
                            params: Vec::new(),
                            is_async: false,
                            generics: Vec::new(),
                            return_type: None,
                            body: Box::new(TsArrowBody::Expr(Box::new(ident("__bynkClockNow")))),
                        },
                    ],
                ),
                // P2 (#2578): each instance its own recorder; nothing is
                // delivered to a consumer participant.
                BindingSource::QueueProducer { .. } => {
                    call(ident("__makeIntegrationQueue"), vec![])
                }
                // P3b (#2582): each instance its own fresh recorder.
                BindingSource::AnalyticsDataset { .. } => {
                    call(ident("__makeIntegrationAnalytics"), vec![])
                }
                // P4a (#2602): each instance its own in-memory database, and
                // P4b (#2608) runs its migrations on it first.
                BindingSource::D1Database { .. } => call(
                    ident("__makeIntegrationD1"),
                    d1_migrations_args(d1_migrations.get(&d1_database_name(
                        &bynk_project::worker_dir_name(p),
                        &binding.name,
                    ))),
                ),
                // #3300: a fixed version, the same for every participant.
                BindingSource::Singleton { .. } => {
                    call(ident("__makeIntegrationVersionMetadata"), vec![])
                }
                // Interop I5 (#1972): the foreign Worker is not here, so its
                // binding answers as `bynk dev`'s default double does.
                BindingSource::ForeignService { worker, .. } => {
                    foreign_service_double(&binding.name, worker)
                }
                BindingSource::Agent { class_name } => call(
                    ident("__makeIntegrationDoNamespace"),
                    vec![member(ident(ns.clone()), class_name), ident(env.clone())],
                ),
                BindingSource::EventsFanout { class_name } => call(
                    ident("__makeIntegrationDoNamespace"),
                    vec![
                        member(ident(fanout_alias(&ns)), class_name),
                        ident(env.clone()),
                    ],
                ),
            };
            body.push(TsStmt::assign(
                member(ident(env.clone()), &binding.name),
                value,
                None,
            ));
        }
    }
    // Root env binds to every participant.
    // P7.2: deferred, same reason as `env_{ns}` above.
    body.push(TsStmt::const_stmt(
        TsBindingName::Ident("rootEnv".to_string()),
        Some(TsType::named("any")),
        TsExpr::object(vec![]),
        None,
    ));
    // I2 (#1971): the test cases call in through `rootEnv`, as
    // [`HARNESS_CALLER`], so it holds the key for the edge to each participant.
    for key in participants
        .iter()
        .filter_map(|p| wire_format::edge_key_to(p))
    {
        body.push(TsStmt::assign(
            member(ident("rootEnv"), &key),
            str_lit(bynk_emit::LOCAL_CALL_KEY),
            None,
        ));
    }
    for p in participants {
        let ns = test_ns(p);
        let binding = bynk_emit::emitter::wrangler::service_binding(p).name;
        body.push(TsStmt::assign(
            member(ident("rootEnv"), binding),
            service_binding_forward(&format!("worker_{ns}"), &format!("env_{ns}"), sends(p)),
            None,
        ));
    }
    let mut fields = vec![("env".to_string(), ident("rootEnv"))];
    for p in exposed {
        let env = format!("env_{}", test_ns(p));
        fields.push((env.clone(), ident(env)));
    }
    body.push(TsStmt::return_stmt(Some(TsExpr::object(fields)), None));
    let build = TsStmt::decl(
        TsDecl::Function {
            name: "__buildHarness".to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            body,
            is_async: false,
            inline: false,
        },
        None,
    );
    let mut memo = TsStmt::raw(
        "let __bynkHarness: ReturnType<typeof __buildHarness> | undefined;\nfunction __makeHarness() {\n  return (__bynkHarness ??= __buildHarness());\n}\n".to_string(),
        None,
    );
    memo.no_blank_before = true;
    vec![build, memo]
}

/// #2138: the harness of a remote run — each participant's binding reaches
/// its URL in the test target (`BYNK_TEST_TARGETS`, keyed by worker name)
/// through `__bynkRemoteBinding`, so every driver's request goes to the real
/// runtime. Memoised per case like the in-process harness, so a case body
/// lowers identically against either.
pub(super) fn emit_remote_harness(participants: &[String]) -> Vec<TsStmt> {
    let mut text = String::from(
        "function __buildHarness() {\n  const rootEnv: { [binding: string]: __ServiceBinding } = {};\n",
    );
    for p in participants {
        let binding = bynk_emit::emitter::wrangler::service_binding(p).name;
        let worker = worker_dir_name(p);
        text.push_str(&format!(
            "  rootEnv.{binding} = __bynkRemoteBinding({worker:?});\n"
        ));
    }
    text.push_str("  return { env: rootEnv };\n}\n");
    text.push_str("let __bynkHarness: ReturnType<typeof __buildHarness> | undefined;\nfunction __makeHarness() {\n  return (__bynkHarness ??= __buildHarness());\n}\n");
    vec![
        TsStmt::raw(include_str!("../test_runtime/remote.ts").to_string(), None),
        // #2138 (track §3.6): the `--settle` retry the case bodies use.
        TsStmt::raw(include_str!("../test_runtime/settle.ts").to_string(), None),
        TsStmt::raw(text, None),
    ]
}

/// Build the [`checker::TypedCommons`] used to lower integration case bodies —
/// the harness view ([`test_suites::integration_view`]: `uses` commons plus
/// participant types/fns/methods, closed over the types they reach, #1814),
/// so static calls and constructors resolve.
pub(super) fn integration_typed_commons(
    view: &test_suites::SuiteView,
    tys: &Arc<checker::Types>,
) -> checker::TypedCommons {
    let test_suites::SuiteView {
        types,
        fns,
        methods,
        ..
    } = view.clone();
    checker::TypedCommons {
        commons: Arc::new(Commons {
            name: QualifiedName {
                parts: vec![Ident {
                    name: HARNESS_CALLER.to_string(),
                    span: Span::default(),
                }],
                span: Span::default(),
            },
            items: Vec::new(),
            uses: Vec::new(),
            documentation: None,
            form: CommonsForm::Brace,
            span: Span::default(),
            trivia: Trivia::default(),
            trailing_comments: Vec::new(),
        }),
        types,
        fns,
        methods,
        expr_types: HashMap::new(),
        callees: HashMap::new(),
        warnings: vec![],
        // T3.6b (R4.1): the *caller's* table, not a fresh one. A synthesised
        // commons starts with an empty `expr_types`, but the case/property
        // checks below fill it in — interning into the table they were handed
        // — and the lowering then resolves those ids back through this field.
        // A table of its own would make every one of those reads a
        // wrong-table lookup.
        ty_intern: Arc::clone(tys),
        actor_bindings: HashMap::new(),
        assets: HashMap::new(),
        embeds: HashMap::new(),
        consts: HashMap::new(),
        shadowed_imports: HashMap::new(),
        extensible_enums: Default::default(),
        stability: Default::default(),
        exec_plan: Default::default(),
    }
}

pub(super) fn sanitise_suite(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push('_');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "suite".to_string()
    } else {
        trimmed
    }
}

/// Discovered, named test ready to be invoked from the top-level runner.
pub(crate) struct RunnableTest {
    /// Joined target name (e.g., `commerce.payment`), or `system · <suite>`
    /// for a wired (`system`) suite — the runner's module identity and sort key.
    pub(super) target_name: String,
    /// The module's output path relative to the project root.
    pub(super) module_path: PathBuf,
    /// v0.67: `"unit"` or `"integration"` — the suite kind for discovery, mirrors
    /// the runner's `suite-begin` `kind`.
    pub(super) kind: &'static str,
    /// v0.67: the JSON suite name for discovery — the joined target name (unit)
    /// or the bare suite name (integration). Differs from `target_name` only for
    /// integration, which the runner prefixes with `system · ` (#2185).
    pub(super) suite_name: String,
    /// #2138: the cases a remote build skipped (`--skip-unsupported`), and why.
    pub(super) skipped: Vec<crate::project::RemoteSkip>,
    /// #2138: under `--against dev`, the distinct `env` maps of its cases.
    pub(super) envs: Vec<BTreeMap<String, String>>,
    /// #2138: a remote build's module, whose cases report `retries`.
    pub(super) remote: bool,
    /// The module inlines the generative-property runtime (it declares a
    /// `property` or attacks a contract), so its failed cases report `kind`,
    /// `seed` and `counterexample`.
    pub(super) generative: bool,
    /// v0.67: the suite's cases, in declaration (emission) order, retained for
    /// `--no-run` discovery.
    pub(super) cases: Vec<DiscoveredCase>,
    /// #2842: the snapshot files (`snapshots/…snap.json`) the module's suites take
    /// a snapshot into. The module exports `__bynkSnapshotReport` iff non-empty.
    pub(crate) snapshot_files: Vec<String>,
}

/// v0.67: the `path:line:col` of a test-name literal, structured for discovery.
/// Reuses [`bynk_syntax::span::line_col`] and the same forward-slash
/// normalisation `assert_location` applies (bynk-emit/src/emitter/lower.rs), so a
/// discovered case and a run-failure resolve to consistent coordinates.
pub(super) fn discovered_location(source: &str, rel_path: &str, span: Span) -> TestLocation {
    let (line, col) = bynk_syntax::span::line_col(source, span.start);
    TestLocation {
        path: rel_path.replace('\\', "/"),
        line: line as u32,
        col: col as u32,
    }
}

/// v0.67: fold the combined runnable manifest into the discovery suites, ordered
/// by the runner's sort key (`target_name`) so the discovery document matches a
/// run's suite order.
pub(crate) fn discovery_manifest(tests: &[RunnableTest]) -> Vec<DiscoveredSuite> {
    let mut sorted: Vec<&RunnableTest> = tests.iter().collect();
    sorted.sort_by(|a, b| a.target_name.cmp(&b.target_name));
    sorted
        .into_iter()
        .map(|t| DiscoveredSuite {
            name: t.suite_name.clone(),
            kind: t.kind,
            cases: t.cases.clone(),
            skipped: t.skipped.clone(),
            envs: t.envs.clone(),
        })
        .collect()
}

/// Whether a `case` body uses an observation (`Cap.op called …`) or
/// `trace(Cap.op)` anywhere (v0.117) — the signal to wrap `deps` with the
/// recording proxy. Bodies that don't observe emit unchanged.
pub(super) fn block_uses_observation(block: &Block) -> bool {
    let mut found = false;
    let mut check = |e: &Expr| {
        if matches!(e.kind, ExprKind::Observation(_) | ExprKind::Trace { .. }) {
            found = true;
        }
    };
    // #1851: every expression of a statement, a principal's identity
    // included, not a hand-picked value.
    let mut exprs = Vec::new();
    for s in &block.statements {
        bynk_syntax::ast::statement_exprs(s, &mut exprs);
    }
    for e in exprs {
        bynk_model::walk_exprs(e, &mut check);
    }
    bynk_model::walk_exprs(&block.tail, &mut check);
    found
}

/// v0.178 (#662): build the target's service table for a test-body check —
/// each service's protocol word and its `on call` handler signature, so a
/// `svc.call(args)` in a case can be resolved rather than string-matched. A
/// service with no `on call` handler carries `None`, which turns `svc.call(...)`
/// into `bynk.test.service_no_call_handler` instead of a silent runtime crash.
pub(super) fn target_service_handler_kinds(
    table: Option<&UnitTable>,
) -> HashMap<String, Vec<bynk_model::IrHandlerKind>> {
    let Some(t) = table else {
        return HashMap::new();
    };
    t.services
        .iter()
        .map(|(name, decl)| {
            (
                name.clone(),
                decl.handlers
                    .iter()
                    .map(|h| bynk_model::lower_handler_kind_ir(&h.kind))
                    .collect(),
            )
        })
        .collect()
}

/// I3 (#1971), #2020: the target's handlers that bind an actor sum, whose
/// `deps.who` a call-site principal builds: an `on call` sum (`by who: Caller
/// | Legacy`), keyed `(service, "call")`, and an http sum (`by who: User |
/// Visitor`), keyed `(service, <emitted method name>)`. Mirrors the order
/// `bynk_model::lower_actor_seam_ir` resolves a handler's seam in.
pub(super) fn target_sum_handlers(table: Option<&UnitTable>) -> HashSet<(String, String)> {
    let Some(t) = table else {
        return HashSet::new();
    };
    let mut out = HashSet::new();
    for (name, decl) in &t.services {
        for h in &decl.handlers {
            let key = match bynk_model::lower_handler_kind_ir(&h.kind) {
                bynk_model::IrHandlerKind::Call
                    if bynk_check::actors::call_admission_for(h, &t.actors)
                        .is_some_and(|a| a.sum.is_some()) =>
                {
                    "call".to_string()
                }
                bynk_model::IrHandlerKind::Http { method, path }
                    if bynk_check::actors::sum_members_for(h, &t.actors).is_some() =>
                {
                    bynk_emit::emitter::http_handler_method_name_ir(method, &path)
                }
                _ => continue,
            };
            out.insert((name.clone(), key));
        }
        // #2856: a Bearer/Oidc route whose `by` has no binder mints no
        // identity (`deps` carries no `identity` field), so an in-process call
        // must not pass one; recorded under a `nobinder:` key.
        for h in &decl.handlers {
            if let bynk_model::IrHandlerKind::Http { method, path } =
                bynk_model::lower_handler_kind_ir(&h.kind)
                && (bynk_check::actors::bearer_seam_for(h, &t.actors)
                    .is_some_and(|s| s.binder.is_none())
                    || bynk_check::actors::oidc_seam_for(h, &t.actors)
                        .is_some_and(|s| s.binder.is_none()))
            {
                out.insert((
                    name.clone(),
                    format!(
                        "nobinder:{}",
                        bynk_emit::emitter::http_handler_method_name_ir(method, &path)
                    ),
                ));
            }
        }
        // #2380 (L5): an `Oidc` route that binds its actor, recorded under an
        // `oidc:` key, so an in-process call gives it `deps.claims`.
        for h in &decl.handlers {
            if let bynk_model::IrHandlerKind::Http { method, path } =
                bynk_model::lower_handler_kind_ir(&h.kind)
                && bynk_check::actors::oidc_seam_for(h, &t.actors)
                    .is_some_and(|s| s.binder.is_some())
            {
                out.insert((
                    name.clone(),
                    format!(
                        "oidc:{}",
                        bynk_emit::emitter::http_handler_method_name_ir(method, &path)
                    ),
                ));
            }
        }
    }
    out
}

/// v0.119: `Some((run_var, agent_name))` when `prop` is a history property — its
/// single `for all` binding is `run: History[Agent]`.
pub(super) fn prop_history_binding(prop: &PropertyDecl) -> Option<(&str, &str)> {
    prop.forall.bindings.iter().find_map(|b| match &b.type_ref {
        TypeRef::History(inner, _) => match &**inner {
            TypeRef::Named(id) => Some((b.name.name.as_str(), id.name.as_str())),
            _ => None,
        },
        _ => None,
    })
}

pub(super) fn prop_is_history(prop: &PropertyDecl) -> bool {
    prop_history_binding(prop).is_some()
}

/// Emit a single test module TypeScript file plus the [`RunnableTest`]
/// pointer used by the top-level runner.
#[allow(clippy::too_many_arguments)]
/// v0.115 (testing track slice 3): can this contracted free function be attacked
/// by the runner — *a contract is a property that is always on.* The runner
/// generates arguments over the parameters' refinement domains, filters by the
/// `requires` (inputs failing a precondition are discarded, exactly as a
/// `for all … where` does), calls the function, and the dev/test call-site guard
/// asserts each `ensures`, throwing a shrinkable `BynkContractError` on
/// violation. Returns `false` when there is nothing to attack: no `ensures`, no
/// (nameable) parameters, an effectful return (its inputs include the world —
/// dev-guard only), a non-primitive parameter (its generated fields are `bigint`
/// and could mix with `number` arithmetic — dev-guard only), or a parameter the
/// generator cannot inhabit (an over-narrow / `Matches`-pinned refinement) —
/// never a false error, mirroring an over-narrow `where`.
pub(super) fn is_attackable_contract(
    f: &FnDecl,
    resolved: &ResolvedCommons,
    tys: &Arc<Types>,
) -> bool {
    if !matches!(&f.name, FnName::Free(_)) {
        return false;
    }
    if f.ensures.is_empty() || f.params.is_empty() {
        return false;
    }
    if matches!(&f.return_type, TypeRef::Effect(_, _)) {
        return false;
    }
    if f.params.iter().any(|p| p.name.name == "_") {
        return false;
    }
    for p in &f.params {
        let Some(ty) = checker::resolve_type_ref(&p.type_ref, &resolved.types, tys) else {
            return false;
        };
        // Restrict to primitive (or refined-over-primitive) parameters: a
        // generated composite carries `bigint` fields that would mix with the
        // function's `number` arithmetic. Composite-param contracts are covered
        // by the dev guard.
        if numeric_or_scalar_base(ty, &resolved.types, tys).is_none() {
            return false;
        }
        if !test_suites::prop_binding_generable(
            ty,
            &resolved.types,
            test_suites::PROP_GEN_DEPTH,
            tys,
        ) {
            return false;
        }
    }
    true
}

/// The primitive base type a parameter erases to — `Some(base)` for a base type
/// or a refinement/opaque over one, `None` for a composite (record/sum/list/map/
/// option/result/etc.). Used to gate a contract as attackable — every param
/// must erase to a primitive `binding_gen`/`destructure_vals` can actually
/// generate/coerce. (Previously also used to conditionally `Number(…)`-wrap a
/// call argument at the attacked function's own call site — v0.114's
/// generator/erasure split; #1426 made that redundant: `destructure_vals`
/// coerces every `Int`-drawing local at the bind site now, so every argument
/// reaching the call is already a real `number`.)
pub(super) fn numeric_or_scalar_base(
    ty: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
) -> Option<BaseType> {
    match &*tys.get(ty) {
        checker::Ty::Base(b) => Some(*b),
        checker::Ty::Named { name, .. } => match &types.get(name)?.body {
            TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } => Some(*base),
            _ => None,
        },
        _ => None,
    }
}

/// v0.115: the target unit's contracted free functions the runner can attack, in
/// deterministic (name) order, paired with the resolved view for gen/coercion.
pub(super) fn attackable_contracts(
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    tys: &Arc<Types>,
) -> Option<(ResolvedCommons, Vec<FnDecl>)> {
    let (resolved, _) = test_suites::build_privileged_resolved(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    )?;
    let table = unit_tables.get(target_name)?;
    let mut fns: Vec<FnDecl> = table
        .fns
        .values()
        .map(std::sync::Arc::as_ref)
        .filter(|f| is_attackable_contract(f, &resolved, tys))
        .cloned()
        .collect();
    fns.sort_by_key(|a| a.name.display());
    Some((resolved, fns))
}

/// #917: the type-only namespace qualifier every named type reachable from
/// `target_name` renders through inside the test module that targets it. A
/// test module imports the target, the contexts it consumes and its `uses`
/// commons only as namespaces (`import * as orders from …`), never inlining
/// their declarations the way a `uses`r module's own production emission does,
/// so *every* name here is foreign and needs one.
///
/// #2075: the precedence is `synthetic_typed_commons_for_target`'s, so a name
/// qualifies through the declaration the case was checked against: the
/// target's own type, then a consumed unit's (#2062: a consumed *context*'s
/// too, whose codecs a `Json.decode[Order]` in a case now generates), then a
/// `uses`d commons'. A consumed type that collides with a `uses`d one never
/// reaches here: the checker refuses that unit (`bynk.consumes.name_conflict`).
pub(super) fn json_codec_qual_for_target(
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
) -> HashMap<String, String> {
    let mut qual = HashMap::new();
    // #2702: only the target's own types are all visible (a unit-tier suite sees
    // its target's private declarations, ADR 0683 decision I); another unit's
    // `private` type is not in scope, so it must not claim a name.
    let mut qualify = |units: Option<&Vec<String>>, own: bool| {
        for u in units.into_iter().flatten() {
            let ns = test_ns(u);
            if let Some(table) = unit_tables.get(u) {
                for n in table.types.keys() {
                    if own || is_public_type(table, n) {
                        qual.entry(n.clone()).or_insert_with(|| format!("{ns}."));
                    }
                }
            }
        }
    };
    // Highest precedence first; `or_insert` keeps the first claim.
    qualify(Some(&vec![target_name.to_string()]), true);
    qualify(unit_consumes.get(target_name), false);
    qualify(unit_uses.get(target_name), false);
    qual
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- sanitise_suite / sanitise_case_name ----------------------------------
    #[test]
    fn sanitise_suite_lowercases_collapses_and_trims() {
        assert_eq!(sanitise_suite("My Suite"), "my_suite");
        assert_eq!(sanitise_suite("Foo__Bar"), "foo_bar");
        assert_eq!(sanitise_suite("  Hello  "), "hello");
        assert_eq!(sanitise_suite("a1B2"), "a1b2");
        assert_eq!(sanitise_suite("!!!"), "suite"); // empty after trim -> fallback
        assert_eq!(sanitise_suite(""), "suite");
    }

    // -- append_missing_bindings ----------------------------------------------
    // #1486: ported from `emitter.rs`'s own retired `inject_runtime_imports_
    // tests` — that module tested the pre-#1486 post-print text-splice
    // mechanism directly; `append_missing_bindings` is its tree-level
    // successor (this file's own copy of the dedup rule `emitter/workers.rs`'s
    // identically-named function also carries), so the same cases apply,
    // ported to operate on a `Vec<String>` of already-real import names
    // instead of an `import { ... } from "...";` text line.
}
