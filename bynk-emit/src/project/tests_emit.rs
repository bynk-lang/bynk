//! Test/integration-suite processing: `suite`/`test integration` checking +
//! TypeScript emission.
//!
//! P5.4 (`design/tracks/semantics-in-the-checker.md` §6): the checking half
//! of [`process_tests`]/[`process_integration_tests`] — target/participant
//! resolution, duplicate-case-name detection, `stub`-clause resolution, and
//! case/property body type-checking — relocated to
//! `bynk_check::test_suites` (as [`test_suites::phase_test_bodies`]/
//! [`test_suites::phase_integration_bodies`]), shared with
//! `bynk_check::analysis::analyse_project` so a `.bynk` test file gets
//! diagnostics and `RefSink` bindings (go-to-definition/find-references)
//! through the LSP again, closing category 7 of
//! `bynk-check/src/analysis.rs`'s own residual-gap accounting — the last of
//! the seven. This file keeps both functions' exact signatures (`run_checks`
//! needs no change) and everything Phase 5 (TypeScript emission) needs: the
//! two functions now call the relocated checking phase first, then emit only
//! for what it returns as ready. Several functions the checking phase needs
//! are genuinely dual-use — also needed here to lower a checked body to
//! TypeScript — and stayed `pub` in `bynk_check::test_suites` rather than
//! being duplicated; every call into that module below is qualified
//! (`test_suites::foo(...)`) for exactly that reason (see that module's own
//! doc comment for the full accounting).

use super::*;
use crate::emitter::RuntimeUse;
use crate::emitter::wrangler::{BindingSource, WorkerBinding};
// #1759: every namespace a test module imports, from one definition.
use crate::emitter::emit::test_scaffold_ns as test_ns;
use bynk_check::checker::Types;
use bynk_check::test_suites::{self, ResolvedStub, StubScope};
use bynk_check::wire_format;
use bynk_syntax::ast::{
    ArgPattern, BaseType, BinOp, Block, CapabilityOp, Case, Commons, CommonsForm, CommonsItem,
    Expr, ExprKind, FnDecl, FnName, Ident, Param, PredKind, PropertyDecl, QualifiedName,
    Refinement, SeqOutcome, StubClause, StubRhs, Trivia, TypeBody, TypeDecl, TypeRef, Visibility,
};
use bynk_syntax::span::Span;
use bynk_ts::{
    SourceMapBuilder, TsArrowBody, TsBinaryOp, TsBindingName, TsClassMethod, TsDecl, TsExpr, TsLit,
    TsObjectEntry, TsParam, TsProgram, TsStmt, TsSwitchCase, TsType, TsTypeMember,
};
use std::sync::Arc;

// -- v0.7 / v0.118: test declaration processing --

/// P5.4: `test <target>` suite processing. The checking half (target/`stub`
/// resolution, duplicate-case-name detection, case/property body
/// type-checking — formerly this function's own Phases 2-4) now lives in
/// [`test_suites::phase_test_bodies`]; only a target that passed every gate
/// comes back, keyed by its resolved `stub` map, so the loop below is Phase
/// 5 (TypeScript emission) alone, over exactly the targets ready for it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_tests(
    test_groups: &BTreeMap<String, Vec<usize>>,
    parsed: &[ParsedFile],
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    exports_visibility: &HashMap<String, HashMap<String, Visibility>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_uses: &HashMap<String, Vec<String>>,
    // v0.17 (Locale capability track, slice 1, #844): capability name -> owning
    // unit, per target, for capabilities flattened in via `consumes U { Cap }`
    // (this never includes anything a target declares itself). Needed so
    // `__makeTestDeps()` wires an adapter-flattened capability (real *or*
    // stubbed) — it is not in `UnitTable.capabilities`, which holds only
    // locally-declared capabilities.
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    // v0.132: production unit name -> its `parsed` file indices, so a barrel can
    // resolve a multi-file commons the test module imports back to its files.
    groups: &BTreeMap<String, Vec<usize>>,
    import_ext: ImportExt,
    // v0.115: whether the build emits the contract guard (dev/test). The runner
    // attack relies on the guard to assert `ensures`, so it is emitted only when
    // the guard is (they are always paired — `bynkc test` sets both).
    contracts: bool,
    // v0.132: running set of already-emitted barrel output paths, shared with the
    // integration pass so a commons imported by both is barrelled exactly once.
    emitted_barrels: &mut HashSet<PathBuf>,
    // #1883, slice 3: the declared assets, for `svc.GET(asset)` in a case.
    asset_files: &TestAssetFiles,
    // #1975: each user adapter's binding module, as an extensionless path
    // from the output root, for a test that builds the binding for real.
    binding_modules: &HashMap<String, String>,
    // #2052: each event's effective schema version, keyed `<unit>.<Event>`
    // (`emit_project`'s `schema_effective_versions`), for the default envelope
    // of a `svc.event(e)` address.
    schema_versions: &HashMap<String, i64>,
    // #2050: each unit's resolved callees, for [`target_uses_emit`].
    unit_callees: &HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    tys: &Arc<Types>,
) -> (Vec<StagedFile>, Vec<RunnableTest>) {
    let mut outputs: Vec<StagedFile> = Vec::new();
    let mut runnable_tests: Vec<RunnableTest> = Vec::new();

    let ready = test_suites::phase_test_bodies(
        test_groups,
        parsed,
        kinds,
        unit_tables,
        exports_visibility,
        unit_consumes,
        unit_consumes_aliases,
        unit_uses,
        errors,
        refs,
        &mut bynk_check::locals::LocalsSink::new(),
        tys,
    );

    let mut sorted_targets: Vec<&String> = ready.keys().collect();
    sorted_targets.sort();

    for target_name in sorted_targets {
        let target_stubs = &ready[target_name];
        // `ready` only contains targets `phase_test_bodies` resolved against
        // `kinds` — this lookup cannot miss.
        let target_kind = *kinds
            .get(target_name)
            .expect("phase_test_bodies only returns targets it resolved against `kinds`");
        let indices = test_groups.get(target_name).unwrap();
        // #1814: the types the target's privileged view reaches but does not
        // import, computed once per target for every runner below.
        let hidden =
            test_suites::privileged_view(target_name, unit_tables, unit_uses, unit_consumes)
                .map(|v| v.hidden)
                .unwrap_or_default();

        // -- Phase 5: emit TypeScript test module. --
        let emit_out = emit_test_module(
            target_name,
            target_kind,
            indices,
            parsed,
            target_stubs,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_uses,
            unit_flattened,
            exports_visibility,
            import_ext,
            contracts,
            &hidden,
            asset_harness(
                target_name,
                unit_uses,
                unit_consumes,
                unit_tables,
                asset_files,
            ),
            binding_modules,
            schema_versions,
            target_uses_emit(
                target_name,
                target_kind,
                unit_tables,
                unit_flattened,
                unit_callees,
            ),
            tys,
        );
        if let Some((path, program, source_map, runnable)) = emit_out {
            outputs.push(StagedFile {
                output_path: path,
                document: Document::Ts(program),
                source_map,
                debug_metadata: None,
            });
            runnable_tests.push(runnable);

            // v0.132: the module imports the target and each of its `consumes` /
            // `uses` targets as a namespace (`import * as ns from "./<name>.js"`).
            // Any of those that is a multi-file commons has no `out/<name>.ts`, so
            // emit an aggregating barrel (deduped) to make the import resolve.
            let mut imported: Vec<&str> = vec![target_name.as_str()];
            if let Some(consumed) = unit_consumes.get(target_name) {
                imported.extend(consumed.iter().map(String::as_str));
            }
            if let Some(used) = unit_uses.get(target_name) {
                imported.extend(used.iter().map(String::as_str));
            }
            // #1814: and the owner of each reached type it imports.
            imported.extend(hidden.values().map(String::as_str));
            for name in imported {
                if let Some(barrel) =
                    emit_commons_barrel(name, groups, parsed, import_ext, emitted_barrels, &[])
                {
                    outputs.push(barrel);
                }
            }
        }
    }

    // v0.16: the top-level `tests/main.ts` runner is emitted once by the caller
    // after both unit- and integration-test passes, so it can aggregate both.
    (outputs, runnable_tests)
}

/// P5.4: `test integration "name"` suite processing. Validates the `wires`
/// participant set (existence, ≥ 2, no duplicates, full `consumes` closure),
/// type-checks each case body as a cross-context call from a synthetic
/// harness root that consumes every participant, and emits a TypeScript
/// module that stands the participants up as in-process Workers wired by
/// simulated Service Bindings and runs the cases across the real
/// serialise/deserialise wire. The checking half (participant inference, the
/// `system`-needs-a-serialisation-edge gate, duplicate-case-name detection,
/// the harness cross-context view, and per-case body type-checking) now
/// lives in [`test_suites::phase_integration_bodies`], which hands back each
/// ready group's harness [`resolver::CrossContextInfo`] — the one piece
/// worth not recomputing here (built from clone-heavy maps).
/// `participants`/`uses_targets`/`case_inputs` are cheap and pure, so the
/// loop below recomputes them exactly as this function always did, using the
/// now-relocated [`test_suites::infer_participants`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_integration_tests(
    integration_groups: &BTreeMap<String, Vec<usize>>,
    parsed: &[ParsedFile],
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_uses: &HashMap<String, Vec<String>>,
    // v0.132: production unit name -> its `parsed` file indices (see `process_tests`).
    groups: &BTreeMap<String, Vec<usize>>,
    // v0.132: barrel-path dedup set shared with the unit-test pass.
    emitted_barrels: &mut HashSet<PathBuf>,
    // #1883, slice 3: the declared assets, for `svc.GET(asset)` in a case.
    asset_files: &TestAssetFiles,
    // #1959: each context's Worker bindings, bound on each participant's
    // `env`. `None` when earlier errors stop the build anyway.
    context_bindings: Option<&BTreeMap<String, Vec<WorkerBinding>>>,
    // `bynk_check::symbols::discover_event_subscribers`' table.
    event_subscribers: &BTreeMap<(String, String), Vec<(String, String)>>,
    // #2138: a remote test build (`CompileOptions::remote_tests`).
    remote: Option<&crate::project::RemoteTests>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    tys: &Arc<Types>,
) -> (Vec<StagedFile>, Vec<RunnableTest>) {
    let mut outputs: Vec<StagedFile> = Vec::new();
    let mut runnables: Vec<RunnableTest> = Vec::new();

    let ready = test_suites::phase_integration_bodies(
        integration_groups,
        parsed,
        kinds,
        unit_tables,
        unit_consumes,
        unit_consumes_aliases,
        unit_uses,
        errors,
        refs,
        tys,
    );

    let mut sorted: Vec<&String> = ready.keys().collect();
    sorted.sort();

    for group_name in sorted {
        let indices = integration_groups.get(group_name).unwrap();
        let first = indices[0];
        let Some(decl) = parsed[first].integration() else {
            continue;
        };
        // v0.118: there is no `suite` string any more — the wired suite is named
        // for its target context. The participant set is INFERRED from the
        // target's transitive `consumes` closure (no `wires` list).
        let suite_target = decl.target.joined();
        let suite_name = suite_target.clone();
        // #1856: contexts only; a capability-only unit is provided in-process.
        let participants = test_suites::infer_participants(&suite_target, unit_consumes, kinds);
        // #1959: the Workers the harness stands up: also every subscriber to
        // a participant's events, so the fan-out has somewhere to deliver.
        let harness_participants = test_suites::infer_harness_participants(
            &suite_target,
            unit_consumes,
            kinds,
            event_subscribers,
        );
        // `ready` only contains groups `phase_integration_bodies` built a
        // harness cross-context view for — this lookup cannot miss.
        let cross_context = ready
            .get(group_name)
            .expect("phase_integration_bodies only returns groups it resolved");

        let mut uses_targets: Vec<String> = Vec::new();
        for &i in indices {
            if let Some(d) = parsed[i].integration() {
                for u in &d.uses {
                    let q = u.target.joined();
                    if !uses_targets.contains(&q) {
                        uses_targets.push(q);
                    }
                }
            }
        }

        // -- Emit the integration module. --
        // Collect each case with the fragment file it came from, so a suite
        // split across files maps each case body under its own source.
        let mut case_inputs: Vec<SystemCaseInput> = Vec::new();
        for &i in indices {
            let Some(d) = parsed[i].integration() else {
                continue;
            };
            let rel_path = parsed[i].identity_path().to_string_lossy().into_owned();
            let map_source = parsed[i].map_source_name();
            for case in &d.cases {
                case_inputs.push(SystemCaseInput {
                    case,
                    tier: super::case_effective_tier(case, d),
                    env: bynk_check::test_env::case_env(d, case),
                    source: parsed[i].source(),
                    rel_path: rel_path.clone(),
                    map_source: map_source.clone(),
                });
            }
        }
        // #2134: an `env` entry lands on every participant's Worker `env`, so
        // it may not name a binding one of them declares, nor a key the
        // harness sets there itself (a call edge's key, a foreign caller's
        // secret), which would silently win over it. `context_bindings` is
        // `None` when earlier errors stop the build, and the check with it.
        if let Some(all) = context_bindings {
            let mut names: BTreeMap<String, String> = BTreeMap::new();
            for p in &harness_participants {
                for b in all.get(p).into_iter().flatten() {
                    names
                        .entry(b.name.clone())
                        .or_insert_with(|| format!("a binding of `{p}`'s Worker"));
                }
                for secret in foreign_caller_secrets(p, unit_tables) {
                    names.entry(secret).or_insert_with(|| {
                        format!("a foreign caller's secret the harness sets on `{p}`'s Worker")
                    });
                }
            }
            // #2137: the harness clock the harness puts on every participant.
            names.entry("__bynkClock".to_string()).or_insert_with(|| {
                "the harness clock the test harness sets on the participants' Workers".to_string()
            });
            // Every edge key the harness may set, from its own caller and
            // between participants (`emit_integration_harness`).
            let callers = std::iter::once(HARNESS_CALLER)
                .chain(harness_participants.iter().map(String::as_str));
            for key in callers.filter_map(wire_format::edge_key_from).chain(
                harness_participants
                    .iter()
                    .filter_map(|p| wire_format::edge_key_to(p)),
            ) {
                names.entry(key).or_insert_with(|| {
                    "a call-edge key the harness sets on the participants' Workers".to_string()
                });
            }
            for &i in indices {
                if let Some(d) = parsed[i].integration() {
                    bynk_check::test_env::check_env_binding_clash(d, &names, errors);
                }
            }
        }
        // #1814: the harness view the cases were checked in.
        let view =
            test_suites::integration_view(&uses_targets, &participants, unit_tables, unit_uses);
        if let Some((path, program, source_map, runnable)) = emit_integration_module(
            &suite_name,
            &participants,
            &harness_participants,
            context_bindings,
            &uses_targets,
            &view,
            cross_context,
            unit_tables,
            &case_inputs,
            participants.first().and_then(|target| {
                asset_harness(target, unit_uses, unit_consumes, unit_tables, asset_files)
            }),
            remote,
            errors,
            tys,
        ) {
            outputs.push(StagedFile {
                output_path: path,
                document: Document::Ts(program),
                source_map,
                debug_metadata: None,
            });
            runnables.push(runnable);

            // v0.132: the integration module imports each `uses` commons as a
            // namespace (`import * as ns from "./<name>.js"`); participants come
            // in through `../workers/`, so only `uses_targets` need a barrel.
            // Emit one (deduped) for each that is a multi-file commons.
            // #1814: and the owner of each reached type it imports.
            let owners: BTreeSet<&String> = view.hidden.values().collect();
            for name in uses_targets.iter().chain(owners) {
                if let Some(barrel) =
                    emit_commons_barrel(name, groups, parsed, ImportExt::Js, emitted_barrels, &[])
                {
                    outputs.push(barrel);
                }
            }
        }
    }

    (outputs, runnables)
}

/// I3 (#1971): the secrets participant `p`'s foreign callers sign under, which
/// the harness sets on its `env` so a case calling `by <ForeignActor>` signs
/// as that actor.
fn foreign_caller_secrets(p: &str, unit_tables: &HashMap<String, UnitTable>) -> Vec<String> {
    let Some(t) = unit_tables.get(p) else {
        return Vec::new();
    };
    let mut secrets: Vec<String> = t
        .services
        .values()
        .flat_map(|s| s.handlers.iter())
        .filter_map(|h| bynk_check::actors::call_admission_for(h, &t.actors))
        .flat_map(|a| a.foreign.into_iter().map(|f| f.secret))
        .collect();
    secrets.sort();
    secrets.dedup();
    secrets
}

/// v0.118: one `system`-tier case paired with the fragment file it was declared
/// in, so [`emit_integration_module`] maps each case body under its own source.
struct SystemCaseInput<'a> {
    case: &'a Case,
    /// The case's effective tier, which its suite default may supply.
    tier: bynk_syntax::ast::TestTier,
    /// #2134: the variables and secrets the case runs with (its suite's
    /// `env`, overlaid by its own).
    env: BTreeMap<String, String>,
    source: &'a str,
    rel_path: String,
    map_source: String,
}

/// Emit a single integration-test module plus its [`RunnableTest`] pointer.
/// The module imports each participant's workers-mode handler namespace (for
/// serialise/deserialise) and Worker entry (for dispatch), builds an in-process
/// env graph wiring the Service Bindings, and runs each case across the wire.
#[allow(clippy::too_many_arguments)]
fn emit_integration_module(
    suite: &str,
    // The contexts a case reaches (`infer_participants`).
    participants: &[String],
    // #1959: the contexts the harness stands up as Workers
    // (`infer_harness_participants`): `participants` plus event subscribers.
    harness_participants: &[String],
    context_bindings: Option<&BTreeMap<String, Vec<WorkerBinding>>>,
    uses_targets: &[String],
    view: &test_suites::SuiteView,
    cross_context: &resolver::CrossContextInfo,
    unit_tables: &HashMap<String, UnitTable>,
    cases: &[SystemCaseInput],
    // #1883, slice 3: `asset_harness`'s text, when the target ships assets.
    asset_harness: Option<String>,
    // #2138: a remote test build — drivers reach the target's URLs, and a case
    // a remote run can't honour is refused (into `errors`) or skipped.
    remote: Option<&crate::project::RemoteTests>,
    errors: &mut Vec<CompileError>,
    tys: &Arc<Types>,
) -> Option<(PathBuf, TsProgram, Option<String>, RunnableTest)> {
    // #914: the test-scaffold module's runtime import list is emitted below as a
    // fixed set, but the bodies spliced into it lower through the ordinary `Bytes`
    // and boundary-codec paths. What those reach for is recorded here as they emit
    // and injected into the import line as a post-pass, the same shape
    // `emit_project` and `emit_worker_entry` use.
    //
    // #917: a case body here can still call `Json.decode[T]`/`Json.encode` on a
    // named type, and `lower_json_codec_call` still records the root on this
    // `runtime_use` (it is gated on `cx.test_scaffold`, which this harness sets
    // regardless). Nothing drains it — `emit_test_module`'s codec-closure pass is
    // unit-tier-only — so a delegating call here is left exactly as broken as it
    // was before this fix. Deliberate scope, per the issue's own hedge ("the
    // driver path may already be correct — worth confirming rather than
    // assuming"); not yet confirmed either way.
    let runtime_use = RuntimeUse::default();
    let sanitized = sanitise_suite(suite);
    let module_path = PathBuf::from(format!("tests/integration_{sanitized}.test.ts"));
    // #1486: `stmts` collects the module's own real top-level nodes instead of
    // `out: String` — printed once at the boundary via `bynk_ts::print_multi_
    // source`, which builds the source map itself (per-statement `nested_map`/
    // `nested_map_source_id` merges) rather than this function driving a
    // `SourceMapBuilder` by hand through `print_stmt_and_merge`.
    let mut stmts: Vec<TsStmt> = Vec::new();
    // v0.70: the integration module's own aggregate map. A `system` suite may
    // span several fragment files, so it is multi-source; each case's body is
    // registered under its own source (on first sight, in the case loop below)
    // and merged there via that case's own top-level `nested_map_source_id`.
    // v0.72: keyed on each fragment's absolute path so an editor breakpoint on
    // the real `.bynk` binds; the per-case `rel_path` stays the test-runner
    // location.
    let mut map = SourceMapBuilder::new();
    stmts.push(TsStmt::comment(
        "Generated by bynkc — do not edit by hand.",
        None,
    ));
    stmts.push(TsStmt::comment(format!("system test: {suite}"), None));

    // Runtime imports. When a participant owns agents, also pull in the
    // Durable-Object namespace helper + types for the in-memory DO stubs.
    // #1486: this import statement's own final `names` depend on `runtime_use`,
    // which the case-body lowering loop below still populates as it runs — its
    // real construction is deferred to the end of this function (the same
    // reordering `emit_project`'s own `write_header` already does) and spliced
    // in at this fixed position (index 2, right after the two header comments
    // above) once every case has lowered.
    // #1959: what the harness binds, from each harness participant's own
    // Worker bindings.
    // `None` only when earlier errors stop the build, so nothing runs this
    // module; otherwise every context has a list (review of #1964).
    let empty = Vec::new();
    let bindings_of = |p: &String| {
        debug_assert!(
            context_bindings.is_none_or(|all| all.contains_key(p)),
            "harness participant `{p}` has no binding list"
        );
        context_bindings
            .and_then(|all| all.get(p))
            .unwrap_or(&empty)
    };
    let binds = |kind: fn(&BindingSource) -> bool| {
        harness_participants
            .iter()
            .any(|p| bindings_of(p).iter().any(|b| kind(&b.source)))
    };
    let has_objects = binds(|s| {
        matches!(
            s,
            BindingSource::Agent { .. } | BindingSource::EventsFanout { .. }
        )
    });
    let has_kv = binds(|s| matches!(s, BindingSource::Kv | BindingSource::KvInstance { .. }));
    let has_r2 = binds(|s| matches!(s, BindingSource::R2Instance { .. }));
    let has_ratelimit = binds(|s| matches!(s, BindingSource::RateLimitInstance { .. }));
    let has_queue = binds(|s| matches!(s, BindingSource::QueueProducer { .. }));
    let has_analytics = binds(|s| matches!(s, BindingSource::AnalyticsDataset { .. }));
    let has_d1 = binds(|s| matches!(s, BindingSource::D1Database { .. }));
    let runtime_import = emitter::runtime_import_for(&module_path, ImportExt::Js);
    let mut runtime_names: Vec<String> = [
        "Ok",
        "Err",
        "Some",
        "None",
        "__callService",
        "type Result",
        "type Option",
        "type ValidationError",
        "type JsonError",
        "type __JsonValue",
        "type __BoundaryError",
        "type __ServiceBinding",
        "__responseToHttpResult",
        "__responseToHttpOutcome",
        "__responseToUnauthOutcome",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // #1883, slice 3: the asset fetch helper answers with an `HttpResult`.
    if asset_harness.is_some() {
        runtime_names.push("HttpResult".to_string());
    }
    if has_kv {
        runtime_names.push("__makeIntegrationKv".to_string());
    }
    if has_r2 {
        runtime_names.push("__makeIntegrationR2".to_string());
    }
    if has_ratelimit {
        runtime_names.push("__makeIntegrationRateLimit".to_string());
    }
    if has_queue {
        runtime_names.push("__makeIntegrationQueue".to_string());
    }
    if has_analytics {
        runtime_names.push("__makeIntegrationAnalytics".to_string());
    }
    if has_d1 {
        runtime_names.push("__makeIntegrationD1".to_string());
    }
    if has_objects {
        runtime_names.push("__makeIntegrationDoNamespace".to_string());
        runtime_names.push("type __DurableObjectState".to_string());
        runtime_names.push("type __DurableObjectNamespace".to_string());
    }
    let import_idx = stmts.len();

    // Per-participant: workers handler namespace + Worker entry default export,
    // and the events fan-out class of one that emits (#1959).
    for p in harness_participants {
        let ns = test_ns(p);
        let dir = worker_dir_name(p);
        // A case's codecs and the agent classes come from here; a subscriber
        // the harness only stands up, with no agent, needs neither.
        let has_agent = bindings_of(p)
            .iter()
            .any(|b| matches!(b.source, BindingSource::Agent { .. }));
        if participants.contains(p) || has_agent {
            stmts.push(TsStmt::decl(
                TsDecl::ImportNamespace {
                    type_only: false,
                    alias: ns.clone(),
                    from: format!("../workers/{dir}/handlers.js"),
                },
                None,
            ));
        }
        stmts.push(TsStmt::decl(
            TsDecl::ImportDefault {
                alias: format!("worker_{ns}"),
                from: format!("../workers/{dir}/index.js"),
            },
            None,
        ));
        if bindings_of(p)
            .iter()
            .any(|b| matches!(b.source, BindingSource::EventsFanout { .. }))
        {
            stmts.push(TsStmt::decl(
                TsDecl::ImportNamespace {
                    type_only: false,
                    alias: fanout_alias(&ns),
                    from: format!("../workers/{dir}/events_fanout.js"),
                },
                None,
            ));
        }
    }

    // `uses` commons (for constructing arguments).
    let mut uses_imports: Vec<(String, String)> = Vec::new();
    for u in uses_targets {
        let ns = test_ns(u);
        let path = relative_import_for_test(&commons_dir_for(u));
        uses_imports.push((ns, path));
    }
    // #1814: and the commons owning each type the harness view reaches.
    let hidden = hidden_by_owner(&view.hidden);
    for owner in hidden.keys() {
        let path = relative_import_for_test(&commons_dir_for(owner));
        uses_imports.push((test_ns(owner), path));
    }
    // #1883, slice 3: the unit owning each asset a case may name. A
    // participant's own assets are exported by its `handlers.js` (already
    // imported above); a commons' come from that commons' module.
    let asset_owners = system_asset_owners(view, participants, unit_tables);
    for owner in asset_owners.keys() {
        if !participants.contains(owner) {
            let path = relative_import_for_test(&commons_dir_for(owner));
            uses_imports.push((test_ns(owner), path));
        }
    }
    uses_imports.sort();
    uses_imports.dedup();
    for (ns, path) in &uses_imports {
        stmts.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: ns.clone(),
                from: format!("./{path}.js"),
            },
            None,
        ));
    }

    // Expectation helper used by lowered `expect` statements — committed `.ts`
    // source (Decision C), carried as one `Raw` statement. Its own text already
    // ends in a blank line, so the harness statement right after it suppresses
    // the printer's own automatic lead-in blank (`no_blank_before`) rather than
    // stacking a second one on top.
    stmts.push(TsStmt::raw(expectation_runtime_helpers(), None));
    // #1883, slice 3: the embedded files and `__bynkAssetGet`, for an asset
    // fetch in a case.
    if let Some(text) = &asset_harness {
        stmts.push(TsStmt::raw(text.clone(), None));
    }

    // #1994: a participant whose handlers send with `~>` is driven with a
    // collecting execution context, settled before its response returns.
    // L7 (#2414): so is one with an `EdgeCache` instance, which purges
    // through that context's `cache` double.
    let sends = |p: &String| {
        target_uses_send(p, UnitKind::Context, unit_tables)
            || unit_tables
                .get(p)
                .is_some_and(|t| t.derived.values().any(|d| d.is_edge_cache()))
    };
    if harness_participants.iter().any(sends) {
        stmts.push(TsStmt::raw(send_runtime_helpers(), None));
    }
    // #2138: `run.id`, for a module whose cases read it.
    if cases.iter().any(|c| block_uses_run_id(&c.case.body)) {
        stmts.push(TsStmt::raw(run_id_helper(remote.is_some()), None));
    }
    // #2137: the harness clock every participant's `Clock` reads.
    stmts.push(TsStmt::raw(clock_runtime_helpers(), None));

    // The env-graph harness: stand each participant up as an in-process Worker
    // and wire its Service Bindings to its siblings; the root env binds to all.
    // I3 (#1971): the secrets a participant's foreign callers sign under, so
    // a case calling `by <ForeignActor>` signs as that actor.
    let foreign_secrets = |p: &String| foreign_caller_secrets(p, unit_tables);
    // #2136: the target's cron and queue services, driven through the
    // participant Worker's `scheduled` / `queue` entry points.
    // A remote run drives neither (`remote_unsupported` refuses both).
    let job_support = if remote.is_some() {
        SystemJobSupport {
            code: Vec::new(),
            services: emitter::SystemJobServices::default(),
        }
    } else {
        emit_system_job_support(
            suite,
            unit_tables,
            &runtime_use,
            participants.first().is_some_and(sends),
        )
    };
    let exposed: Vec<String> = if job_support.code.is_empty() {
        Vec::new()
    } else {
        participants.first().cloned().into_iter().collect()
    };
    // #2134: when a case runs with `env` entries, the running case's map is
    // held in a module-level `__bynkEnv`, which each case runner sets first:
    // the case's harness puts it on each participant's `env`, and a Bearer
    // driver signs with its secret.
    let with_env = cases.iter().any(|c| !c.env.is_empty());
    if with_env {
        let mut env_decl = TsStmt::raw(
            "// #2134: the running case's `env` entries, set by each case runner.\nlet __bynkEnv: globalThis.Record<string, string> = {};\n",
            None,
        );
        env_decl.no_blank_before = true;
        stmts.push(env_decl);
    }
    // #2138: a remote run's harness binds each participant to its URL in the
    // test target instead of standing it up in process.
    let harness = if remote.is_some() {
        emit_remote_harness(participants)
    } else {
        emit_integration_harness(
            harness_participants,
            &bindings_of,
            &sends,
            &foreign_secrets,
            &exposed,
            with_env,
        )
    };
    for (i, stmt) in harness.into_iter().enumerate() {
        let mut stmt = stmt;
        stmt.no_blank_before = !(with_env && i == 0);
        stmts.push(stmt);
    }

    // Slice B: the test-only signer + per-route drivers for the target's http
    // service, and the set of http service names so the lowering calls a driver.
    let http_support =
        emit_system_http_support(suite, unit_tables, &runtime_use, with_env, remote.is_some());
    // #1486: `http_support.code`'s own elements print glued (no blank between
    // any of them — the same shape `extend_printed_at` gave them pre-
    // conversion), so every element but the first suppresses the printer's
    // automatic lead-in; the first relies on it (exactly one blank after the
    // harness above, matching the pre-conversion manual blank).
    for (i, mut stmt) in http_support.code.into_iter().enumerate() {
        if i > 0 {
            stmt.no_blank_before = true;
        }
        stmts.push(stmt);
    }
    for (i, mut stmt) in job_support.code.into_iter().enumerate() {
        if i > 0 {
            stmt.no_blank_before = true;
        }
        stmts.push(stmt);
    }

    // #2138: under a remote run, a case that uses what a real runtime can't
    // honour is refused, or, under `--skip-unsupported`, skipped with its
    // reason (`RunnableTest::skipped`).
    let mut skipped: Vec<crate::project::RemoteSkip> = Vec::new();
    let mut envs: Vec<BTreeMap<String, String>> = Vec::new();
    let cases: Vec<&SystemCaseInput> = cases
        .iter()
        .filter(|input| {
            let Some(remote) = remote else {
                return true;
            };
            let mut throwaway_errors: Vec<CompileError> = Vec::new();
            let mut throwaway_refs = RefSink::new();
            let (expr_types, callees) = test_suites::typecheck_integration_case_body(
                participants,
                view,
                input.case,
                cross_context,
                unit_tables,
                &mut throwaway_errors,
                &mut throwaway_refs,
                tys,
            );
            // #2138 (§3.4): a warning for an `expect` on an absolute instant.
            if let Some(warning) = test_suites::remote_clock_dependent(input.case, &expr_types, tys)
            {
                errors.push(warning);
            }
            // #2138 (§3.7): and, against a named target, its `env` must agree
            // with the target's `vars`. Under `dev` the runner applies `env`
            // itself, one `bynk dev` per distinct map.
            let refusal = test_suites::remote_unsupported(input.case, &callees).or_else(|| {
                (!remote.dev)
                    .then(|| {
                        bynk_check::test_env::remote_env_disagreement(
                            &input.case.name,
                            input.case.name_span,
                            &input.env,
                            &remote.vars,
                            &remote.secrets,
                        )
                    })
                    .flatten()
            });
            let Some(refusal) = refusal else {
                if remote.dev {
                    if !envs.contains(&input.env) {
                        envs.push(input.env.clone());
                    }
                    if let Some(only) = &remote.only_env {
                        return &input.env == only;
                    }
                }
                return true;
            };
            if remote.skip_unsupported {
                skipped.push(crate::project::RemoteSkip {
                    case: input.case.name.clone(),
                    code: refusal.category.to_string(),
                    reason: refusal.message.clone(),
                });
            } else {
                errors.push(refusal);
            }
            false
        })
        .collect();

    // One async function per case.
    let mut typed = integration_typed_commons(view, tys);
    let mut case_runners: Vec<String> = Vec::new();
    let mut discovered: Vec<DiscoveredCase> = Vec::new();
    for input in cases.iter().copied() {
        let case = input.case;
        discovered.push(DiscoveredCase {
            name: case.name.clone(),
            location: Some(discovered_location(
                input.source,
                &input.rel_path,
                case.name_span,
            )),
            tier: Some(input.tier.as_str()),
            generative: None,
            doc: false,
        });
        let runner_name = sanitise_case_name(&case.name, &mut case_runners.len());
        case_runners.push(runner_name.clone());
        // #1484: everything below builds into a local `case_out`/`case_smb`
        // pair instead of this module's own top-level state directly — the
        // same local-buffer-then-wrap shape `emit_test_case_function`'s own
        // (separate-callee) sibling uses, needed here too since this is a
        // real `TsDecl::Function` (`async_test_runner_fn`) rather than
        // hand-templated text spliced straight into the module.
        let mut case_out = String::new();
        case_out.push_str("  try {\n");
        if with_env {
            let entries: Vec<(String, TsExpr)> = input
                .env
                .iter()
                .map(|(k, v)| (k.clone(), str_lit(v.clone())))
                .collect();
            crate::emitter::extend_printed_at(
                &mut case_out,
                vec![TsStmt::assign(
                    ident("__bynkEnv"),
                    TsExpr::object(entries),
                    None,
                )],
                2,
            );
        }
        // #2136: the drivers and the case share one harness, so state a
        // call leaves behind is what the next call in the case reads.
        case_out.push_str("    __bynkHarness = undefined;\n");
        // #2138 (track §3.6): a remote case reports how often `--settle`
        // retried it.
        if remote.is_some() {
            case_out.push_str("    __bynkRetries = 0;\n");
        }
        // #2137: each case's harness clock starts at the epoch.
        case_out.push_str("    __bynkClockNow = 0;\n");
        case_out.push_str("    const deps = __makeHarness();\n");
        // #2024: every name this scaffold binds in the case's scope, so the
        // body's lowering renames a `let` that would redeclare one.
        let mut harness_bound: HashSet<String> = HashSet::from(["deps".to_string()]);
        // Bring `uses` commons names into scope for argument construction.
        for u in uses_targets {
            let ns = test_ns(u);
            if let Some(table) = unit_tables.get(u) {
                let mut names: Vec<String> = table
                    .types
                    .keys()
                    .chain(table.fns.keys())
                    .cloned()
                    .collect();
                names.sort();
                names.dedup();
                let type_names = aliased_types(table, |_| true);
                harness_bound.extend(names.iter().cloned());
                crate::emitter::extend_printed_at(
                    &mut case_out,
                    emit_ns_destructure(&ns, &names, &type_names),
                    2,
                );
            }
        }
        extend_hidden_destructures(&mut case_out, &hidden, unit_tables);
        harness_bound.extend(hidden.values().flatten().cloned());
        // #1883, slice 3: the assets a case may name, from their owners.
        for (owner, names) in &asset_owners {
            harness_bound.extend(names.iter().cloned());
            crate::emitter::extend_printed_at(
                &mut case_out,
                emit_ns_destructure(&test_ns(owner), names, &[]),
                2,
            );
        }
        // #2073: a participant's type the case names (`Region.Domestic`, a
        // consumed context's enum the view brings in), from the namespace of
        // the participant declaring it. Reserved here, before the lowering, so
        // a `let` of the same name is renamed; destructured below only when
        // the lowered body names it, so a case that names none is unchanged.
        let mut participant_types: Vec<(String, Vec<String>)> = Vec::new();
        for p in participants {
            let Some(t) = unit_tables.get(p) else {
                continue;
            };
            let mut names: Vec<String> = t
                .types
                .keys()
                // The view keeps the first participant's declaration of a
                // name (`integration_view`), so only that one binds it.
                .filter(|n| {
                    !harness_bound.contains(*n)
                        && view.types.get(*n).is_some_and(|d| {
                            let mine = &t.types[*n];
                            Arc::ptr_eq(d, mine) || d.span == mine.span
                        })
                })
                .cloned()
                .collect();
            names.sort();
            harness_bound.extend(names.iter().cloned());
            participant_types.push((p.clone(), names));
        }
        // #1855: the lowering reads the case's checked types (a literal at a
        // refined record field is branded only when its type says so), so
        // type the body here as the check pass did, into throwaway sinks.
        let mut throwaway_errors: Vec<CompileError> = Vec::new();
        let mut throwaway_refs = RefSink::new();
        (typed.expr_types, typed.callees) = test_suites::typecheck_integration_case_body(
            participants,
            view,
            case,
            cross_context,
            unit_tables,
            &mut throwaway_errors,
            &mut throwaway_refs,
            tys,
        );
        // #1875 review: the check pass accepted this body from the same
        // inputs, so an error here means the two have drifted, and the
        // lowering would silently fall back to untyped output.
        debug_assert!(
            throwaway_errors.is_empty(),
            "a system case re-typed for emission reported errors the check pass did not: {:?}",
            throwaway_errors
                .iter()
                .map(|e| e.category)
                .collect::<Vec<_>>()
        );
        let (body_src, body_smb) = emitter::lower_integration_case_body(
            &case.body,
            &mut typed,
            cross_context,
            http_support.http_services.clone(),
            http_support.declared_routes.clone(),
            http_support.route_body.clone(),
            http_support.type_ns.clone(),
            http_support.claims.clone(),
            job_support.services.clone(),
            harness_bound,
            input.source,
            &input.rel_path,
            &runtime_use,
            remote.is_some(),
        );
        for (p, names) in &participant_types {
            let named: Vec<String> = names
                .iter()
                .filter(|n| names_ident(&body_src, n))
                .cloned()
                .collect();
            if named.is_empty() {
                continue;
            }
            let Some(t) = unit_tables.get(p) else {
                continue;
            };
            let type_names = aliased_types(t, |n| named.contains(n));
            crate::emitter::extend_printed_at(
                &mut case_out,
                emit_ns_destructure(&test_ns(p), &named, &type_names),
                2,
            );
        }
        let src_id = map.add_source(input.map_source.clone(), input.source.to_string());
        let body_base = case_out.len();
        for line in body_src.lines() {
            case_out.push_str("    ");
            case_out.push_str(line);
            case_out.push('\n');
        }
        let mut case_smb = SourceMapBuilder::new();
        case_smb.merge(&body_smb, &body_src, &case_out, body_base, 0);
        let retries = if remote.is_some() {
            ", retries: __bynkRetries"
        } else {
            ""
        };
        case_out.push_str(&format!("    return {{ pass: true{retries} }};\n"));
        case_out.push_str("  } catch (e) {\n");
        case_out.push_str("    if (e instanceof ExpectationError) {\n");
        case_out.push_str(&format!(
            "      return {{ pass: false, error: {{ message: e.message, location: e.location }}{retries} }};\n"
        ));
        case_out.push_str("    }\n");
        case_out.push_str(&format!(
            "    return {{ pass: false, error: {{ message: String(e), location: \"unknown\", violation: __bynkViolationOf(e) }}{retries} }};\n"
        ));
        case_out.push_str("  }\n");
        let mut case_stmt = async_test_runner_fn(&runner_name, case_out, Some(case_smb));
        case_stmt.nested_map_source_id = src_id;
        stmts.push(case_stmt);
    }

    // Module runner. v0.127: `only` filters to a single case by name (the
    // per-case run lens); undefined runs every case.
    let run_entries: Vec<(String, String)> = cases
        .iter()
        .enumerate()
        .map(|(idx, input)| (input.case.name.clone(), case_runners[idx].clone()))
        .collect();
    stmts.extend(build_run_function(&run_entries));

    // #914: every case body above has now lowered, so `runtime_use` carries
    // whatever it actually reached for — fold that into the runtime import's
    // own `names` (deduped against the fixed set already listed) and splice
    // the finished import statement in at its reserved position, the same
    // ordering `emit_project`'s own deferred `write_header` relies on.
    if runtime_use.boundary_codec() {
        append_missing_bindings(&mut runtime_names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
    }
    if runtime_use.json_codec() {
        append_missing_bindings(&mut runtime_names, emitter::JSON_CODEC_RUNTIME_IMPORTS);
    }
    if runtime_use.bytes() {
        append_missing_bindings(&mut runtime_names, emitter::BYTES_RUNTIME_IMPORTS);
    }
    if runtime_use.bytes_ops() {
        append_missing_bindings(&mut runtime_names, emitter::BYTES_OPS_RUNTIME_IMPORTS);
    }
    if runtime_use.time_ops() {
        append_missing_bindings(&mut runtime_names, emitter::TIME_OPS_RUNTIME_IMPORTS);
    }
    if runtime_use.url_ops() {
        append_missing_bindings(&mut runtime_names, emitter::URL_OPS_RUNTIME_IMPORTS);
    }
    if runtime_use.request_headers() {
        append_missing_bindings(&mut runtime_names, emitter::REQUEST_HEADERS_RUNTIME_IMPORTS);
    }
    if runtime_use.request_header_pairs() {
        append_missing_bindings(&mut runtime_names, ", __requestHeaderPairs");
    }
    if runtime_use.token_claims() {
        append_missing_bindings(&mut runtime_names, ", __tokenClaims");
    }
    if runtime_use.eq() {
        append_missing_bindings(&mut runtime_names, emitter::EQ_RUNTIME_IMPORTS);
    }
    // #1934: a case that compares a typed http outcome with `Ok(…)`.
    if runtime_use.http_result() {
        append_missing_bindings(&mut runtime_names, "HttpResult");
    }
    if runtime_use.int() {
        append_missing_bindings(&mut runtime_names, emitter::INT_RUNTIME_IMPORTS);
    }
    stmts.splice(
        import_idx..import_idx,
        emitter::runtime_files::runtime_import_stmts(false, &runtime_names, &runtime_import),
    );

    let module_file = module_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "module.ts".to_string());
    let program = TsProgram { stmts };
    let printed = bynk_ts::print_multi_source(&program, map, &module_file);
    Some((
        module_path.clone(),
        program,
        printed.source_map,
        RunnableTest {
            // #2185: headed by the suite's tier, `system` — the runner's
            // `PREFIX` (`emit_test_main`) strips the same text back off.
            target_name: format!("system · {suite}"),
            module_path,
            kind: "integration",
            suite_name: suite.to_string(),
            cases: discovered,
            skipped,
            envs,
            remote: remote.is_some(),
            generative: false,
        },
    ))
}

/// #2073: whether the lowered TypeScript `src` names the identifier `name`,
/// as a whole word outside a member access (`x.Region` names a property) and
/// outside a string literal (an `expect`'s failure message quotes the source)
/// or a `//` or `/* */` comment. A template literal's text is skipped too, but
/// its `${…}` holes are code.
fn names_ident(src: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$';
    // `code` is `src` with every literal character blanked, so offsets and
    // word boundaries are kept.
    let mut code = String::with_capacity(src.len());
    // `None`: code. `Some(q)`: inside a string quoted by `q`. A template's
    // hole pushes the template, and its braces, on `stack`.
    let mut quote: Option<char> = None;
    let mut stack: Vec<usize> = Vec::new(); // brace depth in each open hole
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == '\\' {
                    code.push(' ');
                    if chars.next().is_some() {
                        code.push(' ');
                    }
                    continue;
                }
                if q == '`' && c == '$' && chars.peek() == Some(&'{') {
                    chars.next();
                    code.push_str("  ");
                    stack.push(0);
                    quote = None;
                    continue;
                }
                if c == q {
                    quote = None;
                }
                code.push(' ');
            }
            None => match c {
                '"' | '\'' | '`' => {
                    quote = Some(c);
                    code.push(' ');
                }
                '/' if chars.peek() == Some(&'/') => {
                    // A line comment runs to the end of the line.
                    code.push(' ');
                    for d in chars.by_ref() {
                        if d == '\n' {
                            code.push('\n');
                            break;
                        }
                        code.push(' ');
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    // A block comment runs to the next `*/`.
                    chars.next();
                    code.push_str("  ");
                    let mut star = false;
                    for d in chars.by_ref() {
                        code.push(' ');
                        if star && d == '/' {
                            break;
                        }
                        star = d == '*';
                    }
                }
                '{' if !stack.is_empty() => {
                    *stack.last_mut().expect("non-empty") += 1;
                    code.push(c);
                }
                '}' if !stack.is_empty() => {
                    let depth = stack.last_mut().expect("non-empty");
                    if *depth == 0 {
                        // The hole closes; the template resumes.
                        stack.pop();
                        quote = Some('`');
                        code.push(' ');
                    } else {
                        *depth -= 1;
                        code.push(c);
                    }
                }
                _ => code.push(c),
            },
        }
    }
    code.match_indices(name).any(|(i, _)| {
        let before = code[..i].chars().next_back();
        let after = code[i + name.len()..].chars().next();
        !before.is_some_and(|c| is_ident(c) || c == '.') && !after.is_some_and(is_ident)
    })
}

/// #1486: `emitter/workers.rs`'s own `append_missing_bindings`'s sibling
/// duplicate — same reason that one and `emitter/workers_entry.rs`'s own copy
/// don't share a single implementation (each doc comment explains why for its
/// own file): folds `extra`'s comma-separated bindings (e.g.
/// [`crate::emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS`]) into `names` directly,
/// skipping anything already present under the same "strip an optional `type
/// ` prefix, compare bare names" dedup rule. `emit_test_module`/
/// `emit_integration_module`'s own runtime-import statement is now a real
/// `TsDecl::Import` built once every case has lowered (deferred, not spliced
/// into already-printed text the way the pre-#1486 code's own
/// `emitter::inject_runtime_imports` post-pass did — see each caller's own
/// doc comment) — this is that deferred construction's own merge step.
fn append_missing_bindings(names: &mut Vec<String>, extra: &str) {
    fn bare(binding: &str) -> &str {
        binding
            .trim()
            .strip_prefix("type ")
            .unwrap_or(binding.trim())
    }
    let present: std::collections::HashSet<&str> = names.iter().map(|s| bare(s)).collect();
    let mut to_add: Vec<String> = Vec::new();
    for b in extra.split(',').map(str::trim) {
        if !b.is_empty() && !present.contains(bare(b)) && !to_add.iter().any(|a| bare(a) == bare(b))
        {
            to_add.push(b.to_string());
        }
    }
    names.extend(to_add);
}

/// testing-the-boundary Slice B: for a `system` suite whose target exposes an
/// `http` service, emit a **test-only** HS256 signer and one **driver** per
/// route. A driver builds a real `Request` (concrete path, JSON body, a signed
/// `Authorization: Bearer …`), drives the target Worker's public `fetch`, and
/// decodes the `Response` back to `HttpResult[T]`. The developer's `by
/// User("bob")` supplies the `sub`; the framework signs it (the developer never
/// hand-crafts auth — real-token ceremony is an e2e concern), and the real,
/// unmodified emitted Worker verifies it. Returns the emitted TS plus the set of
/// the target's http service names, so the case-body lowering knows to call a
/// driver (`__sysdrive_<svc>_<key>`) rather than `callService`.
/// The declared `(service, method, path)` http routes of a system target — the
/// set the lowering checks to tell a normal call from a **wrong-method** call
/// (#707): a `(method, path)` not in the set, whose *path* is, drives the `405`
/// fall-through through the generic `__sysdrive_wrongmethod_<svc>` driver.
type DeclaredRoutes = std::collections::HashSet<(String, String, String)>;
/// #708: for each declared route with a body param, the body's zero-based
/// position among the route's positional call args and its declared type —
/// what the raw driver's mixed typed+`Wire` call-site lowering needs to
/// serialise a typed body into the raw driver's `string` slot.
type RouteBodyMap = HashMap<(String, String, String), (usize, bynk_syntax::ast::TypeRef)>;

/// The emitted `emit_system_http_support` output: the driver/signer TS
/// source plus the metadata the case-body lowering needs to route and
/// convert calls.
///
/// #1479: `code` is now real [`TsStmt`]s (was pre-printed `String`) — the
/// HS256 signer block (`__bynkNow`/`__b64url`/`__bytesB64url`/
/// `__bynkSignHs256`) is committed `.ts` source (`emitter/test_runtime/
/// jwt_signer.ts`, #1485), spliced in via [`jwt_signer_runtime_helpers`] and
/// carried as one [`bynk_ts::TsStmtKind::Raw`] statement, the same carrier
/// this track's earlier still-`String`-typed-sibling conversions already
/// used.
struct SystemHttpSupport {
    code: Vec<TsStmt>,
    http_services: std::collections::HashSet<String>,
    declared_routes: DeclaredRoutes,
    /// #708: per-route body-param position/type, for the raw driver's
    /// mixed typed+`Wire` call-site conversion.
    route_body: RouteBodyMap,
    /// The target's type namespace (`<target>.`), `route_body`'s types
    /// resolve `serialise_*` calls through.
    type_ns: String,
    /// #2019: the refinement-guarded routes and each refinement actor's
    /// witness claims, so a call acting as a refinement signs its claims.
    claims: emitter::SystemHttpClaims,
}

/// #2135: what a `Signature`-secured route's drivers sign with — the actor's
/// declared seam and how the driver reads its secret (the case's `env` entry,
/// else the fixed test secret in `process.env`).
struct SigDriver {
    seam: bynk_check::actors::SignatureSeam,
    secret_read: String,
}

/// #2135: which signature a driver sends.
#[derive(Clone, Copy)]
enum SigVariant {
    /// Signed now, with the real secret.
    Signed,
    /// Signed now, with a secret the seam does not hold.
    Forged,
    /// Signed with the real secret, this many seconds in the past.
    Stale(i64),
}

impl SigDriver {
    fn signed(&self) -> SigVariant {
        SigVariant::Signed
    }

    /// `tolerance + 60` seconds, when the actor declares a timestamp header
    /// and a tolerance: the age of a `by Stale` signature.
    fn stale_age(&self) -> Option<i64> {
        self.seam.timestamp_header.as_ref()?;
        Some(self.seam.tolerance_secs? + 60)
    }

    /// The request headers carrying the signature (and timestamp).
    fn headers(&self) -> String {
        let mut out = format!("{:?}: __sig.signature, ", self.seam.header);
        if let Some(ts) = &self.seam.timestamp_header {
            out.push_str(&format!("{ts:?}: __sig.timestamp, "));
        }
        out
    }

    /// `const __sig = await __bynkSignHmacSha256(<body>, <secret>, …);`.
    fn sign(&self, body: &str, variant: SigVariant) -> TsStmt {
        let (secret, age) = match variant {
            SigVariant::Signed => (self.secret_read.clone(), 0),
            // #2155: a wrong secret derived from the real one, so it differs
            // from whatever the seam holds, `env` entry or test default.
            SigVariant::Forged => (format!("({} + \"!\")", self.secret_read), 0),
            SigVariant::Stale(age) => (self.secret_read.clone(), age),
        };
        TsStmt::const_stmt(
            TsBindingName::Ident("__sig".to_string()),
            None,
            await_expr(call(
                ident("__bynkSignHmacSha256"),
                vec![
                    ident(body),
                    ident(secret),
                    TsExpr::Lit(TsLit::Bool(self.seam.timestamp_header.is_some())),
                    num_lit(age.to_string()),
                ],
            )),
            None,
        )
    }
}

/// #2135, #2155: a signed driver's signing statements — a lone `Signature`
/// actor's signature, or a sum's per-call `__auth` — over `body`.
fn signing_stmts(lone: &Option<SigDriver>, sum: &Option<SumAuth>, body: &str) -> Vec<TsStmt> {
    match (lone, sum) {
        (Some(z), _) => vec![z.sign(body, z.signed())],
        (None, Some(sum)) => sum.stmts(body),
        (None, None) => Vec::new(),
    }
}

/// #2155: an actor-sum route's per-call credential. The driver takes the
/// actor the call acts as (`__as`) and builds the matching credential: a
/// Bearer member's token, a `Signature` member's signature, or nothing for a
/// member with no scheme.
struct SumAuth {
    /// Each member with a credential, in declaration order.
    members: Vec<(String, SumCredential)>,
}

enum SumCredential {
    /// A Bearer member: how the driver reads its secret.
    Bearer(String),
    /// A `Signature` member.
    Signature(SigDriver),
}

impl SumAuth {
    /// The statements building `__auth`, the credential headers for the actor
    /// the call acts as, signing `body` where a member signs.
    fn stmts(&self, body: &str) -> Vec<TsStmt> {
        let mut out = vec![TsStmt::raw(
            "const __auth: globalThis.Record<string, string> = {};",
            None,
        )];
        for (name, cred) in &self.members {
            let set = match cred {
                SumCredential::Bearer(secret_read) => format!(
                    "__auth[\"authorization\"] = `Bearer ${{await __bynkSignHs256({{ sub: __sub, exp: __bynkNow() + 3600 }}, {secret_read})}}`;"
                ),
                SumCredential::Signature(z) => {
                    let mut set = format!(
                        "const __sig = await __bynkSignHmacSha256({body}, {}, {}, 0); __auth[{:?}] = __sig.signature;",
                        z.secret_read,
                        z.seam.timestamp_header.is_some(),
                        z.seam.header
                    );
                    if let Some(ts) = &z.seam.timestamp_header {
                        set.push_str(&format!(" __auth[{ts:?}] = __sig.timestamp;"));
                    }
                    set
                }
            };
            out.push(TsStmt::raw(
                format!("if (__as === {name:?}) {{ {set} }}"),
                None,
            ));
        }
        out
    }

    /// The first `Signature` member: what `by Forged`/`by Stale` sign as.
    fn signature(&self) -> Option<&SigDriver> {
        self.members.iter().find_map(|(_, c)| match c {
            SumCredential::Signature(z) => Some(z),
            SumCredential::Bearer(_) => None,
        })
    }
}

fn emit_system_http_support(
    target: &str,
    unit_tables: &HashMap<String, UnitTable>,
    runtime_use: &RuntimeUse,
    // #2134: whether the module holds the running case's `env` entries
    // (`__bynkEnv`), whose entry for the actor's secret is signed with first.
    with_env: bool,
    // #2138: a remote run signs with the secrets the runner's environment
    // holds, so the fixed test secrets are not assigned over them.
    remote: bool,
) -> SystemHttpSupport {
    use bynk_syntax::ast::{HandlerKind, ServiceProtocol};
    let mut http_services: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut declared: DeclaredRoutes = std::collections::HashSet::new();
    let mut route_body: RouteBodyMap = HashMap::new();
    let Some(table) = unit_tables.get(target) else {
        return SystemHttpSupport {
            code: Vec::new(),
            http_services,
            declared_routes: declared,
            route_body,
            type_ns: String::new(),
            claims: emitter::SystemHttpClaims::default(),
        };
    };
    let ns = test_ns(target);
    let binding = crate::emitter::wrangler::service_binding(target).name;
    let type_ns = format!("{ns}.");

    let mut routes: Vec<TsStmt> = Vec::new();
    let mut secrets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // #2135: whether some route is `Signature`-secured, so the module needs
    // the HMAC signer.
    let mut signs_hmac = false;
    // #2019: each refinement actor's witness claims, signed when a case acts
    // as it on a refinement-guarded route.
    let mut claims = emitter::SystemHttpClaims {
        routes: std::collections::HashSet::new(),
        sum_routes: HashMap::new(),
        actors: table
            .actors
            .iter()
            .filter_map(|(name, decl)| {
                let pred =
                    bynk_check::actors::parse_claim_predicate(&decl.refinement.as_ref()?.predicate)
                        .ok()?;
                Some((name.clone(), bynk_check::actors::claim_witness_js(&pred)?))
            })
            .collect(),
    };

    // How a driver reads an actor's secret: the case's `env` entry (#2134),
    // else the fixed test secret in `process.env`.
    let secret_read_of = |secret: &str| {
        let process_read = format!(
            "(globalThis as {{ process?: {{ env?: globalThis.Record<string, string> }} }}).process?.env?.[{secret:?}]"
        );
        // #2138: a remote run's secret values come from the runner in one
        // JSON variable (`__bynkSecrets`).
        let process_read = if remote {
            format!("(__bynkSecrets[{secret:?}] ?? {process_read})")
        } else {
            process_read
        };
        if with_env {
            format!("(__bynkEnv[{secret:?}] ?? {process_read} ?? \"\")")
        } else {
            format!("({process_read} ?? \"\")")
        }
    };
    let mut svc_names: Vec<&String> = table.services.keys().collect();
    svc_names.sort();
    for sname in svc_names {
        let decl = &table.services[sname];
        if !matches!(decl.protocol, ServiceProtocol::Http) {
            continue;
        }
        http_services.insert(sname.clone());
        for h in &decl.handlers {
            let HandlerKind::Http { method, path } = &h.kind else {
                continue;
            };
            declared.insert((sname.clone(), method.as_str().to_string(), path.clone()));
            let bynk_ir::IrHandlerKind::Http {
                method: ir_method, ..
            } = bynk_lower::lower_handler_kind_ir(&h.kind)
            else {
                unreachable!("h.kind already matched HandlerKind::Http above")
            };
            let key = crate::emitter::http_handler_method_name_ir(ir_method, path);
            // Split the handler's params into path params (matching `:name` in
            // the pattern, in order) and the optional body (the remaining param).
            let path_params: Vec<&str> = path
                .split('/')
                .filter_map(|seg| seg.strip_prefix(':'))
                .collect();
            // #2341 (L4): a `headers: RequestHeaders` param is neither: the
            // drivers send it as the request's headers.
            let is_headers = |p: &&bynk_syntax::ast::Param| {
                p.name.name == "headers"
                    && matches!(p.type_ref, bynk_syntax::ast::TypeRef::RequestHeaders(_))
            };
            let headers_spread = if h.params.iter().any(|p| is_headers(&p)) {
                runtime_use.note_request_header_pairs();
                "...__requestHeaderPairs(headers), "
            } else {
                ""
            };
            let (_path_ps, body_ps): (Vec<_>, Vec<_>) = h
                .params
                .iter()
                .filter(|p| !is_headers(p))
                .partition(|p| path_params.contains(&p.name.name.as_str()));
            // #708: record the body param's position (within the call's
            // positional args, matching `h.params` declaration order) and
            // declared type, so a mixed typed+`Wire` call can serialise a
            // typed body into the raw driver's `string` slot.
            if let Some(bp) = body_ps.first() {
                let idx = h
                    .params
                    .iter()
                    .position(|p| p.name.name == bp.name.name)
                    .expect("body param is drawn from h.params");
                route_body.insert(
                    (sname.clone(), method.as_str().to_string(), path.clone()),
                    (idx, bp.type_ref.clone()),
                );
            }
            // The concrete URL: substitute each `:name` with its param.
            let concrete_path = {
                let mut s = String::new();
                for seg in path.split('/').filter(|x| !x.is_empty()) {
                    s.push('/');
                    if let Some(name) = seg.strip_prefix(':') {
                        s.push_str(&format!("${{{}}}", crate::emitter::ts_ident(name)));
                    } else {
                        s.push_str(seg);
                    }
                }
                if s.is_empty() {
                    s.push('/');
                }
                s
            };
            // The signed Authorization header, if the handler's `by` is Bearer.
            // #2019: a refinement-guarded route's signing drivers take the
            // claims the acting actor presents, signed next to `sub`.
            let mut signs_claims = false;
            let (auth_header, _secret_name) = match bynk_check::actors::bearer_seam_for(
                h,
                &table.actors,
            ) {
                Some(seam) => {
                    secrets.insert(seam.secret.clone());
                    signs_claims = seam.authorization.is_some();
                    let claims_spread = if signs_claims {
                        claims.routes.insert((
                            sname.clone(),
                            method.as_str().to_string(),
                            path.clone(),
                        ));
                        "...__claims, "
                    } else {
                        ""
                    };
                    // P7.2: `Record<string, string>`, not `emitter/workers.rs`'s own
                    // `Record<string, unknown>` marker — this expression feeds
                    // `__bynkSignHs256` directly with no `typeof` narrowing step in
                    // between (unlike workers.rs's own use), and real `process.env`
                    // values genuinely are `string | undefined`, so `string` is both
                    // accurate and what makes `?? ""` actually type as `string`.
                    // #2134: a case's `env` entry for the secret wins over the
                    // fixed test secret, as it does at the seam, which reads
                    // the participant's `env` first.
                    let secret_read = secret_read_of(&seam.secret);
                    (
                        format!(
                            "\"authorization\": `Bearer ${{await __bynkSignHs256({{ {claims_spread}sub: __sub, exp: __bynkNow() + 3600 }}, {secret_read})}}`, "
                        ),
                        Some(seam.secret),
                    )
                }
                None => (String::new(), None),
            };
            // #2155: an actor-sum route's credential depends on the member a
            // call acts as, so its drivers take that actor (`__as`) and build
            // the credential per call, rather than the primary member's.
            let route_key = (sname.clone(), method.as_str().to_string(), path.clone());
            let sum = bynk_check::actors::sum_members_for(h, &table.actors).and_then(|members| {
                let members: Vec<(String, SumCredential)> = members
                    .into_iter()
                    .filter_map(|m| {
                        let cred = match m.seam {
                            bynk_check::actors::SumMemberSeam::None => return None,
                            bynk_check::actors::SumMemberSeam::Bearer { secret, .. } => {
                                secrets.insert(secret.clone());
                                SumCredential::Bearer(secret_read_of(&secret))
                            }
                            bynk_check::actors::SumMemberSeam::Signature(seam) => {
                                secrets.insert(seam.secret.clone());
                                let secret_read = secret_read_of(&seam.secret);
                                SumCredential::Signature(SigDriver { seam, secret_read })
                            }
                        };
                        Some((m.actor_name, cred))
                    })
                    .collect();
                (!members.is_empty()).then_some(SumAuth { members })
            });
            let is_sum = h.by_clause.as_ref().is_some_and(|by| by.is_sum());
            let (auth_header, signs_claims) = if is_sum {
                (String::new(), false)
            } else {
                (auth_header, signs_claims)
            };
            if let Some(primary) = sum
                .as_ref()
                .and(h.by_clause.as_ref())
                .map(|by| by.primary().name.clone())
            {
                claims.sum_routes.insert(route_key.clone(), primary);
            }
            // #2135: a `Signature` route is signed as its actor declares: the
            // driver signs the exact bytes it sends, and the `Forged`/`Stale`
            // variants sign with a wrong secret or in the past. On a sum, they
            // sign as its first `Signature` member (#2155).
            let lone_actor = h.by_clause.as_ref().is_some_and(|by| !by.is_sum());
            let lone_sig = if auth_header.is_empty() && lone_actor {
                bynk_check::actors::signature_seam_for(h, &table.actors).map(|seam| {
                    secrets.insert(seam.secret.clone());
                    let secret_read = secret_read_of(&seam.secret);
                    SigDriver { seam, secret_read }
                })
            } else {
                None
            };
            let sig: Option<&SigDriver> = lone_sig
                .as_ref()
                .or_else(|| sum.as_ref().and_then(SumAuth::signature));
            // The signed drivers' own headers: a lone `Signature` actor's, or
            // a sum's per-call `__auth`.
            let sig_header = match (&lone_sig, &sum) {
                (Some(z), _) => z.headers(),
                (None, Some(_)) => "...__auth, ".to_string(),
                (None, None) => String::new(),
            };
            let bad_sig_header = sig.map(SigDriver::headers).unwrap_or_default();
            signs_hmac |= sig.is_some();
            // Body param → serialise; path params → the URL.
            let body_arg = body_ps.first();
            // A `@raw` route takes its body as the octets themselves, under the
            // declared media type (`application/octet-stream` for `"*/*"`): the
            // driver sends the `Bytes` as it is, with no JSON and no base64.
            let raw_type = h.raw_content_type().map(|ct| {
                if ct == "*/*" {
                    "application/octet-stream"
                } else {
                    ct
                }
            });
            let (body_stmt, body_init): (Option<TsStmt>, &str) = match body_arg {
                Some(p) if raw_type.is_some() => {
                    let stmt = TsStmt::const_stmt(
                        TsBindingName::Ident("__body".to_string()),
                        None,
                        ident(crate::emitter::ts_ident(&p.name.name)),
                        None,
                    );
                    (Some(stmt), "body: __body as BufferSource, ")
                }
                Some(p) => {
                    // #1435 (Arc E slice 1): `serialise_expr_via` now returns
                    // a real `bynk_ts::TsExpr` — this call site is still
                    // `String`-based (not yet converted, out of this
                    // slice's scope), so it prints at the boundary
                    // (`bynk_ts::print_expr`), the same treatment
                    // `lower.rs`'s own call sites use.
                    let ser =
                        bynk_ts::print_expr(&crate::emitter::serialisation::serialise_expr_via(
                            &p.type_ref,
                            &crate::emitter::ts_ident(&p.name.name),
                            &type_ns,
                            runtime_use,
                        ));
                    let stmt = TsStmt::const_stmt(
                        TsBindingName::Ident("__body".to_string()),
                        None,
                        call(
                            member(ident("globalThis.JSON"), "stringify"),
                            vec![ident(ser)],
                        ),
                        None,
                    );
                    (Some(stmt), "body: __body, ")
                }
                None => (None, ""),
            };
            // The response payload deserialiser: the `T` of `Effect[HttpResult[T]]`.
            let payload_deser = match strip_effect_httpresult(&h.return_type) {
                // #1435 (Arc E slice 1): boundary-print, same treatment as
                // `ser` above.
                Some(inner) => {
                    bynk_ts::print_expr(&crate::emitter::serialisation::deserialise_ref_via(
                        inner,
                        &type_ns,
                        runtime_use,
                    ))
                }
                None => format!("{type_ns}__deserialise_unit"),
            };
            // Driver params mirror the handler's params (path params, then body).
            let driver_params: Vec<TsParam> = h
                .params
                .iter()
                .map(|p| TsParam {
                    name: crate::emitter::ts_ident(&p.name.name),
                    ty: Some(TsType::named(driver_param_ty(&p.type_ref, &ns))),
                    optional: false,
                })
                .collect();
            let raw_params: Vec<TsParam> = h
                .params
                .iter()
                .map(|p| TsParam {
                    name: crate::emitter::ts_ident(&p.name.name),
                    ty: Some(TsType::named(if is_headers(&p) {
                        driver_param_ty(&p.type_ref, &ns)
                    } else {
                        "string".to_string()
                    })),
                    optional: false,
                })
                .collect();
            let content_type = match (body_arg.is_some(), raw_type) {
                (true, Some(ct)) => format!("\"content-type\": {ct:?}, "),
                (true, None) => "\"content-type\": \"application/json\", ".to_string(),
                (false, _) => String::new(),
            };
            let url = template(format!("https://test{concrete_path}"));
            // Decision A (#1407): the request-init options object (`{ method,
            // headers: {...}, body, }`) keeps its own hand-formatted text —
            // every branch below bakes in an unconditional trailing `, ` before
            // its closing brace, and prints `{ }` (one space, not
            // `TsExpr::Object`'s own tight `{}`) when a sub-object has zero
            // entries — a shape `TsExpr::Object`'s general single-line algebra
            // (comma-separated, no trailing comma, tight `{}` for zero
            // entries) cannot reproduce byte-for-byte. The same "odd, one-off
            // shape stays text" call this track has made before (Decision B,
            // #1327; Decision C, #1359). Everything AROUND it — the function
            // declaration, its params, the `new Request(...)` call, the
            // `await ... .fetch(...)` chain, the final `return
            // responseTo*(...)` — is real `bynk_ts` structure.
            let typed_options = ident(format!(
                "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}{auth_header}{sig_header}}}, {body_init}}}",
                method = method.as_str(),
            ));
            // #2135: the bytes a signing driver signs: what it sends.
            let typed_body = if body_arg.is_some() { "__body" } else { "\"\"" };
            let raw_body = match body_ps.first() {
                Some(p) => crate::emitter::ts_ident(&p.name.name),
                None => "\"\"".to_string(),
            };
            routes.push(sysdrive_driver(
                "",
                sname,
                &key,
                driver_params.clone(),
                body_stmt.clone(),
                url.clone(),
                typed_options.clone(),
                &binding,
                "__responseToHttpResult",
                &payload_deser,
                signs_claims,
                signing_stmts(&lone_sig, &sum, typed_body),
                sum.is_some(),
            ));
            // Slice C: the raw driver for a `Wire(…)`-carrying call. Every slot is
            // a raw `string` (the wire form the boundary receives *unvalidated*):
            // path params flow into the URL, the body string is sent verbatim (no
            // `serialise`), and the response decodes to an `HttpOutcome` —
            // `Rejected(detail)` when the router refused the input before the
            // handler, `Handled(httpResult)` when it ran. Emitted only for a route
            // with a `Wire`-eligible slot (a body or a path param); a bodyless,
            // path-param-less route (e.g. `GET /cart/size`) can carry no `Wire`
            // argument, so its raw driver would be dead code.
            if !h.params.is_empty() {
                let raw_body_init = match body_ps.first() {
                    Some(p) => format!("body: {}, ", crate::emitter::ts_ident(&p.name.name)),
                    None => String::new(),
                };
                // The `Wire` text is text, so a raw route is sent it as `text/plain`:
                // a route declaring another media type answers `415`, which is how a
                // case probes it, and a `"*/*"` route still takes the text.
                let raw_content_type = if raw_type.is_some() {
                    "\"content-type\": \"text/plain; charset=utf-8\", ".to_string()
                } else {
                    content_type.clone()
                };
                let raw_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {headers_spread}{raw_content_type}{auth_header}{sig_header}}}, {raw_body_init}}}",
                    method = method.as_str(),
                ));
                routes.push(sysdrive_driver(
                    "raw_",
                    sname,
                    &key,
                    raw_params.clone(),
                    None,
                    url.clone(),
                    raw_options,
                    &binding,
                    "__responseToHttpOutcome",
                    &payload_deser,
                    signs_claims,
                    signing_stmts(&lone_sig, &sum, &raw_body),
                    sum.is_some(),
                ));
            }
            // #706: the no-auth driver for a `by Nobody` call — the same request
            // the typed driver builds, minus the `Authorization` header, so the
            // real auth seam rejects it. A `401` decodes to `Rejected(
            // Unauthorized)` (`responseToUnauthOutcome`); anything else decodes
            // normally. Emitted only for a Bearer-secured route (one that carries
            // an auth header) — an unsecured route has no seam to reject a missing
            // credential, so a `by Nobody` there is meaningless.
            let secured = !auth_header.is_empty() || sig.is_some() || sum.is_some();
            if secured {
                let noauth_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}}}, {body_init}}}",
                    method = method.as_str(),
                ));
                routes.push(sysdrive_driver(
                    "noauth_",
                    sname,
                    &key,
                    driver_params.clone(),
                    body_stmt.clone(),
                    url.clone(),
                    noauth_options,
                    &binding,
                    "__responseToUnauthOutcome",
                    &payload_deser,
                    false,
                    Vec::new(),
                    false,
                ));
            }
            // #821: the raw *and* no-auth driver combined, for a call mixing
            // `Wire(…)` with `by Nobody` — every slot is a raw `string` (as
            // `__sysdrive_raw`) and the `Authorization` header is dropped (as
            // `__sysdrive_noauth`), so the seam rejects the missing credential
            // before the (possibly malformed) raw body is even read.
            // `responseToUnauthOutcome` already delegates a non-`401` status to
            // `responseToHttpOutcome`'s shape-based classification, so a raw
            // body that would have been rejected on its own shape still decodes
            // correctly here. Emitted under the same conditions as both parent
            // drivers: a `Wire`-eligible slot and a Bearer-secured route.
            if !h.params.is_empty() && secured {
                let raw_body_init = match body_ps.first() {
                    Some(p) => format!("body: {}, ", crate::emitter::ts_ident(&p.name.name)),
                    None => String::new(),
                };
                let rawnoauth_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}}}, {raw_body_init}}}",
                    method = method.as_str(),
                ));
                routes.push(sysdrive_driver(
                    "rawnoauth_",
                    sname,
                    &key,
                    raw_params.clone(),
                    None,
                    url.clone(),
                    rawnoauth_options,
                    &binding,
                    "__responseToUnauthOutcome",
                    &payload_deser,
                    false,
                    Vec::new(),
                    false,
                ));
            }
            // #2135: `by Forged` (signed with a wrong secret) and `by Stale`
            // (signed `tolerance + 60` seconds in the past), typed and raw,
            // each decoding a `401` to `Rejected(Unauthorized)`. `Stale` only
            // where the actor declares a timestamp and a tolerance.
            if let Some(z) = sig {
                let forged_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}{bad_sig_header}}}, {body_init}}}",
                    method = method.as_str(),
                ));
                let mut variants = vec![("forged", SigVariant::Forged)];
                if let Some(age) = z.stale_age() {
                    variants.push(("stale", SigVariant::Stale(age)));
                }
                for (name, variant) in variants {
                    routes.push(sysdrive_driver(
                        &format!("{name}_"),
                        sname,
                        &key,
                        driver_params.clone(),
                        body_stmt.clone(),
                        url.clone(),
                        forged_options.clone(),
                        &binding,
                        "__responseToUnauthOutcome",
                        &payload_deser,
                        false,
                        vec![z.sign(typed_body, variant)],
                        false,
                    ));
                    if !h.params.is_empty() {
                        let raw_body_init = match body_ps.first() {
                            Some(p) => {
                                format!("body: {}, ", crate::emitter::ts_ident(&p.name.name))
                            }
                            None => String::new(),
                        };
                        let options = ident(format!(
                            "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}{bad_sig_header}}}, {raw_body_init}}}",
                            method = method.as_str(),
                        ));
                        routes.push(sysdrive_driver(
                            &format!("raw{name}_"),
                            sname,
                            &key,
                            raw_params.clone(),
                            None,
                            url.clone(),
                            options,
                            &binding,
                            "__responseToUnauthOutcome",
                            &payload_deser,
                            false,
                            vec![z.sign(&raw_body, variant)],
                            false,
                        ));
                    }
                }
            }
        }
        // #707: one generic wrong-method driver per service — drives an arbitrary
        // `(method, path)` (an existing path, an undeclared method) and decodes
        // the router's `405` fall-through to `Rejected(MethodNotAllowed)`. The
        // handler never runs, so there is no body to serialise and the payload
        // deserialiser is unused (the `405` takes the `Rejected` arm). Unlike the
        // per-route drivers' own options object (Decision A above), `{ method }`
        // is a fixed single shorthand entry — never conditionally empty — so it
        // matches `TsExpr::Object`'s own real single-line rendering exactly and
        // is built as a real node, not opaque text.
        let wrongmethod = TsStmt::decl(
            TsDecl::Function {
                name: format!("__sysdrive_wrongmethod_{sname}"),
                generics: Vec::new(),
                params: vec![
                    TsParam {
                        name: "method".to_string(),
                        ty: Some(TsType::named("string")),
                        optional: false,
                    },
                    TsParam {
                        name: "path".to_string(),
                        ty: Some(TsType::named("string")),
                        optional: false,
                    },
                ],
                return_type: None,
                body: vec![
                    TsStmt::const_stmt(
                        TsBindingName::Ident("__h".to_string()),
                        None,
                        call(ident("__makeHarness"), vec![]),
                        None,
                    ),
                    TsStmt::const_stmt(
                        TsBindingName::Ident("__req".to_string()),
                        None,
                        TsExpr::New {
                            callee: Box::new(ident("globalThis.Request")),
                            args: vec![
                                TsExpr::template_lit(
                                    vec!["https://test".to_string(), String::new()],
                                    vec![ident("path")],
                                ),
                                TsExpr::object_entries(vec![TsObjectEntry::Shorthand(
                                    "method".to_string(),
                                )]),
                            ],
                        },
                        None,
                    ),
                    TsStmt::const_stmt(
                        TsBindingName::Ident("__res".to_string()),
                        None,
                        await_expr(method_call(
                            member(member(ident("__h"), "env"), &binding),
                            "fetch",
                            vec![ident("__req")],
                        )),
                        None,
                    ),
                    TsStmt::return_stmt(
                        Some(call(
                            ident("__responseToHttpOutcome"),
                            vec![
                                ident("__res"),
                                TsExpr::Arrow {
                                    params: vec![TsParam {
                                        name: "__j".to_string(),
                                        ty: Some(TsType::named("__JsonValue")),
                                        optional: false,
                                    }],
                                    is_async: false,
                                    generics: Vec::new(),
                                    return_type: None,
                                    body: Box::new(TsArrowBody::Expr(Box::new(call(
                                        ident("Ok"),
                                        vec![TsExpr::As {
                                            expr: Box::new(ident("__j")),
                                            ty: TsType::named("never"),
                                        }],
                                    )))),
                                },
                            ],
                        )),
                        None,
                    ),
                ],
                is_async: true,
                inline: false,
            },
            None,
        );
        routes.push(wrongmethod);
    }

    if http_services.is_empty() {
        return SystemHttpSupport {
            code: Vec::new(),
            http_services,
            declared_routes: declared,
            route_body,
            type_ns: String::new(),
            claims: emitter::SystemHttpClaims::default(),
        };
    }

    // #1485: the HS256 signer block — a monotonic clock the signer's `exp`
    // uses (kept out of `bundle`d runtime), plus the test-only HS256 signer
    // itself (never in the deployable app; e2e owns real auth) — now lives
    // at `emitter/test_runtime/jwt_signer.ts`, the same committed-`.ts`-file
    // treatment `expectation_runtime_helpers`/`stub_runtime_helpers` already
    // get, rather than an unexplained raw string literal with `\`-continuations.
    let mut code = vec![TsStmt::raw(jwt_signer_runtime_helpers(), None)];
    if signs_hmac {
        let mut signer = TsStmt::raw(
            include_str!("../emitter/test_runtime/hmac_signer.ts").to_string(),
            None,
        );
        signer.no_blank_before = true;
        code.push(signer);
    }
    // Set each secret the target's actors read, so the real Bearer seam verifies.
    // P7.2: `Record<string, string>` — same reasoning as `__bynkSignHs256`'s own
    // `secret: string` parameter in `emitter/test_runtime/jwt_signer.ts`.
    let record_string_string = TsType::named_with_args(
        "globalThis.Record",
        vec![TsType::named("string"), TsType::named("string")],
    );
    let cast_globalthis_process = |process_optional: bool| -> TsExpr {
        let env_ty = TsType::Object(vec![TsTypeMember::prop(
            "env",
            record_string_string.clone(),
        )]);
        let process_member = if process_optional {
            TsTypeMember::optional_prop("process", env_ty)
        } else {
            TsTypeMember::prop("process", env_ty)
        };
        TsExpr::As {
            expr: Box::new(TsExpr::As {
                expr: Box::new(ident("globalThis")),
                ty: TsType::named("unknown"),
            }),
            ty: TsType::Object(vec![process_member]),
        }
    };
    for s in secrets.iter().filter(|_| !remote) {
        let assign_process = TsStmt::assign(
            member(cast_globalthis_process(false), "process"),
            TsExpr::Binary {
                op: TsBinaryOp::NullishCoalescing,
                left: Box::new(member(cast_globalthis_process(true), "process")),
                right: Box::new(TsExpr::object(vec![(
                    "env".to_string(),
                    TsExpr::object(vec![]),
                )])),
            },
            None,
        );
        let assign_secret = TsStmt::assign(
            TsExpr::Index {
                object: Box::new(member(
                    member(cast_globalthis_process(false), "process"),
                    "env",
                )),
                index: Box::new(str_lit(s.clone())),
            },
            str_lit("__bynk_test_secret"),
            None,
        );
        code.push(assign_process);
        code.push(assign_secret);
    }
    code.extend(routes);
    SystemHttpSupport {
        code,
        http_services,
        declared_routes: declared,
        route_body,
        type_ns,
        claims,
    }
}

/// #2136: the cron and queue drivers of a `system` suite's target, and the
/// services they cover ([`emitter::SystemJobServices`]).
struct SystemJobSupport {
    code: Vec<TsStmt>,
    services: emitter::SystemJobServices,
}

/// #2136: one driver per cron handler and per queue service of the `system`
/// target, calling the participant Worker's real entry point with a
/// platform-shaped event and the harness's execution-context double, which it
/// settles (ADR 0478). The runtime half lives at `emitter/test_runtime/jobs.ts`.
///
/// - `__sysdrive_schedule_<svc>_<index>(at?)` calls `scheduled({ cron,
///   scheduledTime }, env, ctx)` and yields the `Result` the test build's
///   entry stored on `ctx` (`scheduled_result_hook`). A handler without the
///   scheduled-time parameter is fired at `0`. A handler that throws fails the
///   case, as it would at `unit`.
/// - `__sysdrive_message_<q>(m)` serialises `m` as the producer does, sends it
///   through a JSON round trip, and calls `queue(batch, env)` with a
///   one-message batch whose `ack()` / `retry()` the double records, yielding
///   `Ack` or `Retry(reason)` (the reason the entry logs).
fn emit_system_job_support(
    target: &str,
    unit_tables: &HashMap<String, UnitTable>,
    runtime_use: &RuntimeUse,
    target_sends: bool,
) -> SystemJobSupport {
    use bynk_syntax::ast::{HandlerKind, ServiceProtocol};
    let mut services = emitter::SystemJobServices::default();
    let mut drivers: Vec<TsStmt> = Vec::new();
    let Some(table) = unit_tables.get(target) else {
        return SystemJobSupport {
            code: drivers,
            services,
        };
    };
    let ns = test_ns(target);
    let worker = format!("worker_{ns}");
    let env = format!("__h.env_{ns}");
    let type_ns = format!("{ns}.");
    let mut svc_names: Vec<&String> = table.services.keys().collect();
    svc_names.sort();
    for sname in svc_names {
        let decl = &table.services[sname];
        match &decl.protocol {
            ServiceProtocol::Cron => {
                let mut exprs = Vec::new();
                for h in &decl.handlers {
                    let HandlerKind::Cron { expr } = &h.kind else {
                        continue;
                    };
                    let idx = exprs.len();
                    let key = crate::emitter::cron_handler_method_name(sname, idx);
                    let (params, at) = match h.params.first() {
                        Some(p) => (
                            format!("at: {}", driver_param_ty(&p.type_ref, &ns)),
                            "at".to_string(),
                        ),
                        None => (String::new(), "0".to_string()),
                    };
                    drivers.push(TsStmt::raw(
                        format!(
                            "async function __sysdrive_schedule_{sname}_{idx}({params}) {{\n  const __h = __makeHarness();\n  return __bynkScheduled<Awaited<ReturnType<typeof {ns}.{sname}.{key}>>>({expr:?}, (exec) => {worker}.scheduled({{ cron: {expr:?}, scheduledTime: {at} }}, {env}, exec));\n}}\n"
                        ),
                        None,
                    ));
                    exprs.push(expr.clone());
                }
                services.cron.insert(sname.clone(), exprs);
            }
            ServiceProtocol::Queue { name } => {
                let Some(h) = decl
                    .handlers
                    .iter()
                    .find(|h| matches!(h.kind, HandlerKind::Message))
                else {
                    continue;
                };
                let Some(p) = h.params.first() else {
                    continue;
                };
                let key = crate::emitter::queue_handler_method_name(sname, 0);
                let param = crate::emitter::ts_ident(&p.name.name);
                let body = bynk_ts::print_expr(&crate::emitter::serialisation::serialise_expr_via(
                    &p.type_ref,
                    &param,
                    &type_ns,
                    runtime_use,
                ));
                let exec_arg = if target_sends { ", exec" } else { "" };
                drivers.push(TsStmt::raw(
                    format!(
                        "async function __sysdrive_message_{sname}({param}: {ty}) {{\n  const __h = __makeHarness();\n  return __bynkDeliver<Awaited<ReturnType<typeof {ns}.{sname}.{key}>>>({name:?}, {body}, (batch, exec) => {worker}.queue(batch, {env}{exec_arg}));\n}}\n",
                        ty = driver_param_ty(&p.type_ref, &ns),
                    ),
                    None,
                ));
                services.queue.insert(sname.clone());
            }
            _ => {}
        }
    }
    let mut code = Vec::new();
    if !drivers.is_empty() {
        code.push(TsStmt::raw(
            include_str!("../emitter/test_runtime/jobs.ts").to_string(),
            None,
        ));
        code.extend(drivers);
    }
    SystemJobSupport { code, services }
}

/// Type a system-http driver parameter (v0.182): a named type is reached
/// through the target's handler namespace (`todos.AddRequest`); primitives and
/// compound shapes fall back to `unknown` — the driver only forwards the value
/// to the namespace's `serialise_*`, and the Bynk checker already typed the call.
fn driver_param_ty(t: &bynk_syntax::ast::TypeRef, _ns: &str) -> String {
    use bynk_syntax::ast::TypeRef;
    match t {
        // A named type may embed a branded refined field (`AddRequest.title:
        // Title`), and the case body passes a plain object literal — the Bynk
        // checker already type-checked the call's args against the handler
        // (Slice A), so the driver forwards the value to the namespace's
        // `serialise_*`. Primitives stay precise.
        TypeRef::Named(_) => "any".to_string(),
        other => crate::emitter::ts_type_ref(other),
    }
}

/// Extract `T` from a handler return type `Effect[HttpResult[T]]` (v0.182).
fn strip_effect_httpresult(t: &bynk_syntax::ast::TypeRef) -> Option<&bynk_syntax::ast::TypeRef> {
    use bynk_syntax::ast::TypeRef;
    let inner = match t {
        TypeRef::Effect(b, _) => b.as_ref(),
        other => other,
    };
    match inner {
        TypeRef::HttpResult(payload, _) => Some(payload.as_ref()),
        _ => None,
    }
}

/// A `{ fetch: (req: Request) => worker_X.fetch(req, env_X) } as ServiceBinding`
/// expression — the identical shape both the per-participant and root-env
/// binding wiring in [`emit_integration_harness`] build, differing only in
/// which worker/env pair they close over.
fn service_binding_forward(worker_ident: &str, env_ident: &str, sends: bool) -> TsExpr {
    let req_param = TsParam {
        name: "req".to_string(),
        ty: Some(TsType::named("globalThis.Request")),
        optional: false,
    };
    // #1994: a Worker whose handlers send with `~>` takes the execution
    // context as `fetch`'s third argument; the harness hands it a collecting
    // double and settles the sends before the response reaches the case.
    let forward = if sends {
        call(
            ident("__bynkFetchSettled"),
            vec![TsExpr::Arrow {
                params: vec![TsParam {
                    name: "exec".to_string(),
                    ty: None,
                    optional: false,
                }],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(method_call(
                    ident(worker_ident),
                    "fetch",
                    vec![ident("req"), ident(env_ident), ident("exec")],
                )))),
            }],
        )
    } else {
        method_call(
            ident(worker_ident),
            "fetch",
            vec![ident("req"), ident(env_ident)],
        )
    };
    TsExpr::As {
        expr: Box::new(TsExpr::object(vec![(
            "fetch".to_string(),
            TsExpr::Arrow {
                params: vec![req_param],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(forward))),
            },
        )])),
        ty: TsType::named("__ServiceBinding"),
    }
}

/// Interop I5 (#1972): a Service Binding standing in for a foreign Worker in
/// the integration harness, answering every request with
/// [`crate::emitter::wrangler::foreign_service_double_body`]'s 503, as the
/// double `bynk dev` binds by default does. A case that needs another answer
/// stubs the instance's `send`.
fn foreign_service_double(binding: &str, worker: &str) -> TsExpr {
    let response = TsExpr::New {
        callee: Box::new(ident("globalThis.Response")),
        args: vec![
            str_lit(crate::emitter::wrangler::foreign_service_double_body(
                binding, worker,
            )),
            TsExpr::object(vec![(
                "status".to_string(),
                num_lit(crate::emitter::wrangler::FOREIGN_SERVICE_DOUBLE_STATUS.to_string()),
            )]),
        ],
    };
    TsExpr::As {
        expr: Box::new(TsExpr::object(vec![(
            "fetch".to_string(),
            TsExpr::Arrow {
                params: Vec::new(),
                is_async: true,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(response))),
            },
        )])),
        ty: TsType::named("__ServiceBinding"),
    }
}

/// #1959: the harness's import alias for participant `ns`'s events fan-out
/// module, shared by the import and the binding that constructs from it.
fn fanout_alias(ns: &str) -> String {
    format!("fanout_{ns}")
}

/// The name a system test case calls a participant as: its cases lower in a
/// commons of this name ([`integration_typed_commons`]), and a cross-context
/// call stamps the calling commons' name. I2 (#1971): the harness gives every
/// participant the local key for the edge from it.
const HARNESS_CALLER: &str = "integration";

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
fn emit_integration_harness<'a>(
    participants: &[String],
    bindings_of: &dyn Fn(&String) -> &'a Vec<WorkerBinding>,
    sends: &dyn Fn(&String) -> bool,
    foreign_secrets: &dyn Fn(&String) -> Vec<String>,
    exposed: &[String],
    // #2134: put the running case's `env` entries (`__bynkEnv`) on each
    // participant's `env`, first, so the harness's own keys and bindings
    // below are never replaced by one.
    with_env: bool,
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
                str_lit(crate::LOCAL_CALL_KEY),
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
                // P4a (#2602): each instance its own empty in-memory database.
                BindingSource::D1Database { .. } => call(ident("__makeIntegrationD1"), vec![]),
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
            str_lit(crate::LOCAL_CALL_KEY),
            None,
        ));
    }
    for p in participants {
        let ns = test_ns(p);
        let binding = crate::emitter::wrangler::service_binding(p).name;
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
fn emit_remote_harness(participants: &[String]) -> Vec<TsStmt> {
    let mut text = String::from(
        "function __buildHarness() {\n  const rootEnv: { [binding: string]: __ServiceBinding } = {};\n",
    );
    for p in participants {
        let binding = crate::emitter::wrangler::service_binding(p).name;
        let worker = worker_dir_name(p);
        text.push_str(&format!(
            "  rootEnv.{binding} = __bynkRemoteBinding({worker:?});\n"
        ));
    }
    text.push_str("  return { env: rootEnv };\n}\n");
    text.push_str("let __bynkHarness: ReturnType<typeof __buildHarness> | undefined;\nfunction __makeHarness() {\n  return (__bynkHarness ??= __buildHarness());\n}\n");
    vec![
        TsStmt::raw(
            include_str!("../emitter/test_runtime/remote.ts").to_string(),
            None,
        ),
        // #2138 (track §3.6): the `--settle` retry the case bodies use.
        TsStmt::raw(
            include_str!("../emitter/test_runtime/settle.ts").to_string(),
            None,
        ),
        TsStmt::raw(text, None),
    ]
}

/// Build the [`checker::TypedCommons`] used to lower integration case bodies —
/// the harness view ([`test_suites::integration_view`]: `uses` commons plus
/// participant types/fns/methods, closed over the types they reach, #1814),
/// so static calls and constructors resolve.
fn integration_typed_commons(
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
        commons: Commons {
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
        },
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
        shadowed_imports: HashMap::new(),
        extensible_enums: Default::default(),
        stability: Default::default(),
    }
}

fn sanitise_suite(s: &str) -> String {
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
    target_name: String,
    /// The module's output path relative to the project root.
    module_path: PathBuf,
    /// v0.67: `"unit"` or `"integration"` — the suite kind for discovery, mirrors
    /// the runner's `suite-begin` `kind`.
    kind: &'static str,
    /// v0.67: the JSON suite name for discovery — the joined target name (unit)
    /// or the bare suite name (integration). Differs from `target_name` only for
    /// integration, which the runner prefixes with `system · ` (#2185).
    suite_name: String,
    /// #2138: the cases a remote build skipped (`--skip-unsupported`), and why.
    skipped: Vec<crate::project::RemoteSkip>,
    /// #2138: under `--against dev`, the distinct `env` maps of its cases.
    envs: Vec<BTreeMap<String, String>>,
    /// #2138: a remote build's module, whose cases report `retries`.
    remote: bool,
    /// The module inlines the generative-property runtime (it declares a
    /// `property` or attacks a contract), so its failed cases report `kind`,
    /// `seed` and `counterexample`.
    generative: bool,
    /// v0.67: the suite's cases, in declaration (emission) order, retained for
    /// `--no-run` discovery.
    cases: Vec<DiscoveredCase>,
}

/// v0.67: the `path:line:col` of a test-name literal, structured for discovery.
/// Reuses [`bynk_syntax::span::line_col`] and the same forward-slash
/// normalisation `assert_location` applies (bynk-emit/src/emitter/lower.rs), so a
/// discovered case and a run-failure resolve to consistent coordinates.
fn discovered_location(source: &str, rel_path: &str, span: Span) -> TestLocation {
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
fn block_uses_observation(block: &Block) -> bool {
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
        bynk_ir::walk_exprs(e, &mut check);
    }
    bynk_ir::walk_exprs(&block.tail, &mut check);
    found
}

/// v0.178 (#662): build the target's service table for a test-body check —
/// each service's protocol word and its `on call` handler signature, so a
/// `svc.call(args)` in a case can be resolved rather than string-matched. A
/// service with no `on call` handler carries `None`, which turns `svc.call(...)`
/// into `bynk.test.service_no_call_handler` instead of a silent runtime crash.
fn target_service_handler_kinds(
    table: Option<&UnitTable>,
) -> HashMap<String, Vec<bynk_ir::IrHandlerKind>> {
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
                    .map(|h| bynk_lower::lower_handler_kind_ir(&h.kind))
                    .collect(),
            )
        })
        .collect()
}

/// I3 (#1971), #2020: the target's handlers that bind an actor sum, whose
/// `deps.who` a call-site principal builds: an `on call` sum (`by who: Caller
/// | Legacy`), keyed `(service, "call")`, and an http sum (`by who: User |
/// Visitor`), keyed `(service, <emitted method name>)`. Mirrors the order
/// `bynk_lower::lower_actor_seam_ir` resolves a handler's seam in.
fn target_sum_handlers(table: Option<&UnitTable>) -> HashSet<(String, String)> {
    let Some(t) = table else {
        return HashSet::new();
    };
    let mut out = HashSet::new();
    for (name, decl) in &t.services {
        for h in &decl.handlers {
            let key = match bynk_lower::lower_handler_kind_ir(&h.kind) {
                bynk_ir::IrHandlerKind::Call
                    if bynk_check::actors::call_admission_for(h, &t.actors)
                        .is_some_and(|a| a.sum.is_some()) =>
                {
                    "call".to_string()
                }
                bynk_ir::IrHandlerKind::Http { method, path }
                    if bynk_check::actors::sum_members_for(h, &t.actors).is_some() =>
                {
                    crate::emitter::http_handler_method_name_ir(method, &path)
                }
                _ => continue,
            };
            out.insert((name.clone(), key));
        }
        // #2380 (L5): an `Oidc` route that binds its actor, recorded under an
        // `oidc:` key, so an in-process call gives it `deps.claims`.
        for h in &decl.handlers {
            if let bynk_ir::IrHandlerKind::Http { method, path } =
                bynk_lower::lower_handler_kind_ir(&h.kind)
                && bynk_check::actors::oidc_seam_for(h, &t.actors)
                    .is_some_and(|s| s.binder.is_some())
            {
                out.insert((
                    name.clone(),
                    format!(
                        "oidc:{}",
                        crate::emitter::http_handler_method_name_ir(method, &path)
                    ),
                ));
            }
        }
    }
    out
}

/// v0.119: `Some((run_var, agent_name))` when `prop` is a history property — its
/// single `for all` binding is `run: History[Agent]`.
fn prop_history_binding(prop: &PropertyDecl) -> Option<(&str, &str)> {
    prop.forall.bindings.iter().find_map(|b| match &b.type_ref {
        TypeRef::History(inner, _) => match &**inner {
            TypeRef::Named(id) => Some((b.name.name.as_str(), id.name.as_str())),
            _ => None,
        },
        _ => None,
    })
}

fn prop_is_history(prop: &PropertyDecl) -> bool {
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
fn is_attackable_contract(f: &FnDecl, resolved: &ResolvedCommons, tys: &Arc<Types>) -> bool {
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
fn numeric_or_scalar_base(
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
fn attackable_contracts(
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
fn json_codec_qual_for_target(
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
) -> HashMap<String, String> {
    let mut qual = HashMap::new();
    let mut qualify = |units: Option<&Vec<String>>| {
        for u in units.into_iter().flatten() {
            let ns = test_ns(u);
            if let Some(table) = unit_tables.get(u) {
                for n in table.types.keys() {
                    qual.entry(n.clone()).or_insert_with(|| format!("{ns}."));
                }
            }
        }
    };
    // Highest precedence first; `or_insert` keeps the first claim.
    qualify(Some(&vec![target_name.to_string()]));
    qualify(unit_consumes.get(target_name));
    qualify(unit_uses.get(target_name));
    qual
}

#[allow(clippy::too_many_arguments)]
fn emit_test_module(
    target_name: &str,
    target_kind: UnitKind,
    indices: &[usize],
    parsed: &[ParsedFile],
    stubs: &HashMap<String, ResolvedStub>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    exports_visibility: &HashMap<String, HashMap<String, Visibility>>,
    import_ext: ImportExt,
    contracts: bool,
    hidden: &BTreeMap<String, String>,
    // #1883, slice 3: `asset_harness`'s text, when the target ships assets.
    asset_harness: Option<String>,
    binding_modules: &HashMap<String, String>,
    schema_versions: &HashMap<String, i64>,
    // #2050: whether the target's handlers emit ([`target_uses_emit`]).
    uses_emit: bool,
    tys: &Arc<Types>,
) -> Option<(PathBuf, TsProgram, Option<String>, RunnableTest)> {
    // #914: the test-scaffold module's runtime import list is emitted below as a
    // fixed set, but the bodies spliced into it lower through the ordinary `Bytes`
    // and boundary-codec paths. What those reach for is recorded here as they emit
    // and injected into the import line as a post-pass, the same shape
    // `emit_project` and `emit_worker_entry` use.
    let runtime_use = RuntimeUse::default();
    runtime_use.set_json_codec_qual(json_codec_qual_for_target(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
    ));
    let _ = exports_visibility;
    let ext = import_ext.as_str();
    // #1486: `stmts` collects the module's own real top-level nodes instead of
    // `out: String` — printed once at the boundary via `bynk_ts::print_multi_
    // source`, which builds the source map itself (per-statement `nested_map`/
    // `nested_map_source_id` merges) rather than this function driving a
    // `SourceMapBuilder` by hand through `print_stmt_and_merge`.
    let mut stmts: Vec<TsStmt> = Vec::new();
    // v0.70: the test module's own aggregate map. A test group can aggregate
    // cases from several `.bynk` files, so it is multi-source; each case's body
    // is registered under its own source (on first sight, in the case loop
    // below) and merged there via that case's own top-level
    // `nested_map_source_id`.
    let mut map = SourceMapBuilder::new();
    let target_ns = test_ns(target_name);
    let target_dir = commons_dir_for(target_name);
    // Output file: tests/<sanitised-target>.test.ts
    let module_path = PathBuf::from(format!("tests/{}.test.ts", target_name.replace('.', "_")));

    stmts.push(TsStmt::comment(
        "Generated by bynkc — do not edit by hand.",
        None,
    ));
    stmts.push(TsStmt::comment(format!("test target: {target_name}"), None));

    // Result/Option helpers — same shape as the production runtime imports.
    // The test module lives at `tests/<file>.test.ts`, so the runtime is one
    // directory up. Compute through the same depth machinery used by the
    // per-context emitter. If the target context declares agents, also pull
    // in `makeTestState` so agent invocations can synthesise DO state.
    // #1486: this import statement's own final `names` depend on
    // `runtime_use`, which the case/property/attack lowering loops below
    // still populate as they run — its real construction is deferred to the
    // end of this function (the same reordering `emit_project`'s own
    // `write_header` already does) and spliced in at this fixed position
    // (index 2, right after the two header comments above) once everything
    // has lowered.
    let has_agents = unit_tables
        .get(target_name)
        .map(|t| !t.agents.is_empty())
        .unwrap_or(false);
    let runtime_import = emitter::runtime_import_for(&module_path, import_ext);
    let mut runtime_names: Vec<String> = ["Ok", "Err", "Some", "None"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if has_agents {
        runtime_names.push("__makeTestState".to_string());
    }
    runtime_names.push("type Result".to_string());
    runtime_names.push("type Option".to_string());
    runtime_names.push("type ValidationError".to_string());
    // #1883, slice 3: the asset fetch helper answers with an `HttpResult`.
    if asset_harness.is_some() {
        runtime_names.push("HttpResult".to_string());
    }
    let import_idx = stmts.len();

    // Compute relative import path from tests/ to the target's output dir.
    let import_target = relative_import_for_test(&target_dir);
    stmts.push(TsStmt::decl(
        TsDecl::ImportNamespace {
            type_only: false,
            alias: target_ns.clone(),
            from: format!("./{import_target}.{ext}"),
        },
        None,
    ));

    // The deps factory, built here for the units its providers name (#1863),
    // which are imported with the consumed contexts below.
    let uses_send = target_uses_send(target_name, target_kind, unit_tables);
    // #1975: an adapter suite tests its binding, so its factory builds the
    // binding for real; a context's `integration` cases get a second factory
    // that builds each consumed adapter's binding, when that differs.
    let deps = emit_test_deps(
        target_name,
        target_kind,
        stubs,
        unit_tables,
        unit_consumes,
        unit_consumes_aliases,
        unit_flattened,
        (target_kind == UnitKind::Adapter).then_some(binding_modules),
        "__makeTestDeps",
        uses_emit,
    );
    let has_integration_case = indices.iter().any(|&i| {
        parsed[i].test().is_some_and(|t| {
            t.cases
                .iter()
                .any(|c| super::case_effective_tier(c, t) != bynk_syntax::ast::TestTier::Unit)
        })
    });
    let integration_factory = |name: &str| {
        emit_test_deps(
            target_name,
            target_kind,
            stubs,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            Some(binding_modules),
            name,
            uses_emit,
        )
    };
    let integration_deps = (target_kind == UnitKind::Context
        && has_integration_case
        && bynk_ts::print_stmt(&integration_factory("__makeTestDeps").stmt, 0)
            != bynk_ts::print_stmt(&deps.stmt, 0))
    .then(|| integration_factory("__makeIntegrationDeps"));
    let deps_units: BTreeSet<&String> = deps
        .units
        .iter()
        .chain(integration_deps.iter().flat_map(|d| d.units.iter()))
        .collect();

    // Consumed contexts (for the target context, if any), and any other unit
    // a provider in the deps factory names.
    let mut consumed_imports: Vec<(String, String)> = Vec::new();
    let consumed = unit_consumes.get(target_name).into_iter().flatten();
    for q in consumed.chain(deps_units.iter().copied()) {
        if q == target_name {
            continue;
        }
        let ns = test_ns(q);
        let dir = commons_dir_for(q);
        let import_path = relative_import_for_test(&dir);
        consumed_imports.push((ns, import_path));
    }
    consumed_imports.sort();
    consumed_imports.dedup();
    for (ns, path) in &consumed_imports {
        stmts.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: ns.clone(),
                from: format!("./{path}.{ext}"),
            },
            None,
        ));
    }
    // #1975: the binding module of each adapter a factory builds for real,
    // under the alias the composition root gives it. K4: and `bynk.crypto`'s,
    // whose test double is its own binding ([`platform_double`]). P4a
    // (#2602): and `bynk.cloudflare`'s for a `D1` double, which is the real
    // `WorkersD1` over an in-memory database.
    let all_doubles = || {
        deps.doubles
            .iter()
            .chain(integration_deps.iter().flat_map(|d| d.doubles.iter()))
    };
    let d1_double = |owner: &String, cap: &String| {
        owner == bynk_check::firstparty::CLOUDFLARE_UNIT && cap == "D1"
    };
    let mut binding_units: BTreeSet<&String> = deps_units.clone();
    binding_units.extend(
        all_doubles()
            .filter(|(owner, cap, _)| {
                owner == bynk_check::firstparty::CRYPTO_UNIT || d1_double(owner, cap)
            })
            .map(|(owner, _, _)| owner),
    );
    if all_doubles().any(|(owner, cap, _)| d1_double(owner, cap)) {
        runtime_names.push("__makeIntegrationD1".to_string());
    }
    for unit in &binding_units {
        // P4a (#2602): `bynk.cloudflare`'s binding is first-party, so it is
        // never in `binding_modules` (a test keeps its doubles); the `D1`
        // double imports it from its fixed path.
        let first_party = (unit.as_str() == bynk_check::firstparty::CLOUDFLARE_UNIT).then(|| {
            bynk_check::firstparty::CLOUDFLARE_BINDING_FILENAME
                .trim_end_matches(".ts")
                .to_string()
        });
        if let Some(module) = binding_modules.get(*unit).or(first_party.as_ref()) {
            stmts.push(TsStmt::decl(
                TsDecl::ImportNamespace {
                    type_only: false,
                    alias: format!("{}__binding", unit.replace('.', "_")),
                    from: format!("../{module}.{ext}"),
                },
                None,
            ));
        }
    }

    // `uses` commons reachable from the test fragments — needed for `Money`,
    // etc., used inside test bodies. We pull from the target context's uses.
    let mut uses_imports: Vec<(String, String)> = Vec::new();
    if let Some(used) = unit_uses.get(target_name) {
        for u in used {
            let ns = test_ns(u);
            let dir = commons_dir_for(u);
            let import_path = relative_import_for_test(&dir);
            uses_imports.push((ns, import_path));
        }
    }
    // #1814: and the commons owning each type the target's view reaches but
    // does not import, which a lowered case spells (a kernel's
    // `(__x: Repo) => …`, `Repo.shout(…)`); `emit_test_scope_setup` aliases
    // them.
    for owner in hidden_by_owner(hidden).keys() {
        let ns = test_ns(owner);
        if !consumed_imports.iter().any(|(n, _)| *n == ns) {
            uses_imports.push((ns, relative_import_for_test(&commons_dir_for(owner))));
        }
    }
    uses_imports.sort();
    uses_imports.dedup();
    for (ns, path) in &uses_imports {
        stmts.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: ns.clone(),
                from: format!("./{path}.{ext}"),
            },
            None,
        ));
    }

    // Expectation helper used by lowered `expect` statements — committed `.ts`
    // source (Decision C), carried as one `Raw` statement. Its own text
    // already ends in a blank line, so whichever real statement comes right
    // after it (below) suppresses the printer's own automatic lead-in blank
    // (`no_blank_before`) rather than stacking a second one on top —
    // `suppress_next_blank` tracks that until the first candidate actually
    // fires (property helpers, history helpers, observation helpers, stub
    // helpers, or — if this module has none of those — the deps factory,
    // always last in that chain and so the guaranteed fallback).
    stmts.push(TsStmt::raw(expectation_runtime_helpers(), None));
    // #1883, slice 3: the embedded files and `__bynkAssetGet`, for an asset
    // fetch in a case.
    if let Some(text) = &asset_harness {
        stmts.push(TsStmt::raw(text.clone(), None));
    }
    // #1994: a target whose handlers use `~>` gets the execution-context
    // double its deps carry as `__exec`, and the settle helper that awaits
    // the collected `waitUntil` promises after each addressed call.
    if uses_send {
        stmts.push(TsStmt::raw(send_runtime_helpers(), None));
    }
    // #2137: a module whose cases `advance Clock` gets the harness clock, and
    // its `Clock` double reads it (fresh per case, from the epoch).
    let uses_advance = indices.iter().any(|&i| {
        parsed[i]
            .test()
            .is_some_and(|t| t.cases.iter().any(|c| block_uses_advance(&c.body)))
    });
    // #2138: `run.id`, for a module whose cases read it.
    if indices.iter().any(|&i| {
        parsed[i]
            .test()
            .is_some_and(|t| t.cases.iter().any(|c| block_uses_run_id(&c.body)))
    }) {
        stmts.push(TsStmt::raw(run_id_helper(false), None));
    }
    if uses_advance {
        stmts.push(TsStmt::raw(clock_runtime_helpers(), None));
    }
    let mut suppress_next_blank = true;

    // v0.115: the target unit's contracted free functions the runner can attack
    // (each `ensures` is a generative check that is always on). Emitted only when
    // the contract guard is (the attack relies on the guard to assert `ensures`);
    // `bynkc test` sets both, `bynkc compile` neither.
    let (attack_resolved, attack_fns) = if contracts {
        attackable_contracts(
            target_name,
            unit_tables,
            unit_uses,
            unit_consumes,
            unit_consumes_aliases,
            tys,
        )
        .map(|(r, fns)| (Some(r), fns))
        .unwrap_or((None, Vec::new()))
    } else {
        (None, Vec::new())
    };

    // v0.114: the generative-property runtime — emitted only when this module
    // declares a `property` (or v0.115 attacks a contract), so modules with only
    // `case`s stay byte-for-byte unchanged.
    let has_properties = !attack_fns.is_empty()
        || indices
            .iter()
            .any(|&i| parsed[i].test().is_some_and(|t| !t.properties.is_empty()));
    if has_properties {
        let mut stmt = TsStmt::raw(property_runtime_helpers(), None);
        if suppress_next_blank {
            stmt.no_blank_before = true;
            suppress_next_blank = false;
        }
        stmts.push(stmt);
    }
    // v0.119: the history-property runtime — emitted only when a module declares a
    // `for all run: History[Agent]` property, so value-only property modules stay
    // byte-for-byte unchanged.
    let has_history_properties = indices.iter().any(|&i| {
        parsed[i]
            .test()
            .is_some_and(|t| t.properties.iter().any(prop_is_history))
    });
    if has_history_properties {
        let mut stmt = TsStmt::raw(history_runtime_helpers(), None);
        if suppress_next_blank {
            stmt.no_blank_before = true;
            suppress_next_blank = false;
        }
        stmts.push(stmt);
    }

    // v0.117: the observation runtime — emitted only when a `case` in this module
    // observes (`Cap.op called …` / `trace(Cap.op)`), so modules without
    // observation stay byte-for-byte unchanged.
    let uses_observation = indices.iter().any(|&i| {
        parsed[i]
            .test()
            .is_some_and(|t| t.cases.iter().any(|c| block_uses_observation(&c.body)))
    });
    if uses_observation {
        let record_types = observation_call_record_types(target_name, unit_tables);
        // Review of #1486, finding 2: pre-conversion, this whole section was
        // `out.push_str(observation_runtime_helpers()); out.push('\n');
        // extend_printed_at(&mut out, record_types); out.push('\n');` — the
        // *second* `push('\n')` ran unconditionally, so an empty
        // `record_types` (a `unit_tables` miss, or a target with no
        // capabilities/ops) still got two blank lines after the helpers, not
        // one. Baking the second blank directly into the helpers' own Raw
        // text when there is nothing to glue after it reproduces that;
        // otherwise the printer's automatic policy alone supplies the one
        // blank before the first record type, matching the old single
        // `push('\n')` between them exactly.
        let helpers_text = if record_types.is_empty() {
            format!("{}\n", observation_runtime_helpers())
        } else {
            observation_runtime_helpers()
        };
        let mut stmt = TsStmt::raw(helpers_text, None);
        if suppress_next_blank {
            stmt.no_blank_before = true;
            suppress_next_blank = false;
        }
        stmts.push(stmt);
        // The synthetic per-op call-record types a `trace(Cap.op)` result's
        // elements carry — so `trace(…).filter((c) => c.field …)` type-checks
        // against the operation's parameter names. Glued together (no blank
        // between them), the same shape `extend_printed_at` gave them
        // pre-conversion.
        for (i, mut rec) in record_types.into_iter().enumerate() {
            if i > 0 {
                rec.no_blank_before = true;
            }
            stmts.push(rec);
        }
    }

    // v0.118: emit one `__Stub_<Cap>` stub class per overridden capability
    // seam. Sorted by capability so emission is deterministic regardless of the
    // map's hash iteration order. #1652: argument patterns match with the
    // runtime's structural `__bynkEq` (the same equality as `==`), so the
    // separate `__bynkDeepEqual` helper this block used to inject is gone.
    let mut sorted_stubs: Vec<(&String, &ResolvedStub)> = stubs.iter().collect();
    sorted_stubs.sort_by(|a, b| a.0.cmp(b.0));
    for (_, rp) in sorted_stubs {
        let mut stmt = emit_stub_class(
            rp,
            target_name,
            unit_tables,
            unit_uses,
            unit_consumes,
            unit_consumes_aliases,
            &runtime_use,
            hidden,
            tys,
        );
        if suppress_next_blank {
            stmt.no_blank_before = true;
            suppress_next_blank = false;
        }
        stmts.push(stmt);
    }

    // #2134: when a case runs with `env` entries, the running case's map is
    // held in a module-level `__bynkEnv`, which the case runner sets and
    // clears, and the `Secrets` double answers from it.
    let with_env = indices
        .iter()
        .any(|&i| parsed[i].test().is_some_and(bynk_check::test_env::has_env));
    if with_env {
        let mut env_decl = TsStmt::raw(
            "// #2134: the running case's `env` entries, set by its runner.\nlet __bynkEnv: globalThis.Record<string, string> = {};\n",
            None,
        );
        env_decl.no_blank_before = true;
        stmts.push(env_decl);
    }

    // #291: the overlay `stub` layers over a seam's tier default, and the
    // deterministic test double of each platform capability the target
    // consumes.
    if !stubs.is_empty() {
        stmts.push(TsStmt::raw(STUB_OVERLAY_TS.to_string(), None));
    }
    // Both factories may name the same double; define each once. Keyed on the
    // whole `(owner, capability, name)`, so two owners' same-named doubles
    // both emit and collide loudly at `tsc` rather than one being dropped.
    let mut defined: BTreeSet<&(String, String, String)> = BTreeSet::new();
    for double in deps
        .doubles
        .iter()
        .chain(integration_deps.iter().flat_map(|d| d.doubles.iter()))
    {
        let (owner, cap, name) = double;
        if !defined.insert(double) {
            continue;
        }
        // #2137: in a module whose cases `advance`, the `Clock` double reads
        // the harness clock.
        let def = if with_env {
            env_secrets_double(owner, cap, &test_ns(owner), name)
                .or_else(|| platform_double(owner, cap, &test_ns(owner), name, uses_advance))
        } else {
            platform_double(owner, cap, &test_ns(owner), name, uses_advance)
        };
        if let Some(def) = def {
            stmts.push(TsStmt::raw(def, None));
        }
    }

    // Emit the deps factory, built above.
    let mut deps_stmt = deps.stmt.clone();
    if suppress_next_blank {
        deps_stmt.no_blank_before = true;
    }
    stmts.push(deps_stmt);
    if let Some(integration) = &integration_deps {
        let mut stmt = integration.stmt.clone();
        stmt.no_blank_before = true;
        stmts.push(stmt);
    }

    // #1860: each run's suite ordinal, for a suite-scoped `stub` — `None`
    // when no clause is one, so the runs pass none.
    let suite_scoped = stubs.values().any(stub_is_suite_scoped);
    let run_suite = |i: usize| {
        suite_scoped
            .then(|| test_suites::suite_ordinal(i, indices, parsed))
            .flatten()
    };

    // Emit one async function per test case. Capture each case's name + source
    // location for `--no-run` discovery as we go (same order the runner reports).
    let mut case_runners: Vec<String> = Vec::new();
    let mut discovered: Vec<DiscoveredCase> = Vec::new();
    for &i in indices {
        let Some(test_decl) = parsed[i].test() else {
            continue;
        };
        let rel_path = parsed[i].identity_path();
        let rel_path = rel_path.to_string_lossy();
        let suite = run_suite(i);
        for case in &test_decl.cases {
            discovered.push(DiscoveredCase {
                name: case.name.clone(),
                location: Some(discovered_location(
                    parsed[i].source(),
                    &rel_path,
                    case.name_span,
                )),
                tier: Some(super::case_effective_tier(case, test_decl).as_str()),
                generative: None,
                doc: false,
            });
            let runner_name = sanitise_case_name(&case.name, &mut case_runners.len());
            case_runners.push(runner_name.clone());
            // v0.118: record each case's effective tier (its own `as <tier>`, else
            // the suite default, else `unit`). unit and integration share this
            // in-process harness; the tier rides the emitted module for reporting.
            stmts.push(TsStmt::comment(
                format!(
                    "case tier: {}",
                    super::case_effective_tier(case, test_decl).as_str()
                ),
                None,
            ));
            let tier = super::case_effective_tier(case, test_decl);
            let deps_factory =
                // Every tier above `unit` runs real collaborators. (A `system`
                // case sits in a `system` suite, emitted through the Worker
                // harness, whose compose builds the binding itself.)
                if integration_deps.is_some() && tier != bynk_syntax::ast::TestTier::Unit {
                    "__makeIntegrationDeps"
                } else {
                    "__makeTestDeps"
                };
            let mut case_stmt = emit_test_case_function(
                &runner_name,
                case,
                with_env.then(|| bynk_check::test_env::case_env(test_decl, case)),
                deps_factory,
                target_name,
                target_kind,
                stubs,
                unit_tables,
                unit_uses,
                unit_consumes,
                unit_consumes_aliases,
                parsed[i].source(),
                &rel_path,
                &runtime_use,
                suite,
                hidden,
                tys,
            );
            // v0.70: merge this case's body checkpoints into the module map under
            // the case's `.bynk` source (a test group can span several files).
            // Forward slashes so the map's `sources` are portable (Windows joins
            // with `\`), matching the emitter's other specifier rendering.
            // v0.72: the map `source` is the file's absolute path (not the
            // project-relative `rel_path`, which a debugger would resolve against
            // the emitted `.ts`'s dir) so an editor breakpoint on the real
            // `.bynk` test file binds.
            let src_id =
                map.add_source(parsed[i].map_source_name(), parsed[i].source().to_string());
            case_stmt.nested_map_source_id = src_id;
            // The "case tier" comment right above is not a real blank-line
            // pair the automatic policy knows about (only two consecutive
            // `Comment`s are exempted) — glue them explicitly, as the pre-
            // conversion code's own lack of a blank between them did.
            case_stmt.no_blank_before = true;
            stmts.push(case_stmt);
        }
    }

    // v0.114: emit one async runner per generative `property`. Each property's
    // seed derives from the run's root seed via a stable ordinal, so a run
    // reproduces byte-for-byte under `--seed`.
    let mut prop_runners: Vec<String> = Vec::new();
    for &i in indices {
        let Some(test_decl) = parsed[i].test() else {
            continue;
        };
        let rel_path = parsed[i].identity_path();
        let rel_path = rel_path.to_string_lossy();
        let suite = run_suite(i);
        for prop in &test_decl.properties {
            discovered.push(DiscoveredCase {
                name: prop.name.clone(),
                location: Some(discovered_location(
                    parsed[i].source(),
                    &rel_path,
                    prop.name_span,
                )),
                tier: None,
                generative: Some(if prop_is_history(prop) {
                    "history"
                } else {
                    "property"
                }),
                doc: false,
            });
            let mut idx = prop_runners.len();
            let runner_name = format!("__prop_{}", sanitise_case_name(&prop.name, &mut idx));
            let prop_ordinal = prop_runners.len();
            prop_runners.push(runner_name.clone());
            // v0.119: a `for all run: History[Agent]` property routes to the driven-
            // sequence runner; a value property keeps the existing path.
            let prop_stmt = if prop_is_history(prop) {
                emit_test_history_property_function(
                    &runner_name,
                    prop,
                    prop_ordinal,
                    target_name,
                    target_kind,
                    unit_tables,
                    unit_uses,
                    unit_consumes,
                    unit_consumes_aliases,
                    parsed[i].source(),
                    &rel_path,
                    &runtime_use,
                    suite,
                    hidden,
                    tys,
                )
            } else {
                emit_test_property_function(
                    &runner_name,
                    prop,
                    prop_ordinal,
                    target_name,
                    target_kind,
                    unit_tables,
                    unit_uses,
                    unit_consumes,
                    unit_consumes_aliases,
                    parsed[i].source(),
                    &rel_path,
                    &runtime_use,
                    suite,
                    hidden,
                    tys,
                )
            };
            stmts.push(prop_stmt);
        }
    }

    // v0.115: emit one async runner per contract attack. Each shares the property
    // runtime and the run's seed lineage, so contract failures shrink and
    // reproduce exactly like a hand-written `property`. The report location uses a
    // representative test file of the target (the attack has no source line).
    let rep_rel_path: String = indices
        .iter()
        .find_map(|&i| {
            parsed[i]
                .test()
                .map(|_| parsed[i].identity_path().to_string_lossy().into_owned())
        })
        .unwrap_or_default();
    for f in &attack_fns {
        let FnName::Free(fname) = &f.name else {
            continue;
        };
        let attack_name = format!("contract {}", fname.name);
        discovered.push(DiscoveredCase {
            name: attack_name.clone(),
            location: None,
            tier: None,
            generative: Some("property"),
            doc: false,
        });
        let mut idx = prop_runners.len();
        let runner_name = format!("__prop_{}", sanitise_case_name(&attack_name, &mut idx));
        let prop_ordinal = prop_runners.len();
        prop_runners.push(runner_name.clone());
        let attack_stmt = emit_contract_attack_function(
            &runner_name,
            f,
            attack_resolved.as_ref().unwrap(),
            prop_ordinal,
            target_name,
            target_kind,
            unit_tables,
            unit_uses,
            unit_consumes,
            unit_consumes_aliases,
            &rep_rel_path,
            &runtime_use,
            hidden,
            tys,
        );
        if let Some(attack_stmt) = attack_stmt {
            stmts.push(attack_stmt);
        }
    }

    // #917: every case/property/stub/attack body above has now lowered, so every
    // `Json.decode[T]`/`Json.encode` root it reached for is in hand. The unit
    // this module targets (or one of its `uses`) exports `T` but no codec for
    // it — generate the caller's own closure here, the same pattern #661 uses
    // for a workers cross-context caller's consumed-boundary types. Plain
    // `function` declarations hoist, and this is a pure append after every
    // real statement above, so it disturbs no earlier source-map offset.
    //
    // P6.28 (design/tracks/the-ir.md §6a): `take_json_codec_roots` now returns
    // the checker's own `TyId`s, not `TypeRef`s — converted back here, once,
    // right before the one remaining consumer (`collect_codec_closure`) that is
    // still genuinely `TypeRef`-driven, rather than at each of the two push
    // sites in `emitter/lower.rs`. A root `ty_to_type_ref` rejects (functions,
    // effects, type variables) is silently dropped, the same filtering the old
    // push-time conversion already did — moving where the filter runs, not what
    // it filters.
    let json_codec_roots: Vec<TypeRef> = runtime_use
        .take_json_codec_roots()
        .into_iter()
        .filter_map(|ty| crate::emitter::ty_to_type_ref(ty, tys))
        .collect();
    if !json_codec_roots.is_empty() {
        let synthetic = synthetic_typed_commons_for_target(
            target_name,
            unit_tables,
            unit_uses,
            unit_consumes,
            hidden,
            tys,
        );
        let (codec_names, codec_insts) = crate::emitter::serialisation::collect_codec_closure(
            &json_codec_roots,
            &synthetic.types,
        );
        if !codec_names.is_empty() || !codec_insts.is_empty() {
            // The helpers below always name `JsonValue`/`BoundaryError` in their
            // own signatures, regardless of which arm (if any) the case bodies'
            // own `Json.decode`/`Json.encode` calls happened to trip.
            runtime_use.note_json_codec();
            runtime_use.note_boundary_codec();
            let qual = runtime_use.json_codec_qual();
            stmts.extend(crate::emitter::serialisation::decls_as_stmts_block(
                crate::emitter::serialisation::emit_helpers_for_owner_qualified(
                    &codec_names,
                    &synthetic.types,
                    target_name,
                    &qual,
                    &runtime_use,
                ),
            ));
            stmts.extend(crate::emitter::serialisation::decls_as_stmts(
                crate::emitter::serialisation::emit_generic_helpers_qualified(
                    &codec_insts,
                    &synthetic.types,
                    &qual,
                    &runtime_use,
                ),
            ));
        }
    }

    // #2052: a delivery helper per events service some body addressed with
    // `svc.event(e)`. Appended after every body has lowered, like the codec
    // closure above; a plain `function` declaration hoists.
    let event_services = runtime_use.test_event_services();
    if !event_services.is_empty() {
        stmts.push(TsStmt::raw(
            event_delivery_helpers(
                target_name,
                &event_services,
                unit_tables,
                unit_consumes,
                schema_versions,
            ),
            None,
        ));
    }

    // Module-level runner. v0.127: `only` filters to a single case/property by
    // name (the per-case run lens); undefined runs every one.
    let mut run_entries: Vec<(String, String)> = Vec::new();
    let mut case_index = 0;
    for &i in indices {
        let Some(test_decl) = parsed[i].test() else {
            continue;
        };
        for case in &test_decl.cases {
            let runner_name = &case_runners[case_index];
            run_entries.push((case.name.clone(), runner_name.clone()));
            case_index += 1;
        }
    }
    let mut prop_index = 0;
    for &i in indices {
        let Some(test_decl) = parsed[i].test() else {
            continue;
        };
        for prop in &test_decl.properties {
            let runner_name = &prop_runners[prop_index];
            run_entries.push((prop.name.clone(), runner_name.clone()));
            prop_index += 1;
        }
    }
    // v0.115: run the contract attacks (they follow every source-declared
    // property in `prop_runners`, so the index continues).
    for f in &attack_fns {
        let FnName::Free(fname) = &f.name else {
            continue;
        };
        let runner_name = &prop_runners[prop_index];
        run_entries.push((format!("contract {}", fname.name), runner_name.clone()));
        prop_index += 1;
    }
    stmts.extend(build_run_function(&run_entries));

    // #914: every case/property/stub/attack body above has now lowered, so
    // `runtime_use` carries whatever it actually reached for — fold that into
    // the runtime import's own `names` (deduped against the fixed set already
    // listed) and splice the finished import statement in at its reserved
    // position, the same ordering `emit_project`'s own deferred `write_header`
    // relies on.
    if runtime_use.boundary_codec() {
        append_missing_bindings(&mut runtime_names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
    }
    if runtime_use.json_codec() {
        append_missing_bindings(&mut runtime_names, emitter::JSON_CODEC_RUNTIME_IMPORTS);
    }
    if runtime_use.bytes() {
        append_missing_bindings(&mut runtime_names, emitter::BYTES_RUNTIME_IMPORTS);
    }
    if runtime_use.bytes_ops() {
        append_missing_bindings(&mut runtime_names, emitter::BYTES_OPS_RUNTIME_IMPORTS);
    }
    if runtime_use.time_ops() {
        append_missing_bindings(&mut runtime_names, emitter::TIME_OPS_RUNTIME_IMPORTS);
    }
    if runtime_use.url_ops() {
        append_missing_bindings(&mut runtime_names, emitter::URL_OPS_RUNTIME_IMPORTS);
    }
    if runtime_use.request_headers() {
        append_missing_bindings(&mut runtime_names, emitter::REQUEST_HEADERS_RUNTIME_IMPORTS);
    }
    if runtime_use.request_header_pairs() {
        append_missing_bindings(&mut runtime_names, ", __requestHeaderPairs");
    }
    if runtime_use.token_claims() {
        append_missing_bindings(&mut runtime_names, ", __tokenClaims");
    }
    if runtime_use.eq() {
        append_missing_bindings(&mut runtime_names, emitter::EQ_RUNTIME_IMPORTS);
    }
    // #1934: a case that compares a typed http outcome with `Ok(…)`.
    if runtime_use.http_result() {
        append_missing_bindings(&mut runtime_names, "HttpResult");
    }
    if runtime_use.int() {
        append_missing_bindings(&mut runtime_names, emitter::INT_RUNTIME_IMPORTS);
    }
    stmts.splice(
        import_idx..import_idx,
        emitter::runtime_files::runtime_import_stmts(false, &runtime_names, &runtime_import),
    );

    let module_file = module_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "module.ts".to_string());
    let program = TsProgram { stmts };
    let printed = bynk_ts::print_multi_source(&program, map, &module_file);
    Some((
        module_path.clone(),
        program,
        printed.source_map,
        RunnableTest {
            target_name: target_name.to_string(),
            module_path,
            kind: "unit",
            suite_name: target_name.to_string(),
            cases: discovered,
            skipped: Vec::new(),
            envs: Vec::new(),
            remote: false,
            generative: has_properties,
        },
    ))
}

/// v0.132: an aggregating barrel for a commons split across a directory.
///
/// Production emits a multi-file commons per file (`out/<name>/*.ts`) and never
/// an aggregate `out/<name>.ts`, but every test/integration import references the
/// commons as one namespace (`import * as ns from "./<name>.js"`). Emit
/// `out/<name>.ts` that `export *`s each of the commons' source files so that
/// namespace import resolves for the directory layout exactly as it does for a
/// single file. The flat merge cannot collide: intra-commons symbol names are
/// already unique across every kind (`bynk.resolve.duplicate_type`/`_fn`/
/// `_method`), so no two files re-export the same name.
///
/// Returns `None` — no barrel — when the commons is *not* multi-file (a
/// single-file commons already owns `out/<name>.ts`; a barrel would collide), or
/// when this barrel path was already emitted (`emitted` dedups across the several
/// test/integration modules that may import the same commons).
///
/// `groups` maps each production unit name to the `parsed` indices of its files;
/// the multi-file predicate and the re-exported file set both read from there, so
/// the barrel can never drop a file that declares nothing type/fn/method-shaped
/// (which `FileDeclIndex` would omit).
///
/// #1820: a multi-file *context*'s barrel is also part of a production bundle
/// build, emitted by `run_checks` ahead of the test passes, since its
/// composition root imports the context from `out/<name>.ts` too; on workers,
/// `run_checks` claims its path, since the context's modules are the Worker's.
/// `run_checks` alone passes `modules_out`, this unit's staged modules, for
/// [`duplicated_export_reexports`]; the test passes, which only ever reach a
/// commons here, pass none. A commons' files repeat no generated export: it
/// has no boundary codecs, and only a context rebrands a `uses`d type.
pub(super) fn emit_commons_barrel(
    name: &str,
    groups: &BTreeMap<String, Vec<usize>>,
    parsed: &[ParsedFile],
    import_ext: ImportExt,
    emitted: &mut HashSet<PathBuf>,
    modules_out: &[StagedFile],
) -> Option<StagedFile> {
    let indices = groups.get(name)?;
    // Multi-file only: *every* file must sit under a `<name>/` directory, the
    // layout where no `out/<name>.ts` is otherwise produced. A unit with any file
    // at `<name>.bynk` already owns `out/<name>.ts`, so it must not get a barrel.
    if indices.is_empty()
        || !indices
            .iter()
            .all(|&i| is_multi_file_layout(&parsed[i].source_path(), name))
    {
        return None;
    }
    let output_path = commons_dir_for(name).with_extension("ts");
    if !emitted.insert(output_path.clone()) {
        return None; // already emitted for an earlier importing module
    }
    // Barrel body: `export *` per distinct source file, sorted for determinism.
    // Specifiers run through the emitter's cross-commons machinery so the path is
    // correct for dotted names (`commons a.b` sits at `out/a/b.ts`, re-exporting
    // `./b/<file>.js`) and forward-slash-normalised on Windows. `commons_dir_for`
    // stands in for the barrel's own location — its parent is the directory the
    // barrel lives in, which is all the relative computation needs.
    let barrel_loc = commons_dir_for(name);
    let mut files: Vec<PathBuf> = indices.iter().map(|&i| parsed[i].source_path()).collect();
    files.sort();
    files.dedup();
    let mut stmts = vec![TsStmt::comment(
        "Generated by bynkc — do not edit by hand.",
        None,
    )];
    for file in &files {
        let spec = emitter::cross_commons_import_specifier_for_path(&barrel_loc, file, import_ext);
        stmts.push(TsStmt::decl(TsDecl::ReExportAll { from: spec }, None));
    }
    let (merged, defined) = merged_unit_helpers(
        name,
        indices,
        parsed,
        &barrel_loc,
        import_ext,
        |file| file.to_path_buf(),
        true,
    );
    stmts.extend(duplicated_export_reexports(
        &files,
        modules_out,
        &defined,
        &barrel_loc,
        import_ext,
    ));
    stmts.extend(merged);
    Some(StagedFile {
        output_path,
        document: Document::Ts(TsProgram { stmts }),
        source_map: None,
        debug_metadata: None,
    })
}

/// #1844 review: the barrel's own definitions of the helpers every file of a
/// multi-file *context* generates for itself. User names are unique across a
/// unit, but these are not: each file with agents exports `__resetAgents`
/// (over its own agents), and each file with services exports `__makeSurface`
/// and the context's `__<Ctx>Deps` (over its own services). Two files of
/// either kind made the barrel's `export *` ambiguous (TS2308), and at
/// runtime the ambiguous name is dropped from the namespace, so a test's
/// `__resetAgents()` was `undefined`. Where two or more files contribute one,
/// the barrel exports a merged definition, which takes precedence over the
/// star exports: reset every file's agents; combine every file's surface over
/// the intersection of their deps.
///
/// `module_of` maps a file's source path to the path its module is emitted
/// at, relative to which `barrel_loc` imports it: the identity for a bundle
/// barrel, the file's own module under `handlers/` for a Worker's
/// `handlers.ts` (#1820). `surface` says whether each file exports a
/// surface at all: a Worker's files do not, since its composition root
/// (`compose.ts`) builds the surface over the whole context.
pub(super) fn merged_unit_helpers(
    name: &str,
    indices: &[usize],
    parsed: &[ParsedFile],
    barrel_loc: &Path,
    import_ext: ImportExt,
    module_of: impl Fn(&Path) -> PathBuf,
    surface: bool,
) -> (Vec<TsStmt>, Vec<String>) {
    let mut with_agents: Vec<PathBuf> = Vec::new();
    let mut with_services: Vec<PathBuf> = Vec::new();
    for &i in indices {
        let pf = &parsed[i];
        if pf.kind() != UnitKind::Context {
            continue;
        }
        let items = pf.items();
        if items.iter().any(|it| matches!(it, CommonsItem::Agent(_))) {
            with_agents.push(module_of(&pf.source_path()));
        }
        if items.iter().any(|it| matches!(it, CommonsItem::Service(_))) {
            with_services.push(module_of(&pf.source_path()));
        }
    }
    with_agents.sort();
    with_services.sort();
    let mut stmts = Vec::new();
    // The names defined below, pushed in the branch that defines each, so
    // `duplicated_export_reexports` never also re-exports one file's copy.
    // Unlike a codec or a rebrand, each file's copy differs (it covers that
    // file's agents or services), so re-exporting one would be wrong.
    let mut defined = Vec::new();
    let mut aliases: BTreeMap<PathBuf, String> = BTreeMap::new();
    let mut alias_of = |file: &PathBuf, stmts: &mut Vec<TsStmt>| -> String {
        if let Some(a) = aliases.get(file) {
            return a.clone();
        }
        let alias = format!("__file{}", aliases.len());
        stmts.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: alias.clone(),
                from: emitter::cross_commons_import_specifier_for_path(
                    barrel_loc, file, import_ext,
                ),
            },
            None,
        ));
        aliases.insert(file.clone(), alias.clone());
        alias
    };
    if with_agents.len() > 1 {
        let calls: Vec<String> = with_agents
            .iter()
            .map(|f| format!("  {}.__resetAgents();", alias_of(f, &mut stmts)))
            .collect();
        stmts.push(TsStmt::raw(
            format!(
                "export function __resetAgents(): void {{\n{}\n}}\n",
                calls.join("\n")
            ),
            None,
        ));
        defined.push("__resetAgents".to_string());
    }
    if surface && with_services.len() > 1 {
        let deps = format!("__{}Deps", emitter::emit::context_pascal(name));
        let files: Vec<String> = with_services
            .iter()
            .map(|f| alias_of(f, &mut stmts))
            .collect();
        let intersection = files
            .iter()
            .map(|a| format!("{a}.{deps}"))
            .collect::<Vec<_>>()
            .join(" & ");
        let spreads = files
            .iter()
            .map(|a| format!("...{a}.__makeSurface(deps)"))
            .collect::<Vec<_>>()
            .join(", ");
        stmts.push(TsStmt::raw(
            format!(
                "export type {deps} = {intersection};\n\
                 export function __makeSurface(deps: {deps}) {{\n  return {{ {spreads} }};\n}}\n"
            ),
            None,
        ));
        defined.push("__makeSurface".to_string());
        defined.push(deps);
    }
    (stmts, defined)
}

/// #1820: the explicit re-exports a multi-file context's barrel needs where
/// two or more of its files' modules export one name. User names are unique
/// across a unit, but each file emits its own copy of what its declarations
/// need from outside it: the context's rebrand of a `uses`d commons type, that
/// commons type's codecs re-exported, a boundary type's codecs (wherever the
/// type is declared). Each copy is a function of the type alone, so the
/// barrel re-exports the name from the first module (sorted by path) that
/// exports it, which takes precedence over `export *`; without it, `export *`
/// is ambiguous (TS2308) and drops the name at runtime. `merged` names the
/// helpers the barrel defines itself (`merged_unit_helpers`), which are
/// skipped.
///
/// `modules` is each file's module path; `emitted`, the staged output in
/// which each is found at its [`ts_output_path`]. What a module exports is
/// read with [`TsStmt::exported_names`], which sees structured exports only;
/// every generated name that repeats across files is exported structurally
/// (the commons codec re-export is [`TsDecl::ExportNames`] for this reason).
/// A name the chosen module exports only as a type (a rebrand of a commons
/// type with no `of` constructor is a bare `type` alias) is re-exported as
/// `type X`: under the build's `isolatedModules`, `export { X }` of a
/// type-only name is TS1205 (#1862 review).
pub(super) fn duplicated_export_reexports(
    modules: &[PathBuf],
    emitted: &[StagedFile],
    merged: &[String],
    barrel_loc: &Path,
    import_ext: ImportExt,
) -> Vec<TsStmt> {
    // Per name, each exporting module and whether it exports a value under
    // the name.
    let mut exporters: BTreeMap<&str, Vec<(&PathBuf, bool)>> = BTreeMap::new();
    for module in modules {
        let output = ts_output_path(module);
        let Some(Document::Ts(program)) = emitted
            .iter()
            .find(|f| f.output_path == output)
            .map(|f| &f.document)
        else {
            continue;
        };
        for stmt in &program.stmts {
            for exported in stmt.exported_names() {
                // A type is often both an `interface` and a `const` of one
                // name: one module, counted once, with a value if either
                // statement exports one. This loop visits one module's
                // statements at a time, so a module's earlier entry for the
                // name, if any, is the last one.
                let modules = exporters.entry(exported.name).or_default();
                match modules.last_mut() {
                    Some((m, has_value)) if *m == module => *has_value |= exported.has_value,
                    _ => modules.push((module, exported.has_value)),
                }
            }
        }
    }
    let mut chosen: BTreeMap<&PathBuf, Vec<String>> = BTreeMap::new();
    for (name, modules) in &exporters {
        if let [(first, has_value), _, ..] = modules.as_slice()
            && !merged.iter().any(|m| m == name)
        {
            let spec = if *has_value {
                name.to_string()
            } else {
                format!("type {name}")
            };
            chosen.entry(first).or_default().push(spec);
        }
    }
    chosen
        .into_iter()
        .map(|(module, names)| {
            let from =
                emitter::cross_commons_import_specifier_for_path(barrel_loc, module, import_ext);
            TsStmt::decl(TsDecl::ReExport { names, from }, None)
        })
        .collect()
}

/// Render the relative import path from the `tests/` output directory to the
/// directory holding a target unit's TypeScript output.
fn relative_import_for_test(target_dir: &Path) -> String {
    let parts: Vec<String> = target_dir
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s.to_string_lossy().to_string()),
            _ => None,
        })
        .collect();
    if parts.is_empty() {
        "../index".to_string()
    } else {
        format!("../{}", parts.join("/"))
    }
}

/// Finding #17: the source lives at `emitter/test_runtime/expectation.ts` —
/// a real, syntax-highlighted `.ts` file rather than a Rust string built
/// line-by-line via `push_str`. `ExpectationError`'s fields are declared and
/// assigned explicitly rather than via TS parameter properties: parameter
/// properties are a transform-only construct that Node's strip-only
/// type-stripping rejects (`ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`), and
/// `bynkc test --inspect` runs this `.ts` directly under strip-only Node
/// (slice 2, ADR 0104). The explicit form is equivalent and strip-clean.
fn expectation_runtime_helpers() -> String {
    include_str!("../emitter/test_runtime/expectation.ts").to_string()
}

/// #2052: the `__bynkEvent_<svc>` helper behind each `svc.event(payload[,
/// envelope])` address in a test module. It delivers as the Worker's
/// `/_bynk/event/` route and the Bundle fan-out do: through the service's
/// generated `event` method, whose prologue runs the subscription's pattern
/// filter and `via schema(N)` guard, so the helper re-implements neither. The
/// method takes the envelope only when the handler declares `env` or the
/// service has a `via schema(N)` guard (`emit_event_wrapper`'s condition), so
/// the helper forwards it only then.
///
/// An omitted envelope (`undefined`) is supplied as an emission would mint it:
/// a fresh `eventId` per call (`test-event-<n>`, counted across the module), the
/// publishing context as `publisherId`, the test clock's reading (`0`, the
/// epoch) as `emittedAt`, and the event's current schema version.
fn event_delivery_helpers(
    target_name: &str,
    services: &[String],
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    schema_versions: &HashMap<String, i64>,
) -> String {
    use bynk_syntax::ast::ServiceProtocol;
    let ns = test_ns(target_name);
    let mut out = String::new();
    let mut counts = false;
    for svc in services {
        let Some(decl) = unit_tables
            .get(target_name)
            .and_then(|t| t.services.get(svc))
        else {
            continue;
        };
        let ServiceProtocol::Events {
            event_type,
            schema_dispatch,
            ..
        } = &decl.protocol
        else {
            continue;
        };
        let event = match event_type {
            TypeRef::Named(id) => id.name.clone(),
            _ => continue,
        };
        let takes_envelope = schema_dispatch.is_some()
            || decl.handlers.iter().any(|h| {
                matches!(h.kind, bynk_syntax::ast::HandlerKind::Event) && h.params.len() == 2
            });
        // #2052 review: the publisher resolves as the service's header
        // resolves `E` (the target's own event, else the one consumed context
        // declaring it). With no unique one the checker refuses a default
        // envelope (`bynk.test.event_publisher_ambiguous`), so the helper then
        // only forwards a case's own.
        let consumed = unit_consumes.get(target_name).cloned().unwrap_or_default();
        let defaults =
            bynk_check::test_suites::event_publisher(target_name, &event, unit_tables, &consumed)
                .ok()
                .map(|owner| {
                    let version = schema_versions
                        .get(&format!("{owner}.{event}"))
                        .copied()
                        .or_else(|| {
                            unit_tables
                                .get(&owner)
                                .and_then(|t| t.events.get(&event))
                                .map(|d| d.schema_version())
                        })
                        .unwrap_or(1);
                    (owner, version)
                });
        if takes_envelope {
            let env = match &defaults {
                Some((owner, version)) => {
                    counts = true;
                    format!(
                        "envelope ?? {{ eventId: `test-event-${{++__bynkEventSeq}}`, publisherId: \"{}\", emittedAt: 0, schemaVersion: {version} }}",
                        crate::emitter::escape_ts_string(owner)
                    )
                }
                None => "envelope".to_string(),
            };
            out.push_str(&format!(
                "function __bynkEvent_{svc}(payload: unknown, envelope: unknown, deps: unknown) {{\n  const env = {env};\n  return {ns}.{svc}.event(payload as never, env as never, deps as never);\n}}\n"
            ));
        } else {
            // The handler cannot observe an envelope, so a case's own is dropped.
            out.push_str(&format!(
                "function __bynkEvent_{svc}(payload: unknown, _envelope: unknown, deps: unknown) {{\n  return {ns}.{svc}.event(payload as never, deps as never);\n}}\n"
            ));
        }
    }
    if counts {
        out.insert_str(0, "let __bynkEventSeq = 0;\n");
    }
    out
}

/// #1994: the test double for the Worker's `ExecutionContext` — a `waitUntil`
/// that collects the promises a `~>` send hands it, and (L7, #2414) a `cache`
/// that records each `EdgeCache` purge and accepts it unless `succeeds` is
/// cleared — and `__bynkSettle`, which
/// awaits them after an addressed handler call, so a case's expectations see
/// the sends' effects. Source lives at `emitter/test_runtime/send.ts`.
fn send_runtime_helpers() -> String {
    include_str!("../emitter/test_runtime/send.ts").to_string()
}

/// #2137: the harness clock — `__bynkClockNow`, which `advance Clock by`
/// moves (`__bynkAdvanceClock`, refusing a negative at run time), and the
/// `Clock` that reads it. Source lives at `emitter/test_runtime/clock.ts`.
fn clock_runtime_helpers() -> String {
    include_str!("../emitter/test_runtime/clock.ts").to_string()
}

/// #2138: `__bynkRunId`, the value `run.id` reads. A remote build's `system`
/// module reads `BYNK_TEST_RUN_ID` (the runner sets a fresh one); every
/// in-process module is the fixed `"local"`, even when the variable is set
/// (an `--against` run's unit suites share its process environment).
fn run_id_helper(remote: bool) -> String {
    if remote {
        include_str!("../emitter/test_runtime/run_id.ts").to_string()
    } else {
        "const __bynkRunId: string = \"local\";\n".to_string()
    }
}

/// #2138: whether a case body reads `run.id` anywhere.
fn block_uses_run_id(block: &Block) -> bool {
    let mut found = false;
    let mut check = |e: &Expr| {
        if let ExprKind::FieldAccess { receiver, field } = &e.kind
            && field.name == "id"
            && matches!(&receiver.kind, ExprKind::Ident(id) if id.name == "run")
        {
            found = true;
        }
    };
    let mut exprs = Vec::new();
    for s in &block.statements {
        bynk_syntax::ast::statement_exprs(s, &mut exprs);
    }
    for e in exprs {
        bynk_ir::walk_exprs(e, &mut check);
    }
    bynk_ir::walk_exprs(&block.tail, &mut check);
    found
}

/// #2137: whether a case body (or a block nested in it) holds an `advance`.
fn block_uses_advance(b: &Block) -> bool {
    if b.statements
        .iter()
        .any(|s| matches!(s, bynk_syntax::ast::Statement::Advance(_)))
    {
        return true;
    }
    let mut exprs = Vec::new();
    for s in &b.statements {
        bynk_syntax::ast::statement_exprs(s, &mut exprs);
    }
    exprs.push(&b.tail);
    let mut found = false;
    for e in exprs {
        bynk_ir::walk_exprs(e, &mut |x: &Expr| {
            found |= match &x.kind {
                ExprKind::Block(inner) => block_uses_advance(inner),
                ExprKind::If {
                    then_block,
                    else_block,
                    ..
                } => block_uses_advance(then_block) || block_uses_advance(else_block),
                ExprKind::Match { arms, .. } => arms.iter().any(|a| {
                    matches!(&a.body, bynk_syntax::ast::MatchBody::Block(b) if block_uses_advance(b))
                }),
                _ => false,
            };
        });
    }
    found
}

/// #1994: whether a context target has a handler that sends with `~>`, so its
/// handlers' `deps` require `__exec` — the same gate the Worker entry and
/// `compose` thread the request's execution context on.
fn target_uses_send(
    target_name: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
) -> bool {
    target_kind == UnitKind::Context
        && unit_tables.get(target_name).is_some_and(|t| {
            t.services.values().any(|s| {
                s.handlers
                    .iter()
                    .any(|h| crate::emitter::block_uses_send(&h.body))
            })
        })
}

/// #2050: whether a context target has a handler — a service's or an agent's
/// — that emits through the first-party `Events`, so the `deps` its handlers
/// run on require `__eventsDispatch`: the same gate (`unit_table_uses_emit`)
/// the composition root builds its own dispatch closure on.
fn target_uses_emit(
    target_name: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    unit_callees: &HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
) -> bool {
    target_kind == UnitKind::Context
        && unit_flattened
            .get(target_name)
            .and_then(|f| f.get("Events"))
            .is_some_and(|owner| owner == bynk_check::firstparty::BYNK_UNIT)
        && unit_tables
            .get(target_name)
            .is_some_and(|t| unit_table_uses_emit(t, unit_callees.get(target_name)))
}

/// #1485: the test-only HS256 JWT signer `emit_system_http_support`'s own
/// `system` suite driver relies on to sign a `by User("bob")`-style actor's
/// `Authorization: Bearer …` — `__bynkNow`/`__b64url`/`__bytesB64url`/
/// `__bynkSignHs256`. Found by #1472's own callee-cascade read as a
/// genuinely new gap (unlike every other permanent site in this arc, this
/// one had no in-place argument for staying hand-written text): moved to
/// `emitter/test_runtime/jwt_signer.ts`, the same committed-`.ts`-file
/// treatment `expectation_runtime_helpers`/`stub_runtime_helpers` above
/// already get, rather than a raw Rust string literal with `\`-continuations.
fn jwt_signer_runtime_helpers() -> String {
    include_str!("../emitter/test_runtime/jwt_signer.ts").to_string()
}

/// v0.118: emit the `__Stub_<Cap>` stub class for a capability seam
/// overridden by `stub` clauses (testing track slice 6). One async method
/// per overridden operation renders its clauses as a first-match-wins if-chain
/// over the call's argument patterns; a matched clause returns its lowered
/// value, throws an injected fault, or advances a per-call sequence cursor.
///
/// #1483: returns one real [`TsStmt`] (was `String`) — a `Raw` carrier for
/// the whole class, the same Decision-C treatment `emit_provider` (#1480)
/// already established: the class wrapper (header/fields) stays hand-written
/// text (cited in place below), each method printed as its own real
/// `TsClassMethod` fragment and spliced directly in. No `nested_map` — this
/// function has no `LowerCtx`/source-map involvement anywhere in its own
/// construction (`lower_stub_value_block`/`emit_stub_rhs` build already-
/// formed text, not through a body-local `SourceMapBuilder`), so there is
/// nothing to merge, unlike `emit_provider`/`emit_service`/`emit_agent`'s own
/// per-handler bodies.
#[allow(clippy::too_many_arguments)]
fn emit_stub_class(
    rp: &ResolvedStub,
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    runtime_use: &RuntimeUse,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<Types>,
) -> TsStmt {
    let mut out = String::new();
    let cap = &rp.cap;
    // Value expressions are lowered in the target context's privileged view, so
    // its types, variants and `uses` vocabulary resolve unqualified.
    let owning_unit = target_name.to_string();
    let scope_ns = test_ns(&owning_unit);
    // Each in-scope type name's *owning* namespace — the target's own for a
    // locally-declared type, but the specific `uses`d commons' for one reached
    // only through it (never the target's), since `emit_context_rebrands`
    // only re-exports a `uses`-sourced type under the target's namespace when
    // the target's own lowered body references it by name — a stub class
    // implementing an adapter-flattened capability (e.g. `Locale`) generally
    // doesn't (Locale capability track, slice 1, #844).
    let mut type_ns: HashMap<String, String> = unit_tables
        .get(&owning_unit)
        .map(|t| {
            t.types
                .keys()
                .map(|n| (n.clone(), scope_ns.clone()))
                .collect()
        })
        .unwrap_or_default();
    if let Some(used) = unit_uses.get(&owning_unit) {
        for u in used {
            if let Some(table) = unit_tables.get(u) {
                let uns = test_ns(u);
                for n in table.types.keys() {
                    type_ns.entry(n.clone()).or_insert_with(|| uns.clone());
                }
            }
        }
    }
    // #291: a consumed adapter's types (`bynk`'s `Response`, `FetchError`) are
    // what a platform capability's stub takes and returns.
    let adapters: Vec<&String> = unit_consumes
        .get(&owning_unit)
        .into_iter()
        .flatten()
        .filter(|u| unit_tables.get(*u).and_then(|t| t.kind) == Some(UnitKind::Adapter))
        .collect();
    for u in &adapters {
        let uns = test_ns(u);
        for n in unit_tables[*u].types.keys() {
            type_ns.entry(n.clone()).or_insert_with(|| uns.clone());
        }
    }
    let scope_names: Vec<String> = if let Some(table) = unit_tables.get(&owning_unit) {
        let mut v: Vec<String> = table
            .types
            .keys()
            .chain(table.fns.keys())
            .cloned()
            .collect();
        v.sort();
        v.dedup();
        v
    } else {
        Vec::new()
    };
    let scope_type_names: Vec<(String, Vec<String>)> = unit_tables
        .get(&owning_unit)
        .map(|t| aliased_types(t, |_| true))
        .unwrap_or_default();

    // Group clause indices by method, preserving resolution order (case-scoped
    // clauses precede suite-scoped ones, so they win the first-match chain).
    let mut by_method: std::collections::BTreeMap<String, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (idx, clause) in rp.clauses.iter().enumerate() {
        by_method
            .entry(clause.method.name.clone())
            .or_default()
            .push(idx);
    }

    // The class wrapper (header/fields) stays hand-written text — Decision C
    // (#1359's own `emit_provider` precedent): each method's own body is one
    // opaque `TsStmt::raw` (it mixes real structure with `lower_stub_value_
    // block`'s own already-lowered opaque text, an `async () => { ... }`
    // IIFE whose own multi-statement block body no `TsArrowBody::Block`
    // renderer can flatten onto one line — the same "block-bodied arrow
    // stays opaque" call `emit_composition_root`'s own `__eventsDispatch`
    // closure already made), so each method is printed as
    // its own real `TsClassMethod` fragment via `bynk_ts::print_class_method`
    // at depth 0 — the only depth `render_class_method`'s own debug_assert
    // allows a `Raw`-bodied method to print correctly at — and spliced
    // directly into this still-hand-written wrapper, with no blank line
    // between fields/methods or between methods (matching this function's
    // own zero-blank-line output exactly).
    out.push_str(&format!("class __Stub_{cap} {{\n"));
    // One per-call sequence cursor field per `returns each` clause.
    for (idx, clause) in rp.clauses.iter().enumerate() {
        if matches!(clause.rhs, StubRhs::ReturnsEach(..)) {
            out.push_str(&format!("  __seq_{idx} = 0;\n"));
        }
    }
    // #291: a case-scoped clause applies only while its own case runs, so a
    // class with one carries the running case's name (`__makeTestDeps`'s
    // argument), and `__applies` tells `__bynkOverlay` which operations this
    // case stubs at all — an operation stubbed only by other cases reaches the
    // tier default instead. #1860: a suite-scoped clause, when another suite
    // shares the module, applies only while its own suite's runs do, so the
    // class also carries the running suite's ordinal.
    let case_scoped = stub_is_scoped(rp);
    if case_scoped {
        out.push_str("  __case: string | undefined;\n");
        if stub_is_suite_scoped(rp) {
            out.push_str("  __suite: number | undefined;\n");
            out.push_str(
                "  constructor(c?: string, s?: number) {\n    this.__case = c;\n    this.__suite = s;\n  }\n",
            );
        } else {
            out.push_str("  constructor(c?: string) {\n    this.__case = c;\n  }\n");
        }
        out.push_str("  __applies(op: string): boolean {\n");
        for (method, clause_idxs) in &by_method {
            let scopes: Vec<&StubScope> =
                clause_idxs.iter().map(|&i| &rp.clause_scopes[i]).collect();
            // An operation with any clause for every run is always stubbed.
            if scopes.contains(&&StubScope::Every) {
                continue;
            }
            out.push_str(&format!(
                "    if (op === {}) return {};\n",
                bynk_ts::print_expr(&str_lit(method.as_str())),
                scopes_test(&scopes),
            ));
        }
        out.push_str("    return true;\n  }\n");
    }

    for (method, clause_idxs) in &by_method {
        let Some(op) = rp.cap_decl.ops.iter().find(|o| &o.name.name == method) else {
            continue;
        };
        let params: Vec<TsParam> = op
            .params
            .iter()
            .map(|p| TsParam {
                name: p.name.name.clone(),
                ty: Some(emitter::ts_type_ref_qualified_multi_ts_type(
                    &p.type_ref,
                    &type_ns,
                )),
                optional: false,
            })
            .collect();
        let return_ty = emitter::ts_type_ref_qualified_multi_ts_type(&op.return_type, &type_ns);

        let mut body_text = String::new();
        crate::emitter::extend_printed_at(
            &mut body_text,
            emit_ns_destructure(&scope_ns, &scope_names, &scope_type_names),
            2,
        );
        let mut taken: HashSet<&String> = scope_names.iter().collect();
        for u in &adapters {
            let mut names: Vec<String> = unit_tables[*u]
                .types
                .keys()
                .filter(|n| taken.insert(*n))
                .cloned()
                .collect();
            if names.is_empty() {
                continue;
            }
            names.sort();
            let types = aliased_types(&unit_tables[*u], |n| names.contains(n));
            crate::emitter::extend_printed_at(
                &mut body_text,
                emit_ns_destructure(&test_ns(u), &names, &types),
                2,
            );
        }
        for &idx in clause_idxs {
            let clause = &rp.clauses[idx];
            // Argument-pattern consts: a `Value(e)` pattern lowers to a const the
            // condition compares structurally; an `Any` pattern contributes none.
            let mut cond_parts: Vec<String> = Vec::new();
            for (i, pat) in clause.args.iter().enumerate() {
                if let ArgPattern::Value(e) = pat
                    && let Some(param) = op.params.get(i)
                {
                    let body = lower_stub_value_block(
                        e,
                        &param.type_ref,
                        &[],
                        target_name,
                        unit_tables,
                        unit_uses,
                        unit_consumes,
                        unit_consumes_aliases,
                        runtime_use,
                        hidden,
                        tys,
                    );
                    let vname = format!("__pv_{idx}_{i}");
                    body_text.push_str(&format!("    const {vname} = await (async () => {{\n"));
                    for line in body.lines() {
                        body_text.push_str("      ");
                        body_text.push_str(line);
                        body_text.push('\n');
                    }
                    body_text.push_str("    })();\n");
                    runtime_use.note_eq();
                    cond_parts.push(format!("__bynkEq({}, {vname})", param.name.name));
                }
            }
            let scope = &rp.clause_scopes[idx];
            if *scope != StubScope::Every {
                cond_parts.insert(0, scopes_test(&[scope]));
            }
            let cond = if cond_parts.is_empty() {
                "true".to_string()
            } else {
                cond_parts.join(" && ")
            };
            body_text.push_str(&format!("    if ({cond}) {{\n"));
            let rhs_body = emit_stub_rhs(
                clause,
                idx,
                op,
                target_name,
                unit_tables,
                unit_uses,
                unit_consumes,
                unit_consumes_aliases,
                runtime_use,
                hidden,
                tys,
            );
            for line in rhs_body.lines() {
                body_text.push_str("      ");
                body_text.push_str(line);
                body_text.push('\n');
            }
            body_text.push_str("    }\n");
        }
        body_text.push_str(&format!(
            "    throw new globalThis.Error(\"bynk: no stub clause matched for {cap}.{method}\");\n"
        ));

        let method_node = TsClassMethod {
            name: method.clone(),
            private: false,
            is_async: true,
            params,
            return_type: Some(return_ty),
            doc: None,
            body: vec![TsStmt::raw(body_text, None)],
        };
        out.push_str(&bynk_ts::print_class_method(&method_node, 0));
    }
    out.push_str("}\n");
    TsStmt::raw(out, None)
}

/// `throw new Error("bynk: injected capability fault (stubs … fails)");` —
/// the one real site anywhere in this cluster injecting a synthetic
/// capability failure, shared by [`emit_stub_rhs`]'s own `Fails`/
/// `ReturnsEach`-with-a-failing-outcome arms.
fn stub_fault_stmt() -> TsStmt {
    TsStmt::throw_stmt(
        TsExpr::New {
            callee: Box::new(ident("globalThis.Error")),
            args: vec![str_lit("bynk: injected capability fault (stubs … fails)")],
        },
        None,
    )
}

/// Prefix every line of `text` with `prefix` — [`TsStmtKind::Raw`]'s own
/// "pre-indented at a fixed absolute depth by the caller" contract (its own
/// doc), used here to carry [`lower_stub_value_block`]'s already-lowered
/// opaque body text as one switch-case's own statement.
fn reindent_by(text: &str, prefix: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        out.push_str(prefix);
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// v0.118: render the body of one matched `stub` clause — a `returns` value,
/// a `fails` fault, or a `returns each` per-call sequence (last outcome repeats).
#[allow(clippy::too_many_arguments)]
fn emit_stub_rhs(
    clause: &StubClause,
    clause_idx: usize,
    op: &CapabilityOp,
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    runtime_use: &RuntimeUse,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<Types>,
) -> String {
    let lower = |e: &Expr| {
        lower_stub_value_block(
            e,
            &op.return_type,
            &op.params,
            target_name,
            unit_tables,
            unit_uses,
            unit_consumes,
            unit_consumes_aliases,
            runtime_use,
            hidden,
            tys,
        )
    };
    match &clause.rhs {
        StubRhs::Returns(e) => lower(e),
        StubRhs::Fails(_) => bynk_ts::print_stmt(&stub_fault_stmt(), 0),
        StubRhs::ReturnsEach(outcomes, _) => {
            let n = outcomes.len();
            let mut stmts = vec![TsStmt::const_stmt(
                TsBindingName::Ident("__k".to_string()),
                None,
                member(ident("this"), format!("__seq_{clause_idx}")),
                None,
            )];
            if n > 1 {
                stmts.push(if_(
                    TsExpr::Binary {
                        op: TsBinaryOp::LessThan,
                        left: Box::new(member(ident("this"), format!("__seq_{clause_idx}"))),
                        right: Box::new(num_lit((n - 1).to_string())),
                    },
                    TsStmt::increment(member(ident("this"), format!("__seq_{clause_idx}")), None),
                ));
            }
            let outcome_body = |o: &SeqOutcome| match o {
                SeqOutcome::Value(e) => lower(e),
                SeqOutcome::Fails(_) => bynk_ts::print_stmt(&stub_fault_stmt(), 0),
            };
            let mut cases: Vec<TsSwitchCase> = outcomes
                .iter()
                .enumerate()
                .take(n.saturating_sub(1))
                .map(|(j, o)| TsSwitchCase {
                    test: Some(num_lit(j.to_string())),
                    body: vec![TsStmt::raw(reindent_by(&outcome_body(o), "    "), None)],
                    default_braced: false,
                    // Arc E slice 6 (#1445): unaffected by `case_braced`'s
                    // addition — this site's own non-`default` cases were
                    // already, and stay, `{ }`-blocked.
                    case_braced: true,
                })
                .collect();
            cases.push(TsSwitchCase {
                test: None,
                body: vec![TsStmt::raw(
                    reindent_by(&outcome_body(&outcomes[n - 1]), "    "),
                    None,
                )],
                default_braced: true,
                case_braced: true,
            });
            stmts.push(TsStmt::switch_stmt(ident("__k"), cases, None));
            stmts.iter().map(|s| bynk_ts::print_stmt(s, 0)).collect()
        }
    }
}

/// v0.118: lower a single `stub` value expression as if it were a provider
/// operation's tail — type-check it in the target's privileged view (so variant
/// constructors and `uses` names resolve) then lower it to an async body ending
/// in `return <value>;`. Mirrors the retired mock-op-body lowering.
#[allow(clippy::too_many_arguments)]
fn lower_stub_value_block(
    e: &Expr,
    ret_type: &TypeRef,
    params: &[Param],
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    runtime_use: &RuntimeUse,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<Types>,
) -> String {
    let owning_unit = target_name.to_string();
    let mut typed = synthetic_typed_commons_for_target(
        &owning_unit,
        unit_tables,
        unit_uses,
        unit_consumes,
        hidden,
        tys,
    );
    let block = test_suites::value_block(e);
    if let Some((resolved, _)) = test_suites::build_privileged_resolved(
        &owning_unit,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    ) {
        let mut errs: Vec<CompileError> = Vec::new();
        // Build-mode re-check for the lowering's expr types; the analyse exit has
        // already passed, so nothing records (fresh sink).
        checker::check_handler_body(
            &resolved,
            checker::HandlerBodyCheck::new(&block, ret_type, params, &[]),
            checker::CheckSinks {
                tys,
                expr_types: &mut typed.expr_types,
                errors: &mut errs,
                refs: &mut RefSink::new(),
                hints: &mut HintSink::new(),
                locals: &mut LocalsSink::new(),
                requirements: &mut RequirementSink::new(),
                callees: &mut typed.callees,
            },
        );
    }
    let cross = bynk_check::resolver::CrossContextInfo::default();
    // v0.70: `stub` value scaffolding is not user test logic, so its source
    // map is discarded — it stays unmapped (a deliberate scope cut).
    emitter::lower_block_to_async_body(&block, ret_type, &mut typed, &cross, runtime_use).0
}

fn synthetic_typed_commons_for_target(
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<checker::Types>,
) -> checker::TypedCommons {
    let table = unit_tables.get(target_name).cloned().unwrap_or_default();
    let mut types = table.types;
    let mut fns = table.fns;
    let mut methods = table.methods;
    // v0.117: carry the target's capability declarations into the synthetic
    // commons items so observation lowering (`with` param destructure,
    // `trace(Cap.op)` record fields) can look up each op's parameter names.
    // #291: a flattened platform capability (`bynk`'s `Fetch`, `Logger`, …) is
    // a seam too, and a consumed adapter's types (`Response`, `FetchError`) are
    // the values a stub returns and a case names — so both come in, the
    // target's own winning a name clash.
    let mut caps: std::collections::BTreeMap<String, bynk_syntax::ast::CapabilityDecl> = table
        .capabilities
        .iter()
        .map(|(n, c)| (n.clone(), c.clone()))
        .collect();
    // #2062: a consumed *context*'s types too. The scope setup destructures
    // them (`emit_test_scope_setup`), and the lowering qualifies a bare
    // nullary variant (`Domestic` → `Region.Domestic`) only when its sum is
    // here, as a handler body's lowering does from the context's own view.
    for unit in unit_consumes.get(target_name).into_iter().flatten() {
        if let Some(t) = unit_tables.get(unit) {
            for (n, d) in &t.types {
                types.entry(n.clone()).or_insert_with(|| d.clone());
            }
        }
    }
    for (n, c, _) in platform_seams(target_name, unit_tables) {
        caps.entry(n.clone()).or_insert_with(|| c.clone());
    }
    let capability_items: Vec<CommonsItem> =
        caps.into_values().map(CommonsItem::Capability).collect();
    // Pull in names that come into scope via the target's `uses` clauses, so
    // the test-body lowering's static-call check (`<Type>.of(...)` etc.)
    // resolves against the same set of names the source can mention.
    if let Some(used) = unit_uses.get(target_name) {
        for u in used {
            if let Some(t) = unit_tables.get(u) {
                for (n, d) in &t.types {
                    types.entry(n.clone()).or_insert_with(|| d.clone());
                }
                for (n, f) in &t.fns {
                    fns.entry(n.clone()).or_insert_with(|| f.clone());
                }
                for (n, mt) in &t.methods {
                    let entry = methods.entry(n.clone()).or_default();
                    for (m, decl) in &mt.instance {
                        entry
                            .instance
                            .entry(m.clone())
                            .or_insert_with(|| decl.clone());
                    }
                    for (m, decl) in &mt.statics {
                        entry
                            .statics
                            .entry(m.clone())
                            .or_insert_with(|| decl.clone());
                    }
                }
            }
        }
    }
    // #1814: and the types the target's privileged view reaches (`hidden`,
    // each with its owning commons), which the case was checked against,
    // with their methods.
    for (name, owner) in hidden {
        let Some(owner_table) = unit_tables.get(owner) else {
            continue;
        };
        if let Some(d) = owner_table.types.get(name) {
            types.entry(name.clone()).or_insert_with(|| d.clone());
        }
        if let Some(mt) = owner_table.methods.get(name) {
            methods.entry(name.clone()).or_insert_with(|| mt.clone());
        }
    }
    checker::TypedCommons {
        commons: Commons {
            name: QualifiedName {
                parts: target_name
                    .split('.')
                    .map(|p| Ident {
                        name: p.to_string(),
                        span: Span::default(),
                    })
                    .collect(),
                span: Span::default(),
            },
            items: capability_items,
            uses: Vec::new(),
            documentation: None,
            form: CommonsForm::Brace,
            span: Span::default(),
            trivia: Trivia::default(),
            trailing_comments: Vec::new(),
        },
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
        shadowed_imports: HashMap::new(),
        extensible_enums: Default::default(),
        stability: Default::default(),
    }
}

/// `undefined as unknown as <ty>` — the placeholder value for an un-stubbed
/// capability/surface entry, shared by every branch below.
fn undefined_as_unknown_as(ty: impl Into<String>) -> TsExpr {
    TsExpr::As {
        expr: Box::new(TsExpr::As {
            expr: Box::new(ident("undefined")),
            ty: TsType::named("unknown"),
        }),
        ty: TsType::named(ty),
    }
}

/// #291: the platform capabilities `target` flattens in from a consumed adapter
/// (`consumes bynk { Logger }`) — seams of the unit under test, observable and
/// stubbable like its own — as `(capability, declaration, owning adapter)`.
fn platform_seams<'a>(
    target: &str,
    unit_tables: &'a HashMap<String, UnitTable>,
) -> Vec<(&'a String, &'a bynk_syntax::ast::CapabilityDecl, &'a String)> {
    let mut out: Vec<_> = unit_tables
        .get(target)
        .into_iter()
        .flat_map(|t| t.flattened_caps.iter())
        .filter_map(|(cap, owner)| {
            let t = unit_tables.get(owner)?;
            if t.kind != Some(UnitKind::Adapter) {
                return None;
            }
            Some((cap, t.capabilities.get(cap)?, owner))
        })
        .collect();
    out.sort_by_key(|(cap, _, _)| cap.as_str());
    out
}

/// #291: does any of `rp`'s clauses apply to some runs only? Its stub class
/// then carries the running case's name, and the deps factory passes it.
fn stub_is_scoped(rp: &ResolvedStub) -> bool {
    rp.clause_scopes.iter().any(|s| *s != StubScope::Every)
}

/// #1860: does any of `rp`'s clauses apply to one suite's runs? Its stub
/// class then also carries the running suite's ordinal.
fn stub_is_suite_scoped(rp: &ResolvedStub) -> bool {
    rp.clause_scopes
        .iter()
        .any(|s| matches!(s, StubScope::Suite(_)))
}

/// #291/#1860: a stub class's test that the running case or suite is one of
/// `scopes` (none of them [`StubScope::Every`]): `this.__case === "a" ||
/// this.__suite === 1`, cases in name order and then suites.
fn scopes_test(scopes: &[&StubScope]) -> String {
    let mut cases: BTreeSet<&String> = BTreeSet::new();
    let mut suites: BTreeSet<usize> = BTreeSet::new();
    for scope in scopes {
        match scope {
            StubScope::Case(c) => {
                cases.insert(c);
            }
            StubScope::Suite(n) => {
                suites.insert(*n);
            }
            StubScope::Every => {}
        }
    }
    cases
        .into_iter()
        .map(|c| {
            format!(
                "this.__case === {}",
                bynk_ts::print_expr(&str_lit(c.as_str()))
            )
        })
        .chain(suites.into_iter().map(|n| format!("this.__suite === {n}")))
        .collect::<Vec<_>>()
        .join(" || ")
}

/// #291: a stubbed capability's `__makeTestDeps` entry — its `__Stub_<Cap>`
/// layered over `base`, the tier default (the context's own provider, a
/// platform capability's test double, or nothing). A stubbed operation answers
/// from the stub; every other operation reaches the base, so `stub` overrides
/// one operation and leaves the rest of the seam working (case `stub` > suite
/// `stub` > the tier default).
fn stub_overlay(rp: &ResolvedStub, ty: &str, base: Option<TsExpr>) -> TsExpr {
    let cap = &rp.cap;
    // A class with case-scoped clauses is told which case is running, and one
    // with suite-scoped clauses which suite (#1860).
    let mut args = Vec::new();
    if stub_is_scoped(rp) {
        args.push(ident("__case"));
    }
    if stub_is_suite_scoped(rp) {
        args.push(ident("__suite"));
    }
    TsExpr::As {
        expr: Box::new(call(
            ident("__bynkOverlay"),
            vec![
                base.unwrap_or_else(|| ident("undefined")),
                TsExpr::New {
                    callee: Box::new(ident(format!("__Stub_{cap}"))),
                    args,
                },
                str_lit(cap.as_str()),
            ],
        )),
        ty: TsType::named(ty),
    }
}

/// #291: `__bynkOverlay`, emitted into a test module that has a `stub`.
const STUB_OVERLAY_TS: &str = "\
function __bynkOverlay(base: unknown, stub: object, cap: string): unknown {
  return new globalThis.Proxy(stub, {
    get(target, prop) {
      const t = target as Record<PropertyKey, unknown> & { __applies?: (op: string) => boolean };
      // An own property (the stub's state, or a wrapper the recording proxy
      // installed) always answers; a stubbed operation answers when it applies
      // to the running case.
      if (globalThis.Object.prototype.hasOwnProperty.call(t, prop)) return t[prop];
      if (prop in t && (t.__applies === undefined || typeof prop !== \"string\" || t.__applies(prop))) return t[prop];
      if (base !== undefined) {
        const v = (base as Record<PropertyKey, unknown>)[prop];
        return typeof v === \"function\" ? v.bind(base) : v;
      }
      return () => {
        throw new globalThis.Error(`bynk: ${cap}.${String(prop)} is not stubbed, and the capability has no provider in this test`);
      };
    },
  });
}
";

/// P1 (#2590): the body of `bynk.cloudflare`'s `Kv` unit-tier double
/// ([`platform_double`]). Expiry is kept and listed, not enforced, as on the
/// integration tier, on the clock `Clock.now()` reads (`__NOW__`, substituted
/// as `RateLimit`'s is).
const KV_DOUBLE_BODY: &str = r#"  type Entry = { bytes: Uint8Array; metadata: globalThis.Map<string, string>; expiration: number | undefined };
  type Expiration = { tag: string; ttl?: number; instant?: number };
  const store = new globalThis.Map<string, Entry>();
  const enc = new globalThis.TextEncoder();
  const dec = new globalThis.TextDecoder();
  const fault = (why: string): never => {
    throw new globalThis.Error(`bynk.cloudflare.Kv: ${why}`);
  };
  const checkKey = (key: string) => {
    const n = enc.encode(key).length;
    if (n > 512) fault(`a key is at most 512 bytes, not ${n}`);
  };
  const expiryOf = (e: Expiration): number | undefined => {
    if (e.tag === "After") {
      const ttl = e.ttl ?? 0;
      if (globalThis.Math.ceil(ttl / 1000) < 60) fault(`an expiry is at least 60 seconds away, not ${ttl} ms`);
      return __NOW__ + globalThis.Math.ceil(ttl / 1000) * 1000;
    }
    if (e.tag === "At") {
      const instant = e.instant ?? 0;
      const away = globalThis.Math.ceil(instant / 1000) - __NOW__ / 1000;
      if (away < 60) fault(`an expiry is at least 60 seconds away, not ${globalThis.Math.floor(away)} s`);
      return globalThis.Math.ceil(instant / 1000) * 1000;
    }
    return undefined;
  };
  const write = (key: string, bytes: Uint8Array, options: { expiration: Expiration; metadata: ReadonlyMap<string, string> }) => {
    checkKey(key);
    const size = enc.encode(globalThis.JSON.stringify(globalThis.Object.fromEntries(options.metadata))).length;
    if (size > 1024) fault(`metadata is at most 1024 bytes as JSON, not ${size}`);
    store.set(key, { bytes, metadata: new globalThis.Map(options.metadata), expiration: expiryOf(options.expiration) });
  };
  const plain = (bytes: Uint8Array): Entry => ({ bytes, metadata: new globalThis.Map<string, string>(), expiration: undefined });
  return {
    async get(key) {
      const e = store.get(key);
      return e === undefined ? None : Some(dec.decode(e.bytes));
    },
    async put(key, value) {
      store.set(key, plain(enc.encode(value)));
    },
    // Unchecked, as `putTtl` always was; its expiry is kept and listed, as
    // the platform and the integration tier do.
    async putTtl(key, value, ttlSeconds) {
      store.set(key, { ...plain(enc.encode(value)), expiration: __NOW__ + ttlSeconds * 1000 });
    },
    async delete(key) {
      store.delete(key);
    },
    async list(prefix) {
      return [...store.keys()].filter((k) => prefix.tag === "None" || k.startsWith(prefix.value)).sort();
    },
    async getWith(key, options) {
      checkKey(key);
      if (options.cacheTtl.tag === "Some" && globalThis.Math.ceil(options.cacheTtl.value / 1000) < 30) {
        fault(`a cacheTtl is at least 30 seconds, not ${options.cacheTtl.value} ms`);
      }
      const e = store.get(key);
      return e === undefined ? None : Some({ value: dec.decode(e.bytes), metadata: new globalThis.Map(e.metadata) });
    },
    async putWith(key, value, options) {
      write(key, enc.encode(value), options);
    },
    async getBytes(key) {
      checkKey(key);
      const e = store.get(key);
      return e === undefined ? None : Some(e.bytes.slice());
    },
    async putBytes(key, value, options) {
      write(key, value.slice(), options);
    },
    async listPage(options) {
      const limit = options.limit.tag === "Some" ? options.limit.value : 1000;
      if (!globalThis.Number.isInteger(limit) || limit < 1 || limit > 1000) fault(`a page holds 1 to 1000 keys, not ${limit}`);
      const after = options.cursor.tag === "Some" ? options.cursor.value : undefined;
      const names = [...store.keys()]
        .filter((k) => (options.prefix.tag === "None" || k.startsWith(options.prefix.value)) && (after === undefined || k > after))
        .sort();
      const page = names.slice(0, limit);
      return {
        keys: page.map((name) => {
          const e = store.get(name)!;
          return { name, expiration: e.expiration === undefined ? None : Some(e.expiration), metadata: new globalThis.Map(e.metadata) };
        }),
        cursor: page.length < names.length ? Some(page[page.length - 1]) : None,
      };
    },
  };
"#;

/// P5 (#2323): the body of `bynk.cloudflare`'s `R2` unit-tier double
/// ([`platform_double`]).
const R2_DOUBLE_BODY: &str = r#"  const store = new globalThis.Map<string, any>();
  let version = 0;
  const meta = (key: string) => {
    const e = store.get(key);
    return { key, size: e.body.length, etag: e.etag, uploaded: 0, contentType: e.contentType, metadata: e.metadata };
  };
  const write = (key: string, body: Uint8Array, contentType: any, metadata: any) => {
    version += 1;
    store.set(key, { body: body.slice(), etag: `etag-${version}`, contentType, metadata });
    return meta(key);
  };
  return {
    async get(key) {
      const e = store.get(key);
      return e === undefined ? None : Some(e.body.slice());
    },
    async head(key) {
      return store.has(key) ? Some(meta(key)) : None;
    },
    async put(key, body) {
      return write(key, body, None, new globalThis.Map());
    },
    async putWith(key, body, options) {
      const c = options.onlyIf;
      if (c.tag === "Absent" && store.has(key)) return None;
      if (c.tag === "EtagMatches" && store.get(key)?.etag !== c.etag) return None;
      return Some(write(key, body, options.contentType, options.metadata));
    },
    async delete(key) {
      store.delete(key);
    },
    async list(prefix, cursor) {
      const keys = [...store.keys()]
        .filter((k) => (prefix.tag === "None" || k.startsWith(prefix.value)) && (cursor.tag === "None" || k > cursor.value))
        .sort();
      const page = keys.slice(0, 1000);
      return { keys: page.map(meta), cursor: keys.length > page.length ? Some(page[page.length - 1]) : None };
    },
  };
"#;

/// P2 (#2578): the body of `bynk.cloudflare`'s `Queue` unit-tier double
/// ([`platform_double`]).
const QUEUE_DOUBLE_BODY: &str = r#"  const json = (body: string) => {
    try {
      globalThis.JSON.parse(body);
    } catch {
      throw new globalThis.Error("bynk.cloudflare.Queue sends JSON text (normally `Json.encode(value)`), and this body is not JSON");
    }
  };
  return {
    async send(body) {
      json(body);
    },
    async sendWith(body, options) {
      json(body);
      if (options.delay.tag === "Some" && (options.delay.value < 0 || globalThis.Math.ceil(options.delay.value / 1000) > 43200)) {
        throw new globalThis.Error(`bynk.cloudflare.Queue: a delay is between 0 seconds and 12 hours, not ${options.delay.value} ms`);
      }
    },
    async sendBatch(bodies) {
      for (const b of bodies) json(b);
    },
  };
"#;

/// P3b (#2582): the body of `bynk.cloudflare`'s `Analytics` unit-tier double
/// ([`platform_double`]), which checks a point as `WorkersAnalytics` does.
const ANALYTICS_DOUBLE_BODY: &str = r#"  const bytes = (text: string) => new globalThis.TextEncoder().encode(text).length;
  const fault = (why: string): never => {
    throw new globalThis.Error(`bynk.cloudflare.Analytics: ${why}`);
  };
  return {
    async write(point) {
      if (point.blobs.length > 20) fault(`a data point has at most 20 blobs, not ${point.blobs.length}`);
      if (point.doubles.length > 20) fault(`a data point has at most 20 doubles, not ${point.doubles.length}`);
      if (point.index.tag === "Some" && bytes(point.index.value) > 96) fault(`an index is at most 96 bytes, not ${bytes(point.index.value)}`);
      const blobBytes = point.blobs.reduce((n: number, b: string) => n + bytes(b), 0);
      if (blobBytes > 16384) fault(`a data point's blobs total at most 16 KB, not ${blobBytes} bytes`);
      if (!point.doubles.every(globalThis.Number.isFinite)) fault("a double is finite (not NaN or an infinity)");
    },
  };
"#;

/// #291: the deterministic test double for platform capability `cap` (owned by
/// first-party unit `owner`, imported in the test module as `ns`), as the
/// definition of a `__bynkTest_<name>()` factory — `name` is `cap`, or a
/// derived capability's own name (S4a, #1966: `capability Sessions = Kv` has
/// `__bynkTest_Sessions()`, a `Kv` double of its own) — or `None` for a capability
/// with no double (a user adapter's). Each double is the same on every run and
/// does no I/O: `Logger` and `Events` do nothing (observation records the
/// calls), `Clock` reads `0`, `Random` counts its UUIDs and draws integers
/// from a fixed-seed generator, `Secrets` has none, `Locale` is `"en"`,
/// `Idempotency` and `Kv` are in-memory (a TTL or `expiresAfter` is accepted
/// and ignored: expiry is not modelled), and `Fetch` faults — a test never
/// reaches the network, so it must `stub Fetch.send` — as does each
/// `bynk.cloudflare` `Service` instance (`stub Billing.send`). `__makeTestDeps`
/// builds fresh ones per case.
///
/// #2137: with `harness_clock` (a module whose cases `advance Clock`), `Clock`
/// reads the harness clock (`emitter/test_runtime/clock.ts`) instead,
/// resetting it to the epoch as each case builds its deps.
fn platform_double(
    owner: &str,
    cap: &str,
    ns: &str,
    name: &str,
    harness_clock: bool,
) -> Option<String> {
    let body = match (owner, cap) {
        // K5a (#2586): every level a no-op, as `info` and `error` are.
        ("bynk", "Logger") => "  return {\n    async info() {},\n    async error() {},\n    async warn() {},\n    async debug() {},\n    async log() {},\n  };\n".to_string(),
        ("bynk", "Events") => "  return {\n    async emit() {},\n  };\n".to_string(),
        ("bynk", "Clock") if harness_clock => {
            "  __bynkClockNow = 0;\n  return __bynkClock;\n".to_string()
        }
        ("bynk", "Clock") => "  return {\n    async now() {\n      return 0;\n    },\n  };\n".to_string(),
        ("bynk", "Random") => "  let n = 0;\n  let seed = 0x2545f491;\n  return {\n    async uuid() {\n      n += 1;\n      return `00000000-0000-4000-8000-${n.toString(16).padStart(12, \"0\")}` as never;\n    },\n    async int(lo, hi) {\n      seed = (globalThis.Math.imul(seed, 1664525) + 1013904223) >>> 0;\n      return hi > lo ? lo + (seed % (hi - lo)) : lo;\n    },\n    async bytes(n) {\n      if (!globalThis.Number.isInteger(n) || n < 1 || n > 65536) {\n        throw new globalThis.Error(`Random.bytes(n) requires 1 <= n <= 65536, got ${n}`);\n      }\n      const out = new globalThis.Uint8Array(n);\n      for (let i = 0; i < n; i++) {\n        seed = (globalThis.Math.imul(seed, 1664525) + 1013904223) >>> 0;\n        out[i] = seed >>> 24;\n      }\n      return out;\n    },\n  };\n".to_string(),
        // K4: `bynk.crypto` has no fake. Digests, MACs and signatures are
        // deterministic for given inputs (Ed25519 and RSASSA-PKCS1-v1_5
        // included), so the double is the real provider, from the binding the
        // module imports as `bynk_crypto__binding`.
        ("bynk.crypto", "Crypto") => "  return new bynk_crypto__binding.CryptoProvider();\n".to_string(),
        ("bynk", "Secrets") => "  return {\n    async get() {\n      return None;\n    },\n  };\n".to_string(),
        ("bynk", "Locale") => "  return {\n    async current() {\n      return \"en\" as never;\n    },\n  };\n".to_string(),
        ("bynk", "Fetch") => "  return {\n    async send() {\n      throw new globalThis.Error(\"bynk: `Fetch.send` is not stubbed, and a test never reaches the network — add `stub Fetch.send(_) returns …`\");\n    },\n    async sendWith() {\n      throw new globalThis.Error(\"bynk: `Fetch.sendWith` is not stubbed, and a test never reaches the network — add `stub Fetch.sendWith(_, _) returns …`\");\n    },\n    async receiveBytes() {\n      throw new globalThis.Error(\"bynk: `Fetch.receiveBytes` is not stubbed, and a test never reaches the network — add `stub Fetch.receiveBytes(_, _) returns …`\");\n    },\n    async sendBytes() {\n      throw new globalThis.Error(\"bynk: `Fetch.sendBytes` is not stubbed, and a test never reaches the network — add `stub Fetch.sendBytes(_, _, _) returns …`\");\n    },\n  };\n".to_string(),
        ("bynk", "Idempotency") => "  const store = new globalThis.Map<string, unknown>();\n  return {\n    async dedup(key) {\n      return (store.has(key) ? Some(store.get(key)) : None) as never;\n    },\n    async remember(key, value) {\n      store.set(key, value);\n    },\n  };\n".to_string(),
        // P1 (#2590): values are bytes (text is its UTF-8, so the two views
        // agree as on the platform), with metadata and an expiry beside them;
        // the new operations check the platform's limits as `WorkersKv` does.
        // Expiry reads the clock `Clock.now()` does, so an `At` built from
        // it is judged on the same timeline (the harness clock at `unit`).
        ("bynk.cloudflare", "Kv") => KV_DOUBLE_BODY.replace(
            "__NOW__",
            if harness_clock { "__bynkClockNow" } else { "0" },
        ),
        // P5 (#2323): each `R2` instance is an in-memory bucket honouring
        // `putWith`'s conditions, its metadata and `list`'s paging (R2's own
        // page of 1000, the cursor being the last key). `uploaded` reads `0`,
        // as `Clock` does, and an etag is a per-bucket counter.
        ("bynk.cloudflare", "R2") => R2_DOUBLE_BODY.to_string(),
        // P3 (#2342): each `RateLimit` instance counts calls per key over
        // fixed windows of its `@period`, on the harness clock when the
        // module's cases `advance Clock` (else time stands still at 0), so a
        // case can exhaust a limiter and, by advancing, recover. The bare
        // capability's double (a `consumes … { RateLimit }` entry, never
        // usable bare) is built without them, and never refuses.
        ("bynk.cloudflare", "RateLimit") => {
            let now = if harness_clock { "__bynkClockNow" } else { "0" };
            return Some(format!(
                "function __bynkTest_{name}(limit: number = globalThis.Number.POSITIVE_INFINITY, period: number = 60): {ns}.{cap} {{\n  const counts = new globalThis.Map<string, {{ window: number; count: number }}>();\n  return {{\n    async allow(key) {{\n      const window = globalThis.Math.floor({now} / (period * 1000));\n      const e = counts.get(key);\n      const c = e !== undefined && e.window === window ? e : {{ window, count: 0 }};\n      c.count += 1;\n      counts.set(key, c);\n      return c.count <= limit;\n    }},\n  }};\n}}\n"
            ));
        }
        // L7 (#2414): there is no edge cache in a test, so each instance
        // accepts every purge, checking its prefixes as the binding does;
        // `stub Edge.purge(_) returns false` refuses one, and observation
        // sees what was purged.
        ("bynk.cloudflare", "EdgeCache") => "  return {\n    async purge(prefixes) {\n      for (const p of prefixes) {\n        if (!p.startsWith(\"/\") || /[\\u0000-\\u001f\\u007f?#]/.test(p)) throw new globalThis.Error(`bynk.cloudflare.EdgeCache.purge takes paths starting with \\`/\\`, with no query, fragment or control characters; got ${globalThis.JSON.stringify(p)}`);\n      }\n      return true;\n    },\n  };\n".to_string(),
        // P2 (#2578): there is no queue in a test, so each instance accepts
        // every send, checking its bodies are JSON (and a delay in range) as
        // the binding does; observation sees what was sent.
        ("bynk.cloudflare", "Queue") => QUEUE_DOUBLE_BODY.to_string(),
        // P3b (#2582): there is no dataset in a test, so each instance accepts
        // every write within the platform's limits; observation (the generic
        // call wrapper, as for every double) sees what was written.
        ("bynk.cloudflare", "Analytics") => ANALYTICS_DOUBLE_BODY.to_string(),
        // P4a (#2602): each `D1` instance is the real `WorkersD1` over its own
        // empty in-memory SQLite database (`node:sqlite`), so a test runs its
        // statements as D1 would; the case creates its tables.
        ("bynk.cloudflare", "D1") => "  return new bynk_cloudflare__binding.WorkersD1({ D1: __makeIntegrationD1() }, \"D1\");\n".to_string(),
        // Interop I5 (#1972): like `Fetch`, a test never reaches the foreign
        // Worker, so each instance faults until its `send` is stubbed.
        ("bynk.cloudflare", "Service") => format!("  return {{\n    async send() {{\n      throw new globalThis.Error(\"bynk: `{name}.send` is not stubbed, and a test never reaches the Worker behind it — add `stub {name}.send(_) returns …`\");\n    }},\n  }};\n"),
        _ => return None,
    };
    Some(format!(
        "function __bynkTest_{name}(): {ns}.{cap} {{\n{body}}}\n"
    ))
}

/// #2134: the `Secrets` test double of a module whose cases run with `env`
/// entries: it answers `Some(value)` for a name the running case's `env`
/// gives (`__bynkEnv`), and `None` for any other, as [`platform_double`]'s
/// does for every name. `None` for any other capability.
fn env_secrets_double(owner: &str, cap: &str, ns: &str, name: &str) -> Option<String> {
    if (owner, cap) != ("bynk", "Secrets") {
        return None;
    }
    Some(format!(
        "function __bynkTest_{name}(): {ns}.{cap} {{\n  return {{\n    async get(key) {{\n      const v = __bynkEnv[key];\n      return v === undefined ? None : Some(v);\n    }},\n  }};\n}}\n"
    ))
}

/// #1479: returns the real [`TsStmt`] itself (was pre-printed `String`) — the
/// one real declaration this function ever built; its caller now prints it
/// directly via `bynk_ts::print_stmt`, the same shape it always used, just
/// one call further out.
///
/// #1863: a provider is built as the composition root builds it
/// ([`instantiate_provider_ts_expr`]), its own `given` capabilities wired in,
/// whether the context declares it or a context it consumes does. The units
/// that construction names are returned, for the test module to import: a
/// provider's `given` can reach a unit the target doesn't consume itself.
///
/// Unlike the composition root, a provider's `given` that is also one of the
/// deps' own entries is that entry, held in a local, not a second copy: in a
/// test a `stub` overlays the entry, and a copy would escape it (#1864
/// review).
#[allow(clippy::too_many_arguments)]
fn emit_test_deps(
    target_name: &str,
    target_kind: UnitKind,
    stubs: &HashMap<String, ResolvedStub>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    // #1975: the adapters whose binding this factory builds for real, by
    // binding module (an adapter suite, or the `integration` tier's factory);
    // `None` keeps every external provider the placeholder.
    bindings: Option<&HashMap<String, String>>,
    // The factory's name: `__makeTestDeps`, or `__makeIntegrationDeps`.
    name: &str,
    // #2050: whether the target's handlers emit, so their `deps` require
    // `__eventsDispatch` ([`target_uses_emit`]).
    uses_emit: bool,
) -> TestDeps {
    let mut units: BTreeSet<String> = BTreeSet::new();
    let mut doubles: Vec<(String, String, String)> = Vec::new();
    // #1975: a real binding's own `given` that the target does not reach
    // takes the tier's test double, recorded so the module defines it.
    let given_doubles: std::cell::RefCell<Vec<(String, String, String)>> =
        std::cell::RefCell::new(Vec::new());
    let given_double = |owner: &str, cap: &str| -> Option<TsExpr> {
        bindings?;
        platform_double(owner, cap, &test_ns(owner), cap, false)?;
        given_doubles
            .borrow_mut()
            .push((owner.to_string(), cap.to_string(), cap.to_string()));
        Some(call(ident(format!("__bynkTest_{cap}")), Vec::new()))
    };
    // Each top-level entry: its deps key, the `(unit, capability)` it is, and
    // how its value is built.
    let mut tops: Vec<(String, (String, String), TopDep)> = Vec::new();
    let mut surface_entries: Vec<(String, TsExpr)> = Vec::new();
    // #1975: an adapter target's deps are its own capabilities, built from
    // its binding: the binding is the unit under test.
    if matches!(target_kind, UnitKind::Context | UnitKind::Adapter)
        && let Some(table) = unit_tables.get(target_name)
    {
        // Sorted so `__makeTestDeps` field order is deterministic across the
        // capability map's hash iteration order.
        let mut caps: Vec<&String> = table.capabilities.keys().collect();
        caps.sort();
        for cap in caps {
            // v0.118: a capability with a `stub` override plugs its
            // `__Stub_<Cap>` stub; otherwise the declared provider (its real
            // implementation) is used, as an un-overridden seam.
            let how = if table.providers.contains_key(cap) {
                TopDep::Provider
            } else if let Some(inst) = table.derived.get(cap) {
                // S4a (#1966): a derived capability's instance has its base's
                // double, its own fresh one.
                platform_or_placeholder(&inst.base_unit, &inst.base_cap)
            } else {
                TopDep::Placeholder
            };
            tops.push((cap.clone(), (target_name.to_string(), cap.clone()), how));
        }
        // v0.17 (Locale capability track, slice 1, #844): a capability
        // flattened in via `consumes U { Cap }` (e.g. an adapter's `Locale`)
        // is never in `table.capabilities` above — that holds only
        // capabilities this unit declares itself. Its real implementation is
        // wired by production `compose()` from a platform binding the test
        // module never imports, so an un-stubbed one is the placeholder,
        // exactly like a locally-declared capability with no provider — or,
        // #291, a platform capability's deterministic test double.
        let flattened = unit_flattened.get(target_name).cloned().unwrap_or_default();
        let mut flat: Vec<(&String, &String)> = flattened.iter().collect();
        flat.sort_by_key(|(cap, _)| cap.as_str());
        for (cap, owner) in flat {
            // #1975: an adapter's capability whose binding this factory
            // builds is its real provider.
            let how = if bindings.is_some_and(|b| b.contains_key(owner)) {
                TopDep::Provider
            } else {
                platform_or_placeholder(owner, cap)
            };
            tops.push((cap.clone(), (owner.clone(), cap.clone()), how));
        }
        // #1863: a capability another context provides, named with that
        // context's prefix (`given Fees.Fees`), as the composition root wires
        // it (`handler_cross_caps`, which also lists the flattened ones
        // above): that context's provider, keyed by the capability's name,
        // or a first-party platform capability's test double.
        let consumed_all: Vec<String> = unit_consumes.get(target_name).cloned().unwrap_or_default();
        let aliases = unit_consumes_aliases
            .get(target_name)
            .cloned()
            .unwrap_or_default();
        for (cap, owner) in handler_cross_caps(table, &consumed_all, &aliases, &flattened) {
            if tops.iter().any(|(k, _, _)| *k == cap) {
                continue;
            }
            let how = match platform_or_placeholder(&owner, &cap) {
                TopDep::Placeholder => TopDep::Provider,
                double => double,
            };
            tops.push((cap.clone(), (owner, cap), how));
        }
        // Cross-context surface: consumed contexts run with their real surface
        // (v0.118 `stub` is capability-only — a consumed-context capability
        // flattened via `consumes U { Cap }` is folded in via `unit_flattened`
        // above). An `adapter` target (e.g. `consumes bynk { Locale }`) has no
        // `__makeSurface` at all — so it must not get a surface entry either
        // (Locale capability track, slice 1, #844), nor does a context with
        // no services, consumed only for a capability (#1863).
        let mut alias_for_target: HashMap<String, String> = HashMap::new();
        for (alias, q) in &aliases {
            alias_for_target.insert(q.clone(), alias.clone());
        }
        for q in consumed_all.iter().filter(|q| {
            unit_tables
                .get(*q)
                .is_some_and(|t| t.kind != Some(UnitKind::Adapter) && !t.services.is_empty())
        }) {
            let key = alias_for_target
                .get(q)
                .cloned()
                .unwrap_or_else(|| q.rsplit('.').next().unwrap_or(q.as_str()).to_string());
            let other_ns = test_ns(q);
            surface_entries.push((
                key,
                undefined_as_unknown_as(format!(
                    "globalThis.ReturnType<typeof {other_ns}.__makeSurface>"
                )),
            ));
        }
    }

    // #1864 review: build every top-level value with each top-level
    // capability named by a local, so a provider whose `given` reaches one
    // takes that local, stub and all, rather than a fresh copy. The locals a
    // construction used become `const`s ahead of the `return`, each after
    // the ones its own construction used; every other value stays inline.
    let prebuilt: HashMap<(String, String), String> = tops
        .iter()
        .map(|(key, id, _)| (id.clone(), format!("__dep_{key}")))
        .collect();
    let mut built: Vec<BuiltDep> = Vec::new();
    for (key, (unit, cap), how) in &tops {
        let used = std::cell::RefCell::new(BTreeSet::new());
        let ty = format!("{}.{cap}", test_ns(unit));
        let base = match how {
            TopDep::Provider => Some(instantiate_provider_ts_expr(
                unit,
                cap,
                unit_tables,
                unit_consumes,
                unit_consumes_aliases,
                unit_flattened,
                ProviderNamespaces::Test {
                    prebuilt: &prebuilt,
                    used: &used,
                    bindings,
                    double: &given_double,
                },
                None,
                None,
                &mut units,
            )),
            TopDep::Double => {
                // A derived capability's double is its base's.
                let derived = unit_tables.get(unit).and_then(|t| t.derived.get(cap));
                let (owner, base) = derived.map_or((unit, cap), |d| (&d.base_unit, &d.base_cap));
                doubles.push((owner.clone(), base.clone(), cap.clone()));
                // P3 (#2342): a rate limiter's double is told its instance's
                // limit and window.
                let args = match derived.and_then(|d| d.rate_limit()) {
                    Some((limit, period)) => {
                        vec![num_lit(limit.to_string()), num_lit(period.to_string())]
                    }
                    None => Vec::new(),
                };
                Some(call(ident(format!("__bynkTest_{cap}")), args))
            }
            TopDep::Placeholder => None,
        };
        let value = match stubs.get(key) {
            Some(rp) => stub_overlay(rp, &ty, base),
            None => base.unwrap_or_else(|| undefined_as_unknown_as(ty)),
        };
        let id = (unit.clone(), cap.clone());
        built.push(BuiltDep {
            key: key.clone(),
            local: prebuilt[&id].clone(),
            id,
            value,
            used: used.into_inner(),
        });
    }
    let shared: BTreeSet<(String, String)> =
        built.iter().flat_map(|b| b.used.iter().cloned()).collect();
    let mut body: Vec<TsStmt> = Vec::new();
    let mut declared: BTreeSet<(String, String)> = BTreeSet::new();
    // Dependencies first: no provider chain is cyclic (the checker rejects
    // one), so this terminates.
    fn declare(
        id: &(String, String),
        built: &[BuiltDep],
        declared: &mut BTreeSet<(String, String)>,
        body: &mut Vec<TsStmt>,
    ) {
        if declared.contains(id) {
            return;
        }
        let Some(dep) = built.iter().find(|b| b.id == *id) else {
            return;
        };
        for used in &dep.used {
            declare(used, built, declared, body);
        }
        declared.insert(id.clone());
        body.push(TsStmt::const_stmt(
            TsBindingName::Ident(dep.local.clone()),
            None,
            dep.value.clone(),
            None,
        ));
    }
    for dep in &built {
        if shared.contains(&dep.id) {
            declare(&dep.id, &built, &mut declared, &mut body);
        }
    }
    let mut entries: Vec<(String, TsExpr)> = built
        .into_iter()
        .map(|dep| {
            let value = if shared.contains(&dep.id) {
                ident(dep.local)
            } else {
                dep.value
            };
            (dep.key, value)
        })
        .collect();
    if !surface_entries.is_empty() {
        entries.push(("surface".to_string(), TsExpr::object(surface_entries)));
    }
    // #1994: the execution context a `~>`-sending handler hands its promise to.
    if target_uses_send(target_name, target_kind, unit_tables) {
        entries.push(("__exec".to_string(), call(ident("__bynkExec"), Vec::new())));
    }
    let mut entries: Vec<TsObjectEntry> = entries
        .into_iter()
        .map(|(k, v)| TsObjectEntry::Prop(k, v))
        .collect();
    // #2050: the release-at-commit flush of an emitting handler — a service's,
    // or an agent's, which runs on its caller's `deps` — calls
    // `deps.__eventsDispatch`. In a test it hands each event to the `Events`
    // double, which records the call and delivers nothing, so `expect
    // Events.emit called …` observes what the handler released. A method
    // reading `this.Events` rather than a captured value: the recording
    // wrapper lands on the deps object after this factory runs.
    if uses_emit {
        entries.push(TsObjectEntry::Method {
            name: "__eventsDispatch".to_string(),
            is_async: true,
            generics: Vec::new(),
            params: vec![TsParam {
                name: "events".to_string(),
                ty: Some(TsType::named_with_args(
                    "globalThis.Array",
                    vec![TsType::named(crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE)],
                )),
                optional: false,
            }],
            return_type: None,
            doc: None,
            inline: true,
            body: vec![TsStmt::raw(
                "for (const ev of events) await this.Events.emit(ev.payload);",
                None,
            )],
        });
    }
    // The same `"{  }"` double-space quirk `workers.rs`/`project.rs`/
    // `emit.rs`/`gen_ts_for_ty` (this file, Arc C slice 31) already carry as
    // opaque text: the pre-conversion `format!("  return {{ {} }};\n",
    // entries.join(", "))` template always has a space on each side of its
    // `{}` slot, so zero entries (a non-`Context` target, or a `Context`
    // with no capabilities/flattened caps/consumed surface) literally
    // produced a double space, not the tight `"{}"` `TsExpr::object`'s own
    // empty-entries shortcut renders — a real, reachable shape (an
    // integration target's own non-`Context` participants all hit it).
    let case_scoped = stubs.values().any(stub_is_scoped);
    let suite_scoped = stubs.values().any(stub_is_suite_scoped);
    let return_value = if entries.is_empty() {
        ident("{  }")
    } else {
        TsExpr::object_entries(entries)
    };
    body.push(TsStmt::return_stmt(Some(return_value), None));
    doubles.extend(given_doubles.into_inner());
    let stmt = TsStmt::decl(
        TsDecl::Function {
            name: name.to_string(),
            generics: Vec::new(),
            // #291: the running case's name, when some `stub` is case-scoped,
            // and #1860 the running suite's ordinal, when one is suite-scoped.
            params: [
                ("__case", "string", case_scoped),
                ("__suite", "number", suite_scoped),
            ]
            .into_iter()
            .filter(|(_, _, on)| *on)
            .map(|(name, ty, _)| TsParam {
                name: name.to_string(),
                ty: Some(TsType::named(ty)),
                optional: true,
            })
            .collect(),
            return_type: None,
            body,
            is_async: false,
            inline: false,
        },
        None,
    );
    TestDeps {
        stmt,
        units,
        doubles,
    }
}

/// What [`emit_test_deps`] built (#1863).
struct TestDeps {
    /// The `__makeTestDeps` declaration.
    stmt: TsStmt,
    /// The units its providers name, which the test module imports.
    units: BTreeSet<String>,
    /// Each `(owner, capability, name)` whose `__bynkTest_<name>` test double
    /// it calls, which the test module defines (`platform_double`): `name` is
    /// the capability, or a derived capability over it (S4a, #1966).
    doubles: Vec<(String, String, String)>,
}

/// One top-level test dep, built ([`emit_test_deps`]).
struct BuiltDep {
    /// Its deps key: the capability's name.
    key: String,
    /// The `(unit, capability)` it is.
    id: (String, String),
    /// The local that holds it, when another dep is built over it.
    local: String,
    /// Its value, `stub` overlay included.
    value: TsExpr,
    /// The other deps' locals its construction names.
    used: BTreeSet<(String, String)>,
}

/// How a top-level test dep's value is built, before any `stub` overlay.
enum TopDep {
    /// Its provider, through [`instantiate_provider_ts_expr`].
    Provider,
    /// A platform capability's deterministic test double (#291).
    Double,
    /// The `undefined` placeholder: no provider the test module can build.
    Placeholder,
}

/// A capability of `owner` with a platform test double is [`TopDep::Double`];
/// any other, the placeholder, whatever provides it in production.
fn platform_or_placeholder(owner: &str, cap: &str) -> TopDep {
    if platform_double(owner, cap, &test_ns(owner), cap, false).is_some() {
        TopDep::Double
    } else {
        TopDep::Placeholder
    }
}

/// #18 (testing-track infra): a real value destructure plus a per-type alias,
/// instead of one `const { … } = ns as any`. Every bynk-declared type pairs an
/// `export type`/`export interface` with a companion runtime value
/// (`export const X = {...}`, a refinement's `of`/`unsafe`, a sum's variant
/// constructors, or simply `{}` for a plain record) — but destructuring does
/// not carry the *type* side of that merge into the new local binding, so
/// `const { X } = ns` alone leaves `X` usable only as a value. The blanket
/// `as any` papered over that by disabling type-checking for every
/// destructured name, not just the ones needing the workaround — which is
/// exactly why this generator's own output couldn't be gated by
/// `tsc --strict`: nothing inside a generated test body was ever really
/// checked. `type_names` (a subset of `value_names`) gets the alias;
/// `value_names` is unconditionally destructured with no cast.
/// #1479: returns real [`TsStmt`]s (was `out: &mut String`) — every caller
/// now appends via `crate::emitter::extend_printed_at(out, stmts, 2)`, the
/// same depth-2 this scaffold body's own statements always printed at.
/// #1703: each type name carries its type parameters, so a generic type's
/// alias is generic too (`type Box<A> = ns.Box<A>;`). A bare alias of a generic
/// type is `TS2314` under `tsc`, which failed every suite whose target declared
/// or `uses` one, whether or not a case mentioned it. Build the list with
/// [`aliased_types`].
fn emit_ns_destructure(
    ns: &str,
    value_names: &[String],
    type_names: &[(String, Vec<String>)],
) -> Vec<TsStmt> {
    let mut stmts = Vec::new();
    if !value_names.is_empty() {
        stmts.push(TsStmt::const_stmt(
            TsBindingName::ObjectPattern(value_names.to_vec()),
            None,
            ident(ns),
            None,
        ));
    }
    for (t, params) in type_names {
        let target = format!("{ns}.{t}");
        let ty = if params.is_empty() {
            TsType::named(target)
        } else {
            TsType::named_with_args(target, params.iter().map(TsType::named).collect())
        };
        stmts.push(TsStmt::decl(
            TsDecl::TypeAlias {
                name: t.clone(),
                type_params: params.clone(),
                ty,
            },
            None,
        ));
    }
    stmts
}

/// #1703: the types of `table` that `keep` admits, each with its type
/// parameters, sorted by name: the `type_names` [`emit_ns_destructure`] aliases.
fn aliased_types(table: &UnitTable, keep: impl Fn(&String) -> bool) -> Vec<(String, Vec<String>)> {
    let mut types: Vec<(String, Vec<String>)> = table
        .types
        .iter()
        .filter(|(name, _)| keep(name))
        .map(|(name, decl)| {
            let params = decl
                .type_params
                .iter()
                .map(|p| p.name.name.clone())
                .collect();
            (name.clone(), params)
        })
        .collect();
    types.sort();
    types
}

/// #1814: a suite view's hidden types grouped by owning commons, each group
/// sorted by name.
fn hidden_by_owner(hidden: &BTreeMap<String, String>) -> BTreeMap<String, Vec<String>> {
    let mut by_owner: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, owner) in hidden {
        by_owner
            .entry(owner.clone())
            .or_default()
            .push(name.clone());
    }
    by_owner
}

/// #1814: destructure each hidden type from its owning commons' namespace,
/// as a value and a type alias, the way a `uses` commons' types are: a
/// lowered case spells a reached type (a kernel's `(__x: Repo) => …`), names
/// a reached sum's variants (`Status.Done`) and calls its methods
/// (`Repo.shout(…)`).
fn extend_hidden_destructures(
    out: &mut String,
    hidden: &BTreeMap<String, Vec<String>>,
    unit_tables: &HashMap<String, UnitTable>,
) {
    for (owner, names) in hidden {
        let Some(table) = unit_tables.get(owner) else {
            continue;
        };
        let type_names = aliased_types(table, |n| names.contains(n));
        crate::emitter::extend_printed_at(
            out,
            emit_ns_destructure(&test_ns(owner), names, &type_names),
            2,
        );
    }
}

/// Emit the shared per-runner scope setup — agent reset, the `deps` factory, and
/// the destructurings (see [`emit_ns_destructure`]) that bring the target's,
/// `uses`', and consumed contexts' names into scope. Shared by `case` and
/// `property` runners so a property body resolves names exactly as a case does.
#[allow(clippy::too_many_arguments)]
fn emit_test_scope_setup(
    out: &mut String,
    // #1975: the deps factory the case calls (`__makeTestDeps`, or the
    // `integration` tier's `__makeIntegrationDeps`).
    deps_factory: &str,
    target_name: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    // v0.117: when the body observes (`Cap.op called …` / `trace(Cap.op)`), wrap
    // `deps` with the recording proxy and declare the per-case trace `__obs`. Off
    // for bodies that don't observe, so their emitted output is unchanged.
    record_calls: bool,
    // #291: the running case's name, passed to `__makeTestDeps` when some
    // `stub` clause is scoped to the case (so it applies to it and no other).
    case_name: Option<&str>,
    // #1860: the running suite's ordinal, passed when some `stub` clause is
    // scoped to a suite.
    suite: Option<usize>,
    hidden: &BTreeMap<String, String>,
) -> HashSet<String> {
    // #2024: every name this setup binds in the runner's scope, returned so the
    // body's lowering renames a case `let` that would redeclare one.
    let mut bound: HashSet<String> = ["deps", "__obs"].map(String::from).into();
    let deps_args: Vec<TsExpr> = match suite {
        Some(n) => vec![
            case_name.map_or_else(|| ident("undefined"), str_lit),
            TsExpr::Lit(TsLit::Num(n.to_string())),
        ],
        None => case_name.map(str_lit).into_iter().collect(),
    };
    let target_ns = test_ns(target_name);
    // v0.9.2: reset the target context's agent registries so each test sees a
    // fresh per-key state (finding #10's "fresh per test" half).
    let target_has_agents = unit_tables
        .get(target_name)
        .is_some_and(|t| !t.agents.is_empty());
    if target_has_agents {
        let stmt = TsStmt::expr_stmt(
            method_call(ident(target_ns.clone()), "__resetAgents", Vec::new()),
            None,
        );
        out.push_str(&bynk_ts::print_stmt(&stmt, 2));
    }
    // v0.117: the per-case recorded-call trace, and — for a context target with
    // capabilities — a `deps` wrapped so each capability operation records its
    // calls into `__obs`. Observations and `trace(Cap.op)` in the body read it.
    let obs_spec: Option<TsExpr> = if record_calls && target_kind == UnitKind::Context {
        unit_tables.get(target_name).and_then(|table| {
            // #291: the seams are the target's own capabilities and the platform
            // capabilities it flattens in (`consumes bynk { Logger }`); its own
            // wins on a name clash.
            let mut seams: std::collections::BTreeMap<&String, &bynk_syntax::ast::CapabilityDecl> =
                platform_seams(target_name, unit_tables)
                    .into_iter()
                    .map(|(cap, decl, _)| (cap, decl))
                    .collect();
            seams.extend(table.capabilities.iter());
            if seams.is_empty() {
                return None;
            }
            let entries: Vec<(String, TsExpr)> = seams
                .iter()
                .map(|(c, decl)| {
                    let mut ops: Vec<&String> = decl.ops.iter().map(|o| &o.name.name).collect();
                    ops.sort();
                    (
                        (*c).clone(),
                        TsExpr::array(ops.into_iter().map(str_lit).collect()),
                    )
                })
                .collect();
            Some(TsExpr::object(entries))
        })
    } else {
        None
    };
    if matches!(target_kind, UnitKind::Context | UnitKind::Adapter) {
        if let Some(spec) = obs_spec {
            // P7.2: matches `emitter/lower.rs`'s own `{ args: unknown[] }` reads
            // of this exact shape (`Trace`/`Called`-with-predicate lowering).
            let obs_ty = TsType::named_with_args(
                "globalThis.Record",
                vec![
                    TsType::named("string"),
                    TsType::array(TsType::Object(vec![
                        TsTypeMember::Prop {
                            name: "args".to_string(),
                            ty: TsType::array(TsType::named("unknown")),
                            optional: false,
                            readonly: false,
                        },
                        TsTypeMember::Prop {
                            name: "order".to_string(),
                            ty: TsType::named("number"),
                            optional: false,
                            readonly: false,
                        },
                    ])),
                ],
            );
            let obs_stmt = TsStmt::const_stmt(
                TsBindingName::Ident("__obs".to_string()),
                None,
                TsExpr::object(vec![
                    (
                        "log".to_string(),
                        TsExpr::As {
                            expr: Box::new(TsExpr::object(vec![])),
                            ty: obs_ty,
                        },
                    ),
                    ("n".to_string(), num_lit("0")),
                ]),
                None,
            );
            out.push_str(&bynk_ts::print_stmt(&obs_stmt, 2));
            let deps_stmt = TsStmt::const_stmt(
                TsBindingName::Ident("deps".to_string()),
                None,
                call(
                    ident("__bynkRecordDeps"),
                    vec![
                        call(ident(deps_factory), deps_args.clone()),
                        spec,
                        ident("__obs"),
                    ],
                ),
                None,
            );
            out.push_str(&bynk_ts::print_stmt(&deps_stmt, 2));
        } else {
            let deps_stmt = TsStmt::const_stmt(
                TsBindingName::Ident("deps".to_string()),
                None,
                call(ident(deps_factory), deps_args.clone()),
                None,
            );
            out.push_str(&bynk_ts::print_stmt(&deps_stmt, 2));
        }
    } else {
        let deps_stmt = TsStmt::const_stmt(
            TsBindingName::Ident("deps".to_string()),
            None,
            TsExpr::object(vec![]),
            None,
        );
        out.push_str(&bynk_ts::print_stmt(&deps_stmt, 2));
    }
    // Bring the target's top-level names into local scope so the lowered
    // body can reference them unqualified. The target's types and fns are
    // exported from its namespace by the production emitter.
    if let Some(table) = unit_tables.get(target_name) {
        // #1883, slice 3: the target's assets are exported constants too.
        let mut names: Vec<String> = table
            .types
            .keys()
            .chain(table.fns.keys())
            .chain(table.assets.keys())
            .chain(table.embeds.keys())
            .cloned()
            .collect();
        // For contexts, also bring services and providers into scope.
        let extras: Vec<String> = table
            .services
            .keys()
            .chain(table.agents.keys())
            .cloned()
            .collect();
        names.extend(extras);
        // v0.9.2: bring each agent's construction factory into scope so a test
        // body's `AgentName(key)` lowers to `__makeAgentName(key)`.
        for agent in table.agents.keys() {
            names.push(crate::emitter::agent_factory_name(agent));
        }
        names.sort();
        names.dedup();
        let type_names = aliased_types(table, |_| true);
        bound.extend(names.iter().cloned());
        crate::emitter::extend_printed_at(
            out,
            emit_ns_destructure(&target_ns, &names, &type_names),
            2,
        );
    }
    // Bring in `uses` commons names too — the target's body can use them.
    // message-bundles slice 1 (#859): a name the target itself already
    // declares must be excluded here — the same local-shadows-`uses`
    // precedence `compose_unit_symbols` already applies to production
    // emission (project.rs's `combined_fns`/`combined_types`), which this
    // test-scaffold path builds independently and had never applied. Never
    // exercised before this slice: no prior commons declared a name also
    // present in something it `uses`. Without the filter, a target with its
    // own `render` (a messages block's synthetic one) *and* `uses
    // bynk.locale` (which also declares `render`) destructured both into one
    // scope — `Cannot redeclare block-scoped variable 'render'` under `tsc`.
    let target_local: std::collections::HashSet<&String> = unit_tables
        .get(target_name)
        .map(|t| {
            t.types
                .keys()
                .chain(t.fns.keys())
                .chain(t.assets.keys())
                .chain(t.embeds.keys())
                .collect()
        })
        .unwrap_or_default();
    // #1925: a used commons' function or asset is hidden by any value the
    // target declares — its services, agents and bodied providers too — as
    // the checker's view hides it (`bynk_check::project_model::shadowed_imports`).
    // #1927: so is a used commons' type's *value* (its constructors), which a
    // case may not name (`check_shadowed_type_names_in_suite`). Its type
    // alias (`type X = ns.X`, below) is still bound: it lives in TypeScript's
    // type space, beside the target value's `const`, and the lowering names
    // the types of checked expressions (`(__x: X) => …`, #1778).
    let target_values: std::collections::HashSet<&String> = unit_tables
        .get(target_name)
        .map(|t| {
            target_local
                .iter()
                .copied()
                .chain(t.services.keys())
                .chain(t.agents.keys())
                .chain(
                    t.providers
                        .values()
                        .filter(|p| !p.external)
                        .map(|p| &p.provider_name.name),
                )
                .collect()
        })
        .unwrap_or_default();
    // #1883, slice 3: a name brought in by an earlier used commons is not
    // destructured again (two used commons may not share an asset name —
    // `bynk.uses.name_conflict` — but an asset may share none with a fn either).
    let mut seen_assets: std::collections::HashSet<&String> = std::collections::HashSet::new();
    if let Some(used) = unit_uses.get(target_name) {
        for u in used {
            let ns = test_ns(u);
            if let Some(table) = unit_tables.get(u) {
                let assets = table.assets.keys().filter(|n| seen_assets.insert(*n));
                let mut names: Vec<String> = table
                    .types
                    .keys()
                    .chain(table.fns.keys())
                    .chain(assets)
                    .chain(table.embeds.keys())
                    .filter(|n| !target_values.contains(n))
                    .cloned()
                    .collect();
                names.sort();
                names.dedup();
                let type_names = aliased_types(table, |n| !target_local.contains(n));
                bound.extend(names.iter().cloned());
                crate::emitter::extend_printed_at(
                    out,
                    emit_ns_destructure(&ns, &names, &type_names),
                    2,
                );
            }
        }
    }
    // #1814: the types the target's view reaches but does not import, from
    // their owning commons (imported by `emit_test_module`).
    extend_hidden_destructures(out, &hidden_by_owner(hidden), unit_tables);
    bound.extend(hidden.keys().cloned());
    // Bring consumed-context exported names into scope, plus a `Payment`
    // alias for the consumed surface (so `Payment.authorise.call(...)` works).
    if let Some(consumed) = unit_consumes.get(target_name) {
        let aliases = unit_consumes_aliases
            .get(target_name)
            .cloned()
            .unwrap_or_default();
        let mut alias_for: HashMap<String, String> = HashMap::new();
        for (alias, q) in &aliases {
            alias_for.insert(q.clone(), alias.clone());
        }
        for q in consumed {
            let ns = test_ns(q);
            let is_adapter = matches!(
                unit_tables.get(q).and_then(|t| t.kind),
                Some(UnitKind::Adapter)
            );
            if let Some(table) = unit_tables.get(q) {
                let types = aliased_types(table, |_| true);
                let names: Vec<String> = types.iter().map(|(n, _)| n.clone()).collect();
                bound.extend(names.iter().cloned());
                crate::emitter::extend_printed_at(out, emit_ns_destructure(&ns, &names, &types), 2);
                // #1883 (review of #1923): a consumed context's content-hashed
                // assets, which the target's Worker ships too (its fixed paths
                // stay with its own Worker, and are not nameable).
                let mut hashed: Vec<String> = table
                    .assets
                    .iter()
                    .filter(|(n, d)| d.at.is_none() && !target_values.contains(n))
                    .filter(|(n, _)| seen_assets.insert(*n))
                    .map(|(n, _)| n.clone())
                    .collect();
                hashed.sort();
                bound.extend(hashed.iter().cloned());
                if !hashed.is_empty() {
                    crate::emitter::extend_printed_at(
                        out,
                        emit_ns_destructure(&ns, &hashed, &[]),
                        2,
                    );
                }
            }
            // An `adapter` target has no `__makeSurface`/`deps.surface` entry —
            // its capabilities are already flattened onto `deps` directly
            // (Locale capability track, slice 1, #844).
            if is_adapter {
                continue;
            }
            let key = alias_for
                .get(q)
                .cloned()
                .unwrap_or_else(|| q.rsplit('.').next().unwrap_or(q.as_str()).to_string());
            // P7.2: deferred, not narrowed. `deps.surface`'s own per-capability
            // mock shapes are constructed elsewhere in this file; typing `{key}`
            // correctly here needs cross-referencing that mock-construction code
            // rather than guessing a structural type, which risks a `tsc --strict`
            // mismatch against whatever it actually builds. The *node
            // representation* has no such gap, though — `TsExpr::OptionalMember`
            // already exists — so review of #1404 caught this site's own
            // `format!` construction as a real miss, not a documented carve-out.
            let alias_stmt = TsStmt::const_stmt(
                TsBindingName::Ident(key.clone()),
                None,
                TsExpr::OptionalMember {
                    object: Box::new(member(
                        TsExpr::As {
                            expr: Box::new(ident("deps")),
                            ty: TsType::named("any"),
                        },
                        "surface",
                    )),
                    property: key.clone(),
                },
                None,
            );
            out.push_str(&bynk_ts::print_stmt(&alias_stmt, 2));
            bound.insert(key);
        }
    }
    bound
}

/// #1484: the `async function {name}() { ... }` wrapper every test-generation
/// runner shares (`emit_test_case_function`/`emit_test_property_function`/
/// `emit_test_history_property_function`/`emit_contract_attack_function`, and
/// `emit_integration_module`'s own inline case wrapper) — no return type
/// annotation, no generics, no params, never exported. One shared
/// construction (a real [`TsDecl::Function`]) instead of five separate
/// `format!("async function {name}() {{\n")` hand-templates.
///
/// `body_text` is the function's own already-fully-formed inner content —
/// everything between the wrapper's own opening and closing braces (a
/// `try`/`catch`, or a flat sequence of statements ending in a `return`),
/// carried as one opaque [`TsStmt::raw`] and not decomposed further: it mixes
/// real node prints (`bynk_ts::print_stmt` calls already scattered through
/// each caller) with genuinely-opaque lowered bodies (`lower.rs`'s permanent
/// exclusion, ADR 0391) at depths this track's own established convention
/// for these files uses, so decomposing the wrapper's *interior* into a
/// fully real tree per site would risk silently changing that convention's
/// baked-in indentation — the `TsDecl::Function`/`Raw`-body combination
/// `emit_free_fn` (#1480) already established for exactly this reason.
/// `nested_map`, when `Some`, is the one real per-case source map this
/// track still merges (`emit_test_case_function`'s own `case_smb`/
/// `emit_integration_module`'s own inline case) — every other caller's own
/// body map is already a deliberate, pre-existing scope cut (property/
/// history-property/contract-attack bodies are collaborator-free predicate
/// scaffolding), unchanged by this conversion.
fn async_test_runner_fn(
    name: &str,
    body_text: String,
    nested_map: Option<SourceMapBuilder>,
) -> TsStmt {
    let mut raw_body = TsStmt::raw(body_text, None);
    raw_body.nested_map = nested_map;
    TsStmt::decl(
        TsDecl::Function {
            name: name.to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            body: vec![raw_body],
            is_async: true,
            inline: false,
        },
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn emit_test_case_function(
    runner_name: &str,
    case: &Case,
    // #2134: the case's `env` entries, when the module holds `__bynkEnv`
    // (some case in it has entries): set first, and cleared on the way out
    // so a later `property` runs with none.
    env: Option<BTreeMap<String, String>>,
    deps_factory: &str,
    target_name: &str,
    target_kind: UnitKind,
    stubs: &HashMap<String, ResolvedStub>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    source: &str,
    rel_path: &str,
    runtime_use: &RuntimeUse,
    // #1860: the run's suite ordinal, when some `stub` is suite-scoped.
    suite: Option<usize>,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<Types>,
) -> TsStmt {
    // #291: a case some `stub` clause is scoped to passes its name to
    // `__makeTestDeps`, so the clause applies to it.
    let scoped = stubs.values().any(|rp| {
        rp.clause_scopes
            .iter()
            .any(|s| matches!(s, StubScope::Case(c) if *c == case.name))
    });
    let mut out = String::new();
    out.push_str("  try {\n");
    if let Some(env) = &env {
        let entries: Vec<(String, TsExpr)> = env
            .iter()
            .map(|(k, v)| (k.clone(), str_lit(v.clone())))
            .collect();
        out.push_str(&bynk_ts::print_stmt(
            &TsStmt::assign(ident("__bynkEnv"), TsExpr::object(entries), None),
            2,
        ));
    }
    let harness_bound = emit_test_scope_setup(
        &mut out,
        deps_factory,
        target_name,
        target_kind,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        block_uses_observation(&case.body),
        scoped.then_some(case.name.as_str()),
        suite,
        hidden,
    );
    let mut typed = synthetic_typed_commons_for_target(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        hidden,
        tys,
    );
    // v0.117: re-type-check the case body (with the call-record types registered)
    // so the lowering has full expr types — collection kernels, notably a
    // `trace(Cap.op)` result's `List[…]` methods, dispatch on the checked type.
    if let Some((mut resolved, _)) = test_suites::build_privileged_resolved(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    ) {
        test_suites::register_call_record_types(&mut resolved, target_name, unit_tables);
        let mut throwaway_errors: Vec<CompileError> = Vec::new();
        let mut throwaway_refs = RefSink::new();
        // P6.21 review: `callees` is real output here now, not discarded —
        // see `typecheck_case_body`'s own doc comment for why a test-case
        // body's `Callee` classification was previously lost entirely.
        (typed.expr_types, typed.callees) = test_suites::typecheck_case_body(
            target_name,
            &case.body,
            case.span,
            unit_tables,
            &resolved,
            &mut throwaway_errors,
            &mut throwaway_refs,
            HashMap::new(),
            tys,
        );
        // #1875 review: as at the system tier, an error here is drift.
        debug_assert!(
            throwaway_errors.is_empty(),
            "a unit case re-typed for emission reported errors the check pass did not: {:?}",
            throwaway_errors
                .iter()
                .map(|e| e.category)
                .collect::<Vec<_>>()
        );
    }
    let cross = bynk_check::resolver::CrossContextInfo::default();
    let test_services: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.services.keys().cloned().collect())
        .unwrap_or_default();
    let test_agents: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.agents.keys().cloned().collect())
        .unwrap_or_default();
    let (body_src, body_smb) = emitter::lower_test_case_body(
        &case.body,
        &mut typed,
        &cross,
        test_services,
        target_service_handler_kinds(unit_tables.get(target_name)),
        target_sum_handlers(unit_tables.get(target_name)),
        test_agents,
        target_uses_send(target_name, target_kind, unit_tables),
        harness_bound,
        source,
        rel_path,
        runtime_use,
    );
    // v0.70: splice the case body (line-by-line, indented) and merge its source-map
    // sub-builder into the case builder, line-anchored at the splice. The caller
    // (emit_test_module) merges this case builder into the module map under the
    // case's source file.
    let body_base = out.len();
    for line in body_src.lines() {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    let mut case_smb = SourceMapBuilder::new();
    case_smb.merge(&body_smb, &body_src, &out, body_base, 0);
    let return_pass_true = TsStmt::return_stmt(
        Some(TsExpr::object(vec![(
            "pass".to_string(),
            TsExpr::Lit(TsLit::Bool(true)),
        )])),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&return_pass_true, 2));
    out.push_str("  } catch (e) {\n");
    let expectation_error_branch = if_(
        TsExpr::Binary {
            op: TsBinaryOp::InstanceOf,
            left: Box::new(ident("e")),
            right: Box::new(ident("ExpectationError")),
        },
        block(vec![TsStmt::return_stmt(
            Some(TsExpr::object(vec![
                ("pass".to_string(), TsExpr::Lit(TsLit::Bool(false))),
                (
                    "error".to_string(),
                    TsExpr::object(vec![
                        ("message".to_string(), member(ident("e"), "message")),
                        ("location".to_string(), member(ident("e"), "location")),
                    ]),
                ),
            ])),
            None,
        )]),
    );
    out.push_str(&bynk_ts::print_stmt(&expectation_error_branch, 2));
    let fallback_return = TsStmt::return_stmt(
        Some(TsExpr::object(vec![
            ("pass".to_string(), TsExpr::Lit(TsLit::Bool(false))),
            (
                "error".to_string(),
                TsExpr::object(vec![
                    (
                        "message".to_string(),
                        call(ident("String"), vec![ident("e")]),
                    ),
                    ("location".to_string(), str_lit("unknown")),
                    // #2272: the declaration a violated invariant or transition names.
                    (
                        "violation".to_string(),
                        call(ident("__bynkViolationOf"), vec![ident("e")]),
                    ),
                ]),
            ),
        ])),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&fallback_return, 2));
    if env.is_some() {
        out.push_str("  } finally {\n    __bynkEnv = {};\n  }\n");
    } else {
        out.push_str("  }\n");
    }
    async_test_runner_fn(runner_name, out, Some(case_smb))
}

/// v0.114 (testing track slice 2): the runtime the generative `property` runner
/// relies on — a seeded PRNG, per-type shrinkers, the case loop, and shrinking.
/// Emitted once per test module that declares a `property`, alongside the
/// expectation helpers. `__bynkSeed` is the run's root seed (from
/// `BYNK_TEST_SEED`, else random), from which each property derives its seed via
/// `__bynkMix`, so `bynkc test --seed <hex>` reproduces a run byte-for-byte.
/// v0.117: emit a TypeScript `type` alias for each observed capability
/// operation's call record (`type __Cap_op_Call = { param: T, … }`), so a
/// `trace(Cap.op)` list — whose elements are these records — type-checks under
/// `tsc` when a test projects a field (`c.msg`). Names mirror
/// [`checker::call_record_type_name`]. Ordered by capability then operation for
/// deterministic output.
/// #1479: returns real [`TsStmt`]s (was `String`) — its one caller now
/// appends via `crate::emitter::extend_printed_at(out, stmts, 0)`, the same
/// depth-0 top-level shape these type aliases always printed at.
fn observation_call_record_types(
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
) -> Vec<TsStmt> {
    let Some(table) = unit_tables.get(target_name) else {
        return Vec::new();
    };
    // Named/opaque parameter types are re-exported under the target's namespace,
    // so qualify them (`AuthId` → `commerce_payment.AuthId`); base types are
    // unaffected. Matches the mock-signature qualification.
    let scope_ns = test_ns(target_name);
    let mut type_ns: HashMap<String, String> = table
        .types
        .keys()
        .map(|n| (n.clone(), scope_ns.clone()))
        .collect();
    // #291: a consumed adapter's capabilities are seams too, their parameter
    // types qualified by the adapter's namespace (`bynk.Request`).
    let mut caps: std::collections::BTreeMap<&String, &bynk_syntax::ast::CapabilityDecl> =
        table.capabilities.iter().collect();
    for (cap, decl, owner) in platform_seams(target_name, unit_tables) {
        let uns = test_ns(owner);
        for n in unit_tables[owner].types.keys() {
            type_ns.entry(n.clone()).or_insert_with(|| uns.clone());
        }
        caps.entry(cap).or_insert(decl);
    }
    let mut stmts = Vec::new();
    for (cap, decl) in caps {
        for op in &decl.ops {
            let name = checker::call_record_type_name(cap, &op.name.name);
            let fields: Vec<TsTypeMember> = op
                .params
                .iter()
                .map(|p| TsTypeMember::Prop {
                    name: p.name.name.clone(),
                    ty: {
                        // A generic operation's own type parameter (`Events.emit[E]`)
                        // has no binding here, so a field mentioning one is `unknown`.
                        let ty =
                            emitter::ts_type_ref_qualified_multi_ts_type(&p.type_ref, &type_ns);
                        let printed = bynk_ts::print_type(&ty);
                        let generic = op.type_params.iter().any(|tp| {
                            printed
                                .split(|c: char| !c.is_alphanumeric() && c != '_')
                                .any(|w| w == tp.name.name)
                        });
                        if generic {
                            TsType::named("unknown")
                        } else {
                            ty
                        }
                    },
                    optional: false,
                    readonly: false,
                })
                .collect();
            // The same `"{  }"` double-space quirk as elsewhere in this file
            // (`gen_ts_for_ty`/`canon_ts_for_ty`, Arc C slice 31; `emit_test_deps`
            // above) — the pre-conversion `format!("type {name} = {{ {} }};\n",
            // fields.join("; "))` template always has a space on each side of its
            // `{}` slot, so a zero-param capability op (real and reachable)
            // literally produced a double space, not the tight `"{}"`
            // `TsType::Object`'s own empty-members shortcut renders.
            let ty = if fields.is_empty() {
                TsType::named("{  }")
            } else {
                TsType::Object(fields)
            };
            stmts.push(TsStmt::decl(
                TsDecl::TypeAlias {
                    name,
                    type_params: Vec::new(),
                    ty,
                },
                None,
            ));
        }
    }
    stmts
}

/// v0.117: the observation runtime — wraps each observed capability operation on
/// the test `deps` so every call records its arguments and a monotonic order
/// index into the per-case trace `__obs`. Emitted once per module that observes.
/// Finding #17: source lives at `emitter/test_runtime/observation.ts`.
fn observation_runtime_helpers() -> String {
    include_str!("../emitter/test_runtime/observation.ts").to_string()
}

fn property_runtime_helpers() -> String {
    include_str!("../emitter/test_runtime/property.ts").to_string()
}

/// v0.119 (testing track slice 7, ADR 0155): the history-property runtime, emitted
/// alongside [`property_runtime_helpers`] only when a module declares a
/// `for all run: History[Agent]` property. Generates a bounded, seeded sequence of
/// handler calls, drives it through the real handlers (`spec.drive`, the agent
/// module's `__bynkDriveHistory_<Agent>`), evaluates the predicate over the
/// observed `run`, and on failure delta-debugs the *sequence* (re-driving after
/// each reduction so the counterexample stays reachable) then shrinks the surviving
/// arguments — reporting the seed, the shrunk sequence, and a reproduce line.
fn history_runtime_helpers() -> String {
    include_str!("../emitter/test_runtime/history.ts").to_string()
}

/// Integer generation bounds `(lo, hi, floor)` derived from a refinement: `lo`
/// and `hi` bound the random draw (and the boundary values), `floor` is the
/// shrink target. Unrefined `Int` draws over `[-1000, 1000]` toward `0`.
fn int_bounds(refinement: Option<&Refinement>) -> (i64, i64, i64) {
    let mut lo = -1000i64;
    let mut hi = 1000i64;
    let mut floor = 0i64;
    if let Some(r) = refinement {
        for p in &r.predicates {
            match &p.kind {
                PredKind::Positive => {
                    lo = lo.max(1);
                    floor = floor.max(1);
                }
                PredKind::NonNegative => {
                    lo = lo.max(0);
                    floor = floor.max(0);
                }
                PredKind::InRange(a, b) => {
                    lo = a.value;
                    hi = b.value;
                    floor = a.value;
                }
                _ => {}
            }
        }
    }
    if lo > hi {
        hi = lo;
    }
    if floor < lo {
        floor = lo;
    }
    (lo, hi, floor)
}

/// Float generation bounds `(lo, hi)` from a refinement (`lo` doubles as the
/// shrink target). Unrefined `Float` draws over `[-1000, 1000]`.
fn float_bounds(refinement: Option<&Refinement>) -> (f64, f64) {
    let mut lo = -1000.0f64;
    let mut hi = 1000.0f64;
    if let Some(r) = refinement {
        for p in &r.predicates {
            match &p.kind {
                PredKind::Positive => lo = lo.max(1.0),
                PredKind::NonNegative => lo = lo.max(0.0),
                PredKind::InRangeF(a, b) => {
                    lo = a.value;
                    hi = b.value;
                }
                _ => {}
            }
        }
    }
    if lo > hi {
        hi = lo;
    }
    (lo, hi)
}

/// Minimum string length a refinement demands (`0` if none).
fn str_min(refinement: Option<&Refinement>) -> i64 {
    let mut min = 0i64;
    if let Some(r) = refinement {
        for p in &r.predicates {
            match p.kind {
                PredKind::NonEmpty => min = min.max(1),
                PredKind::MinLength(k) | PredKind::Length(k) => min = min.max(k),
                _ => {}
            }
        }
    }
    min
}

/// A canonical (deterministic) TypeScript literal inhabiting a base type — used
/// for opaque `.unsafe` wrapping of the exotic bases (`Duration`, `Bytes`, …).
fn base_canon(base: BaseType) -> String {
    match base {
        BaseType::Int => "0n".to_string(),
        BaseType::String => "\"\"".to_string(),
        BaseType::Bool => "true".to_string(),
        BaseType::Float => "0".to_string(),
        BaseType::Duration | BaseType::Instant => "0".to_string(),
        BaseType::Bytes => "new globalThis.Uint8Array()".to_string(),
    }
}

/// A TypeScript expression that draws a random inhabitant of `base` (refined by
/// `refinement`), branded to `<name>` — via `<name>.unsafe(...)` for an opaque
/// type, or an inline `as` cast for a refined/alias type (ADR 0182).
fn refined_gen_ts(
    name: &str,
    base: BaseType,
    refinement: Option<&Refinement>,
    is_opaque: bool,
) -> TsExpr {
    let draw = match base {
        BaseType::Int => {
            let (lo, hi, _) = int_bounds(refinement);
            method_call(
                ident("rng"),
                "int",
                vec![num_lit(format!("{lo}n")), num_lit(format!("{hi}n"))],
            )
        }
        BaseType::Float => {
            let (lo, hi) = float_bounds(refinement);
            method_call(
                ident("rng"),
                "float",
                vec![num_lit(lo.to_string()), num_lit(hi.to_string())],
            )
        }
        BaseType::String => {
            let min = str_min(refinement);
            method_call(
                ident("rng"),
                "str",
                vec![num_lit(min.to_string()), num_lit((min + 8).to_string())],
            )
        }
        BaseType::Bool => method_call(ident("rng"), "bool", vec![]),
        _ => ident(base_canon(base)),
    };
    // `unchecked_construct_test` is also called from `lower.rs`'s own
    // permanently-excluded general expression lowering (Third correction) and
    // must keep its exact `-> String` signature — its own branching (a real
    // `.unsafe(...)` call vs a real `(... as any)` cast) is not duplicated here
    // as a second node-building path; its already-formed text is carried as an
    // opaque `Ident` instead, the same "an unconverted shared sibling's own
    // text stays opaque" posture this track already uses (P7.9's `ts_base`,
    // #1369's `Cell` zero value).
    ident(emitter::unchecked_construct_test(
        name,
        &bynk_ts::print_expr(&draw),
        is_opaque,
    ))
}

/// Coerce a Sum-variant-payload/Record-field value to `number` when its own
/// resolved type draws/emits as `bigint` — a bare `Int`, or a refined/opaque
/// `Int` (#1398). `gen_ts_for_ty`'s/`canon_ts_for_ty`'s own top-level
/// `Base(Int)` arm draws/emits `bigint` — correct there, since a top-level
/// `for all` binding's own boundaries/shrink machinery is bigint-typed
/// throughout (the "P7.2: deferred, not narrowed" representation gap, #1426
/// tracks its own further consequences). But a Sum variant's real compiled
/// constructor and a Record's real compiled object shape both type an `Int`
/// field `number` (`ts_type_ref_to_ts_type`'s own mapping) — including a
/// refined/opaque one, which erases to `number & { __brand }`, not `bigint`
/// — so a bigint-drawing payload/field value recursed into from *inside* a
/// Sum/Record arm needs this coercion the top-level arm itself must not
/// have. A refined/opaque `Int` (`Percent`, etc.) routes through
/// `refined_gen_ts`'s/its `canon_ts_for_ty` sibling's own
/// `unchecked_construct_test`-wrapped cast (`(rng.int(...) as any)` /
/// `Pct.unsafe(rng.int(...))`) — that cast is compile-time only and does
/// nothing to the runtime value, so it still needs this same coercion
/// (review of #1428): confirmed the exact `TypeError: Cannot mix BigInt and
/// other types` this issue names still reproduces for a refined-Int record
/// field without it. `Number(...)` around either wrapper form is safe:
/// `.unsafe`/`as any` are both identity at runtime, never anything more than
/// a re-tag. Every other type's own recursive result is already
/// number/string/bool/object-shaped, so this stays a no-op there. And the
/// depth-exhaustion `undefined` sentinel (`canon_ts_for_ty`'s own `depth ==
/// 0` early return, reachable through a self-recursive Sum whose *first*
/// variant carries an `Int` payload — `prop_binding_generable` validates only
/// that variant, `bynk-check/src/test_suites.rs`) is passed through
/// unwrapped rather than turned into a silent `Number(undefined)` (`NaN`) —
/// also review of #1428.
fn coerce_int_field(
    t: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
    value: TsExpr,
) -> TsExpr {
    if matches!(&value, TsExpr::Ident(s) if s == "undefined") {
        return value;
    }
    if ty_draws_bigint(t, types, tys) {
        call(ident("globalThis.Number"), vec![value])
    } else {
        value
    }
}

/// Does `t` draw/emit as a JS `bigint` — a bare `Int`, or a refined/opaque
/// type over one? The shared predicate behind [`coerce_int_field`]'s own
/// `Number(…)`-wrap decision and [`destructure_vals`]'s "does this binding
/// set need coercion at all" check (#1426).
fn ty_draws_bigint(
    t: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
) -> bool {
    match &*tys.get(t) {
        checker::Ty::Base(BaseType::Int) => true,
        checker::Ty::Named { name, .. } => matches!(
            types.get(name).map(|d| &d.body),
            Some(TypeBody::Refined {
                base: BaseType::Int,
                ..
            }) | Some(TypeBody::Opaque {
                base: BaseType::Int,
                ..
            })
        ),
        _ => false,
    }
}

/// Coerce a top-level `for all`/attack-parameter binding's own drawn value to
/// `number` (#1426) — like [`coerce_int_field`], but for the bind site rather
/// than a nested Sum/Record field, which needs a genuinely different result
/// shape for a *named* refined/opaque `Int`, not just the same `Number(…)`
/// wrap. Review of #1432, finding 1: a bare `Int` really does compile to
/// plain `number` everywhere, so `Number(value)` alone is correct and
/// sufficient there. But a refined/opaque `Int` (`Percent`, etc.) compiles to
/// a *branded* type (`number & { readonly __brand: "Percent" }`,
/// `emit_refined_type`) — a plain `Number(value)` local's own inferred type
/// is bare `number`, not the brand, so passing it to a real function
/// expecting the named type is a genuine new `tsc --strict` `TS2345` error
/// this fix would otherwise introduce (confirmed: reverting to a bare
/// `Number(…)` wrap here reproduces it for a minimal `fn label(p: Percent)`
/// repro, which compiled and ran fine pre-#1426 since the pre-fix local was
/// `any`, not a concrete type). Closed the same way `refined_gen_ts` already
/// keeps a branded draw compiling — wrap the coerced `Number(…)` in `(… as
/// any)`, restoring the escape hatch a plain array-destructured `any[]`
/// element always had for this shape, while still fixing the *runtime* value
/// (`as any` is compile-time only; the `Number(…)` underneath still runs).
/// Every other type's own value (including a bare `Int`) is unaffected —
/// only a *named* refined/opaque `Int` binding gets the extra wrap.
fn coerce_top_level_int_binding(
    t: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
    value: TsExpr,
) -> TsExpr {
    let is_named_int = matches!(
        &*tys.get(t),
        checker::Ty::Named { .. } if ty_draws_bigint(t, types, tys)
    );
    let coerced = coerce_int_field(t, types, tys, value);
    if is_named_int {
        TsExpr::As {
            expr: Box::new(coerced),
            ty: TsType::named("any"),
        }
    } else {
        coerced
    }
}

/// Destructure `__vals` into its named locals, coercing each `Int`-drawing
/// (bare, refined, or opaque) binding's own runtime value to `number` at the
/// bind site (#1426) via [`coerce_top_level_int_binding`]. `binding_gen`'s
/// own boundaries/shrink machinery for a top-level `for all`/attack-parameter
/// `Int` binding is deliberately `bigint`-typed throughout —
/// `coerce_int_field`'s own doc comment names this as correct, since the
/// shrink loop's `current[i]`/`vals[i]` elements must keep matching what
/// `gens[i].shrink(v)` expects — but the *local* a predicate/attacked-function
/// body evaluates ordinary Bynk `Int` arithmetic against must not stay
/// `bigint`, or `n + 1` throws `TypeError: Cannot mix BigInt and other types`
/// the instant it meets a `number` literal.  Never previously hit: every
/// existing fixture's predicate either avoided arithmetic on a drawn `Int`
/// entirely, or routed through a refined type's own compile-time-only `as
/// any` cast (`refined_gen_ts`), which suppresses the *static* `tsc` error,
/// not the *runtime* one. Reuses `coerce_int_field`'s own "does this type
/// draw bigint" check — the same representation gap #1398/#1428 already
/// closed for record/sum-field values, just at the binding site itself
/// rather than a nested field; `binding_types[i]` is `None` when the
/// binding's own type didn't resolve (mirrors `gens`' own
/// `unwrap_or`/`undefined`-sentinel fallback above — `__vals[i]` is
/// `undefined` there regardless of what type it "should" have been, so it
/// passes through unwrapped either way).
///
/// Falls back to the original plain `const [a, b, ...] = __vals;` array
/// destructure when *no* binding actually needs coercion — a record/sum/
/// string/bool binding (the common case) draws its own top-level value
/// already `number`/`string`/`boolean`/object-shaped, only a bare or
/// refined/opaque `Int` binding draws `bigint` — so the common case emits
/// byte-identical text to before this fix, and only a fixture with a real
/// top-level `Int`-drawing binding sees its own destructure change shape.
fn destructure_vals(
    names: &[String],
    binding_types: &[Option<checker::TyId>],
    types: Option<&HashMap<String, Arc<TypeDecl>>>,
    tys: &Arc<Types>,
) -> String {
    let needs_coercion = binding_types
        .iter()
        .any(|ty| matches!((ty, types), (Some(t), Some(types)) if ty_draws_bigint(*t, types, tys)));
    if !needs_coercion {
        return format!("const [{}] = __vals;", names.join(", "));
    }
    names
        .iter()
        .zip(binding_types)
        .enumerate()
        .map(|(i, (name, ty))| {
            let raw = TsExpr::Index {
                object: Box::new(ident("__vals")),
                index: Box::new(num_lit(i.to_string())),
            };
            let value = match (ty, types) {
                (Some(t), Some(types)) => coerce_top_level_int_binding(*t, types, tys, raw),
                _ => raw,
            };
            format!("const {name} = {};", bynk_ts::print_expr(&value))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A TypeScript expression drawing a random inhabitant of a resolved type using
/// the in-scope `rng` — the property generator (DECISION P: a type is its own
/// inhabitant space). Sums pick a random variant; records generate every field.
fn gen_ts_for_ty(
    ty: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    depth: u32,
    tys: &Arc<Types>,
) -> TsExpr {
    if depth == 0 {
        // #1429: a bare `1` here starved `canon_ts_for_ty`'s own recursion
        // one level short of what `prop_binding_generable` (`bynk-check`)
        // already promised was reachable *for the top-level bound type*.
        // Both functions share the same recursion shape — `depth == 0`
        // checked unconditionally at entry, one unit spent per `Sum`/
        // `Record` step down into a field's own type, `Ty::Base` never
        // itself gating on `depth` — so a type `prop_binding_generable`
        // accepts within `PROP_GEN_DEPTH` is guaranteed to bottom out in
        // `canon_ts_for_ty` within that same budget: a self-recursive type
        // like `T = Base(n: Int) | Cons(tail: T)` previously exhausted
        // this fallback's own budget one field short of `Base`'s terminal
        // `n: Int`, landing a bare `undefined` inside a real, `number`-
        // typed constructor argument (`T.Base(undefined)`, `tsc TS2345`).
        //
        // Review of #1434, finding 1: `prop_binding_generable` only ever
        // descends a `Sum`'s *first* variant, but this function's own Sum
        // branch (below) builds a thunk for *every* variant — so `ty` here
        // can be a type the checker never validated at all (reached only
        // through a non-first variant, e.g. `Nope(b: Bad)` on
        // `T = Plain(n: Int) | Nope(b: Bad)`, where `Bad = Node(l: Bad, r:
        // Bad) | Leaf(n: Int)` is itself first-variant-unterminating). For
        // such a type hand it the full `PROP_GEN_DEPTH` budget regardless,
        // and `canon_ts_for_ty`'s own "always expand the first variant"
        // strategy walks a full branching tree before ever reaching the
        // `Leaf` escape — confirmed to reproduce a real multi-second,
        // 170MB+ emit for exactly this shape (a plausible emit-time
        // hang/OOM, not merely an invalid file) once this fallback started
        // handing out 12 levels of budget instead of 1.
        //
        // So: only spend the full budget when the checker actually
        // promised `ty` itself bottoms out within it: that's exactly what
        // `prop_binding_generable` decides, and — same "first variant
        // only, `.all` short-circuits on the first failing field" shape as
        // `canon_ts_for_ty` — costs at most `PROP_GEN_DEPTH` recursive
        // calls to answer, not a tree walk. Any other type falls back to
        // the original, deliberately conservative `1` — the same "produces
        // a small invalid file fast" behaviour this fallback always had
        // for a type the checker never vouched for, not a regression.
        if test_suites::prop_binding_generable(ty, types, test_suites::PROP_GEN_DEPTH, tys) {
            return canon_ts_for_ty(ty, types, test_suites::PROP_GEN_DEPTH, tys);
        }
        return canon_ts_for_ty(ty, types, 1, tys);
    }
    match &*tys.get(ty) {
        checker::Ty::Base(BaseType::Int) => {
            let (lo, hi, _) = int_bounds(None);
            method_call(
                ident("rng"),
                "int",
                vec![num_lit(format!("{lo}n")), num_lit(format!("{hi}n"))],
            )
        }
        checker::Ty::Base(BaseType::String) => {
            method_call(ident("rng"), "str", vec![num_lit("0"), num_lit("8")])
        }
        checker::Ty::Base(BaseType::Bool) => method_call(ident("rng"), "bool", vec![]),
        checker::Ty::Base(BaseType::Float) => method_call(
            ident("rng"),
            "float",
            vec![num_lit("-1000"), num_lit("1000")],
        ),
        checker::Ty::Base(b) => ident(base_canon(*b)),
        checker::Ty::Named { name, .. } => {
            let Some(decl) = types.get(name) else {
                return ident("undefined");
            };
            match &decl.body {
                TypeBody::Refined {
                    base, refinement, ..
                }
                | TypeBody::Opaque {
                    base, refinement, ..
                } => refined_gen_ts(
                    name,
                    *base,
                    refinement.as_ref(),
                    matches!(decl.body, TypeBody::Opaque { .. }),
                ),
                TypeBody::Sum(s) => {
                    let thunks: Vec<TsExpr> = s
                        .variants
                        .iter()
                        .map(|v| {
                            let body = if v.payload.is_empty() {
                                member(ident(name), &v.name.name)
                            } else {
                                let args: Vec<TsExpr> = v
                                    .payload
                                    .iter()
                                    .map(|f| {
                                        checker::resolve_type_ref(&f.type_ref, types, tys)
                                            .map(|t| {
                                                coerce_int_field(
                                                    t,
                                                    types,
                                                    tys,
                                                    gen_ts_for_ty(t, types, depth - 1, tys),
                                                )
                                            })
                                            .unwrap_or_else(|| ident("undefined"))
                                    })
                                    .collect();
                                call(member(ident(name), &v.name.name), args)
                            };
                            TsExpr::Arrow {
                                params: Vec::new(),
                                is_async: false,
                                generics: Vec::new(),
                                return_type: None,
                                body: Box::new(TsArrowBody::Expr(Box::new(body))),
                            }
                        })
                        .collect();
                    if thunks.is_empty() {
                        ident("undefined")
                    } else {
                        method_call(ident("rng"), "pick", vec![TsExpr::array(thunks)])
                    }
                }
                TypeBody::Record(r) => {
                    let fields: Vec<(String, TsExpr)> = r
                        .fields
                        .iter()
                        .map(|f| {
                            let g = checker::resolve_type_ref(&f.type_ref, types, tys)
                                .map(|t| {
                                    coerce_int_field(
                                        t,
                                        types,
                                        tys,
                                        gen_ts_for_ty(t, types, depth - 1, tys),
                                    )
                                })
                                .unwrap_or_else(|| ident("undefined"));
                            (f.name.name.clone(), g)
                        })
                        .collect();
                    // The same `"{  }"` double-space quirk `workers.rs`/
                    // `project.rs`/`emit.rs` already carry as opaque text
                    // (#1321/#1327/#1390): the pre-conversion `format!("{{
                    // {} }}", fields.join(", "))` template always has a space
                    // on each side of its `{}` slot, so zero fields literally
                    // produced a double space, not the tight `"{}"`
                    // `TsExpr::object`'s own empty-entries shortcut renders.
                    // A zero-field record is exotic but not provably
                    // unreachable — guarded rather than assumed.
                    if fields.is_empty() {
                        ident("{  }")
                    } else {
                        TsExpr::object(fields)
                    }
                }
            }
        }
        _ => ident("undefined"),
    }
}

/// A canonical (deterministic, boundary) inhabitant of a resolved type — the
/// boundary value the runner draws first (refinement floor / minimum length /
/// first variant), and the shrink target for sums.
fn canon_ts_for_ty(
    ty: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    depth: u32,
    tys: &Arc<Types>,
) -> TsExpr {
    if depth == 0 {
        return ident("undefined");
    }
    match &*tys.get(ty) {
        checker::Ty::Base(BaseType::Int) => num_lit("0n"),
        checker::Ty::Base(BaseType::String) => str_lit(""),
        checker::Ty::Base(BaseType::Bool) => TsExpr::Lit(TsLit::Bool(true)),
        checker::Ty::Base(BaseType::Float) => num_lit("0"),
        checker::Ty::Base(b) => ident(base_canon(*b)),
        checker::Ty::Named { name, .. } => {
            let Some(decl) = types.get(name) else {
                return ident("undefined");
            };
            match &decl.body {
                TypeBody::Refined {
                    base, refinement, ..
                }
                | TypeBody::Opaque {
                    base, refinement, ..
                } => {
                    let lit = match base {
                        BaseType::Int => {
                            let (lo, _, _) = int_bounds(refinement.as_ref());
                            num_lit(format!("{lo}n"))
                        }
                        BaseType::Float => {
                            let (lo, _) = float_bounds(refinement.as_ref());
                            num_lit(lo.to_string())
                        }
                        BaseType::String => {
                            let min = str_min(refinement.as_ref());
                            str_lit("x".repeat(min.max(0) as usize))
                        }
                        BaseType::Bool => TsExpr::Lit(TsLit::Bool(true)),
                        other => ident(base_canon(*other)),
                    };
                    ident(emitter::unchecked_construct_test(
                        name,
                        &bynk_ts::print_expr(&lit),
                        matches!(decl.body, TypeBody::Opaque { .. }),
                    ))
                }
                TypeBody::Sum(s) => match s.variants.first() {
                    None => ident("undefined"),
                    Some(v) if v.payload.is_empty() => member(ident(name), &v.name.name),
                    Some(v) => {
                        let args: Vec<TsExpr> = v
                            .payload
                            .iter()
                            .map(|f| {
                                checker::resolve_type_ref(&f.type_ref, types, tys)
                                    .map(|t| {
                                        coerce_int_field(
                                            t,
                                            types,
                                            tys,
                                            canon_ts_for_ty(t, types, depth - 1, tys),
                                        )
                                    })
                                    .unwrap_or_else(|| ident("undefined"))
                            })
                            .collect();
                        call(member(ident(name), &v.name.name), args)
                    }
                },
                TypeBody::Record(r) => {
                    let fields: Vec<(String, TsExpr)> = r
                        .fields
                        .iter()
                        .map(|f| {
                            let g = checker::resolve_type_ref(&f.type_ref, types, tys)
                                .map(|t| {
                                    coerce_int_field(
                                        t,
                                        types,
                                        tys,
                                        canon_ts_for_ty(t, types, depth - 1, tys),
                                    )
                                })
                                .unwrap_or_else(|| ident("undefined"));
                            (f.name.name.clone(), g)
                        })
                        .collect();
                    // Same `"{  }"` double-space quirk as `gen_ts_for_ty`'s
                    // own Record branch above — see its comment for why.
                    if fields.is_empty() {
                        ident("{  }")
                    } else {
                        TsExpr::object(fields)
                    }
                }
            }
        }
        _ => ident("undefined"),
    }
}

/// The generator descriptor for one `for all` binding: boundary values, a random
/// generator, and a shrinker (all real TypeScript expressions, evaluated in the
/// runner's scope where the type constructors are in scope).
struct BindingGen {
    boundaries: Vec<TsExpr>,
    gen_ts: TsExpr,
    shrink: TsExpr,
}

/// Build the generator descriptor for a binding whose resolved type is `ty`.
fn binding_gen(
    ty: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
) -> BindingGen {
    let gen_ts = gen_ts_for_ty(ty, types, test_suites::PROP_GEN_DEPTH, tys);
    let (boundaries, shrink) = match &*tys.get(ty) {
        checker::Ty::Base(BaseType::Int) => {
            let (lo, hi, floor) = int_bounds(None);
            (
                vec![
                    num_lit(format!("{floor}n")),
                    num_lit(format!("{hi}n")),
                    num_lit(format!("{lo}n")),
                ],
                call(
                    ident("__bynkShrinkInt"),
                    vec![ident("v"), num_lit(format!("{floor}n"))],
                ),
            )
        }
        checker::Ty::Base(BaseType::String) => (
            vec![str_lit("")],
            call(ident("__bynkShrinkString"), vec![ident("v"), num_lit("0")]),
        ),
        checker::Ty::Base(BaseType::Bool) => (
            vec![
                TsExpr::Lit(TsLit::Bool(true)),
                TsExpr::Lit(TsLit::Bool(false)),
            ],
            TsExpr::Paren(Box::new(cond_expr(
                ident("v"),
                TsExpr::array(vec![TsExpr::Lit(TsLit::Bool(false))]),
                TsExpr::array(vec![]),
            ))),
        ),
        checker::Ty::Named { name, .. } => match types.get(name).map(|d| &d.body) {
            Some(TypeBody::Refined {
                base, refinement, ..
            })
            | Some(TypeBody::Opaque {
                base, refinement, ..
            }) => {
                // ADR 0182: brand via `.unsafe` for opaque, inline `as` cast for
                // refined/alias. The or-pattern can't capture which variant, so
                // re-derive it by name.
                let is_opaque = matches!(
                    types.get(name).map(|d| &d.body),
                    Some(TypeBody::Opaque { .. })
                );
                match base {
                    BaseType::Int => {
                        let (lo, hi, floor) = int_bounds(refinement.as_ref());
                        let shrunk = emitter::unchecked_construct_test(name, "__n", is_opaque);
                        (
                            vec![
                                ident(emitter::unchecked_construct_test(
                                    name,
                                    &format!("{lo}n"),
                                    is_opaque,
                                )),
                                ident(emitter::unchecked_construct_test(
                                    name,
                                    &format!("{hi}n"),
                                    is_opaque,
                                )),
                            ],
                            method_call(
                                call(
                                    ident("__bynkShrinkInt"),
                                    vec![ident("v"), num_lit(format!("{floor}n"))],
                                ),
                                "map",
                                vec![TsExpr::Arrow {
                                    params: vec![TsParam {
                                        name: "__n".to_string(),
                                        ty: Some(TsType::named("bigint")),
                                        optional: false,
                                    }],
                                    is_async: false,
                                    generics: Vec::new(),
                                    return_type: None,
                                    body: Box::new(TsArrowBody::Expr(Box::new(ident(shrunk)))),
                                }],
                            ),
                        )
                    }
                    BaseType::String => {
                        let min = str_min(refinement.as_ref());
                        let lit = format!("\"{}\"", "x".repeat(min.max(0) as usize));
                        let shrunk = emitter::unchecked_construct_test(name, "__s", is_opaque);
                        (
                            vec![ident(emitter::unchecked_construct_test(
                                name, &lit, is_opaque,
                            ))],
                            method_call(
                                call(
                                    ident("__bynkShrinkString"),
                                    vec![ident("v"), num_lit(min.to_string())],
                                ),
                                "map",
                                vec![TsExpr::Arrow {
                                    params: vec![TsParam {
                                        name: "__s".to_string(),
                                        ty: Some(TsType::named("string")),
                                        optional: false,
                                    }],
                                    is_async: false,
                                    generics: Vec::new(),
                                    return_type: None,
                                    body: Box::new(TsArrowBody::Expr(Box::new(ident(shrunk)))),
                                }],
                            ),
                        )
                    }
                    _ => (
                        vec![canon_ts_for_ty(ty, types, test_suites::PROP_GEN_DEPTH, tys)],
                        TsExpr::array(vec![]),
                    ),
                }
            }
            Some(TypeBody::Sum(_)) => {
                let canon = canon_ts_for_ty(ty, types, test_suites::PROP_GEN_DEPTH, tys);
                (vec![canon.clone()], TsExpr::array(vec![canon]))
            }
            _ => (
                vec![canon_ts_for_ty(ty, types, test_suites::PROP_GEN_DEPTH, tys)],
                TsExpr::array(vec![]),
            ),
        },
        _ => (Vec::new(), TsExpr::array(vec![])),
    };
    BindingGen {
        boundaries,
        gen_ts,
        shrink,
    }
}

/// `{ [name: "…",] boundaries: […], gen: (rng: any) => …, shrink: (v: any) => …,
/// show: (v: any) => __bynkShow(v) }` — the one generator-descriptor object
/// shape [`emit_test_property_function`]/[`emit_test_history_property_function`]/
/// [`emit_contract_attack_function`] each build once per binding/param, now
/// directly from `binding_gen`'s own real `TsExpr` fields (Arc C slice A,
/// #1395) instead of printing them back to text and re-embedding via
/// `format!` — the seam those three functions' own doc comments named as
/// "not yet converted" is closed here. `name` is absent for a history
/// handler's own per-param generator (no `name` key at all in that shape).
fn gen_descriptor_entry(name: Option<TsExpr>, bg: &BindingGen) -> TsExpr {
    let mut entries: Vec<(String, TsExpr)> = Vec::new();
    if let Some(n) = name {
        entries.push(("name".to_string(), n));
    }
    entries.push((
        "boundaries".to_string(),
        TsExpr::array(bg.boundaries.clone()),
    ));
    entries.push((
        "gen".to_string(),
        TsExpr::Arrow {
            params: vec![TsParam {
                name: "rng".to_string(),
                ty: Some(TsType::named("any")),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            // #1397: an unparenthesised object-literal arrow body parses as
            // a block (`{ a: … }` reads `a:` as a statement label, and a
            // bare `{  }` reads as an empty block) — for a record-typed
            // binding, `bg.gen_ts` is `TsExpr::Object` for a non-empty
            // record, or `gen_ts_for_ty`'s own `"{  }"` sentinel `TsExpr::
            // Ident` (its Record arm's double-space-quirk comment) for a
            // zero-field one. Wrap either in `TsExpr::Paren` here, the one
            // choke point all three call sites share. The `Ident` arm
            // matches by leading `{` rather than the sentinel's exact
            // double-space spelling (review of #1425): a valid identifier
            // can never start with `{`, so this is still a no-op for every
            // other `gen_ts` shape, but it keeps matching if that
            // formatting quirk is ever normalised away (the same cleanup
            // #1321/#1327/#1390 already invite for its other two copies).
            body: Box::new(TsArrowBody::Expr(Box::new(match &bg.gen_ts {
                o @ TsExpr::Object { .. } => TsExpr::Paren(Box::new(o.clone())),
                o @ TsExpr::Ident(s) if s.trim_start().starts_with('{') => {
                    TsExpr::Paren(Box::new(o.clone()))
                }
                e => e.clone(),
            }))),
        },
    ));
    entries.push((
        "shrink".to_string(),
        TsExpr::Arrow {
            params: vec![TsParam {
                name: "v".to_string(),
                ty: Some(TsType::named("any")),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(bg.shrink.clone()))),
        },
    ));
    entries.push((
        "show".to_string(),
        TsExpr::Arrow {
            params: vec![TsParam {
                name: "v".to_string(),
                ty: Some(TsType::named("any")),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(call(
                ident("__bynkShow"),
                vec![ident("v")],
            )))),
        },
    ));
    TsExpr::object(entries)
}

/// v0.114: emit one async runner for a generative `property` — the binding
/// generators, the `where` filter and predicate body as closures over the
/// generated tuple, and the `__bynkRunProperty` call that draws cases, shrinks a
/// counterexample, and reports the seed + shrunk tuple + reproduce line.
#[allow(clippy::too_many_arguments)]
fn emit_test_property_function(
    runner_name: &str,
    prop: &PropertyDecl,
    prop_ordinal: usize,
    target_name: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    source: &str,
    rel_path: &str,
    runtime_use: &RuntimeUse,
    // #1860: the run's suite ordinal, when some `stub` is suite-scoped.
    suite: Option<usize>,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<Types>,
) -> TsStmt {
    let mut out = String::new();
    let mut harness_bound = emit_test_scope_setup(
        &mut out,
        "__makeTestDeps",
        target_name,
        target_kind,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        false,
        None,
        suite,
        hidden,
    );

    // Generator descriptors, one per binding, over the target's privileged type
    // view (so refined/opaque constructors resolve).
    let resolved = test_suites::build_privileged_resolved(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    )
    .map(|(r, _)| r);
    // P7.2: deferred, not narrowed. `v`'s real type varies per binding, and
    // for `Int` (and refined-`Int`) bindings the *internal* property-test
    // representation is `bigint` (see `rng.int(...)`'s own `n`-suffixed
    // literals and `__bynkShrinkInt(v: bigint, ...)` above) — not `number`,
    // which is what a general `ts_type_ref`/`ts_ty` rendering of the same
    // declared type would produce. Narrowing correctly needs threading the
    // resolved type (or its internal representation) out of `BindingGen`
    // itself, not a same-line text change, and a wrong guess here risks a
    // real `tsc --strict` failure specifically on the shrink helpers' own
    // typed parameters.
    let gens: Vec<TsExpr> = prop
        .forall
        .bindings
        .iter()
        .map(|b| {
            let bg = resolved
                .as_ref()
                .and_then(|r| checker::resolve_type_ref(&b.type_ref, &r.types, tys).map(|t| (t, r)))
                .map(|(t, r)| binding_gen(t, &r.types, tys))
                .unwrap_or(BindingGen {
                    boundaries: Vec::new(),
                    gen_ts: ident("undefined"),
                    shrink: TsExpr::array(vec![]),
                });
            gen_descriptor_entry(Some(str_lit(b.name.name.clone())), &bg)
        })
        .collect();
    let gens_stmt = TsStmt::const_stmt(
        TsBindingName::Ident("__gens".to_string()),
        None,
        TsExpr::multiline_array(gens),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&gens_stmt, 2));

    // The `where` filter and the predicate body, as closures over the tuple.
    let mut typed = synthetic_typed_commons_for_target(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        hidden,
        tys,
    );
    let cross = bynk_check::resolver::CrossContextInfo::default();
    let binding_names: Vec<String> = prop
        .forall
        .bindings
        .iter()
        .map(|b| b.name.name.clone())
        .collect();
    let binding_types: Vec<Option<checker::TyId>> = prop
        .forall
        .bindings
        .iter()
        .map(|b| {
            resolved
                .as_ref()
                .and_then(|r| checker::resolve_type_ref(&b.type_ref, &r.types, tys))
        })
        .collect();
    let destructure = destructure_vals(
        &binding_names,
        &binding_types,
        resolved.as_ref().map(|r| &r.types),
        tys,
    );
    // #1427: the `for all` bindings' own resolved types, in scope for both
    // the `where`-clause and body re-type-check passes below — mirrors
    // `emit_test_history_property_function`'s own `run: List[Step]` scope
    // entry, generalised to every binding a property declares.
    let initial_scope: HashMap<String, checker::TyId> = binding_names
        .iter()
        .cloned()
        .zip(binding_types.iter().copied())
        .filter_map(|(name, t)| t.map(|t| (name, t)))
        .collect();

    if let Some(w) = &prop.forall.where_pred {
        let synth = Block {
            statements: Vec::new(),
            tail: Box::new(w.clone()),
            span: w.span,
            tail_leading_comments: Vec::new(),
            implicit_tail: false,
        };
        // #1427: re-type-check the `where` clause, the same pass
        // `emit_test_case_function` already runs for a plain `case` body —
        // without it, `typed.expr_types` has no entry for anything here, so
        // a match's own discriminant type is unresolvable and payload-
        // binding lowering (`positional_field_name`) silently falls back to
        // the generic single-field `"value"` name instead of a variant's
        // own real field name.
        if let Some(r) = resolved.as_ref() {
            let mut throwaway_errors: Vec<CompileError> = Vec::new();
            let mut throwaway_refs = RefSink::new();
            let (where_types, where_callees) = test_suites::typecheck_case_body(
                target_name,
                &synth,
                w.span,
                unit_tables,
                r,
                &mut throwaway_errors,
                &mut throwaway_refs,
                initial_scope.clone(),
                tys,
            );
            typed.expr_types.extend(where_types);
            typed.callees.extend(where_callees);
        }
        let (src, _) = emitter::lower_block_to_async_body(
            &synth,
            &TypeRef::Base(BaseType::Bool, w.span),
            &mut typed,
            &cross,
            runtime_use,
        );
        // P7.2: deferred — `__vals`'s elements are heterogeneous, one per binding,
        // some internally `bigint`-represented; same reason as `__gens`'s own
        // construction elsewhere in this function.
        out.push_str("    const __where = (__vals: any[]) => {\n");
        out.push_str(&format!("      {destructure}\n"));
        for line in src.lines() {
            out.push_str("      ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("    };\n");
    } else {
        let where_null = TsStmt::const_stmt(
            TsBindingName::Ident("__where".to_string()),
            None,
            TsExpr::Lit(TsLit::Null),
            None,
        );
        out.push_str(&bynk_ts::print_stmt(&where_null, 2));
    }

    let test_services: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.services.keys().cloned().collect())
        .unwrap_or_default();
    let test_agents: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.agents.keys().cloned().collect())
        .unwrap_or_default();
    // #1427: re-type-check the property body itself — same reason and same
    // pass as the `where` clause above.
    if let Some(r) = resolved.as_ref() {
        let mut throwaway_errors: Vec<CompileError> = Vec::new();
        let mut throwaway_refs = RefSink::new();
        let (body_types, body_callees) = test_suites::typecheck_case_body(
            target_name,
            &prop.forall.body,
            prop.span,
            unit_tables,
            r,
            &mut throwaway_errors,
            &mut throwaway_refs,
            initial_scope,
            tys,
        );
        typed.expr_types.extend(body_types);
        typed.callees.extend(body_callees);
    }
    // Property bodies are collaborator-free predicate scaffolding; like mock op
    // bodies, their source map is a deliberate scope cut (the `expect` location
    // still binds through `assert_loc`).
    // #2024: the `for all` binders share the body closure's scope.
    harness_bound.extend(binding_names.iter().cloned());
    let (body_src, _body_smb) = emitter::lower_test_case_body(
        &prop.forall.body,
        &mut typed,
        &cross,
        test_services,
        target_service_handler_kinds(unit_tables.get(target_name)),
        target_sum_handlers(unit_tables.get(target_name)),
        test_agents,
        target_uses_send(target_name, target_kind, unit_tables),
        harness_bound,
        source,
        rel_path,
        runtime_use,
    );
    // P7.2: deferred, same reason as `__where`'s own `__vals` above.
    out.push_str("    const __body = async (__vals: any[]) => {\n");
    out.push_str(&format!("      {destructure}\n"));
    for line in body_src.lines() {
        out.push_str("      ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("    };\n");

    // Forward slashes so the emitted location/reproduce path is portable — on
    // Windows a `PathBuf` join yields `\`, which must not leak into the golden
    // `.ts` (mirrors `discovered_location`'s normalisation).
    let rel_path_fwd = rel_path.replace('\\', "/");
    let return_run = TsStmt::return_stmt(
        Some(await_expr(call(
            ident("__bynkRunProperty"),
            vec![TsExpr::object(vec![
                (
                    "seed".to_string(),
                    call(
                        ident("__bynkMix"),
                        vec![ident("__bynkSeed"), num_lit(prop_ordinal.to_string())],
                    ),
                ),
                ("cases".to_string(), num_lit("100")),
                ("gens".to_string(), ident("__gens")),
                ("where".to_string(), ident("__where")),
                ("body".to_string(), ident("__body")),
                ("name".to_string(), str_lit(prop.name.clone())),
                ("location".to_string(), str_lit(rel_path_fwd.clone())),
                ("file".to_string(), str_lit(rel_path_fwd)),
            ])],
        ))),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&return_run, 2));
    async_test_runner_fn(runner_name, out, None)
}

/// v0.119 (testing track slice 7, ADR 0155): emit one async runner for a history
/// property — `for all run: History[Agent]`. Builds a per-handler argument
/// generator table, a predicate `__body` over the driven `run` (a `List[Step]`),
/// and a `__drive` closure calling the agent module's `__bynkDriveHistory_<Agent>`,
/// then hands them to `__bynkRunHistory`, which generates, drives, shrinks, and
/// reports exactly like a value `property`. Mirrors [`emit_test_property_function`].
#[allow(clippy::too_many_arguments)]
fn emit_test_history_property_function(
    runner_name: &str,
    prop: &PropertyDecl,
    prop_ordinal: usize,
    target_name: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    source: &str,
    rel_path: &str,
    runtime_use: &RuntimeUse,
    // #1860: the run's suite ordinal, when some `stub` is suite-scoped.
    suite: Option<usize>,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<Types>,
) -> TsStmt {
    let mut out = String::new();
    let mut harness_bound = emit_test_scope_setup(
        &mut out,
        "__makeTestDeps",
        target_name,
        target_kind,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        false,
        None,
        suite,
        hidden,
    );

    let Some((run_var, agent_name)) = prop_history_binding(prop) else {
        // Defensive: the checker rejects a malformed history property before emit,
        // so this is unreachable — emit a trivially-passing runner rather than panic.
        let return_pass_true = TsStmt::return_stmt(
            Some(TsExpr::object(vec![(
                "pass".to_string(),
                TsExpr::Lit(TsLit::Bool(true)),
            )])),
            None,
        );
        out.push_str(&bynk_ts::print_stmt(&return_pass_true, 2));
        return async_test_runner_fn(runner_name, out, None);
    };

    // The synthesised call/step/state types are checker-only (never emitted as
    // real TS), but the lowered predicate annotates the driven history with them.
    // Alias each to `any` so the driven plain objects type-check structurally.
    out.push_str(&format!(
        "    type __History_{agent_name}_Step = any; type __History_{agent_name}_Call = any; type __History_{agent_name}_State = any;\n"
    ));

    // The privileged view, plus the synthetic call/step/state types and the body's
    // expr types (with `run: List[Step]` in scope), so the lowering resolves the
    // predicate's `List` and value surface (`.call is …`, `.old`/`.new`, `.upTo`).
    let mut typed = synthetic_typed_commons_for_target(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        hidden,
        tys,
    );
    let mut handler_descs: Vec<TsExpr> = Vec::new();
    if let Some((mut resolved, _)) = test_suites::build_privileged_resolved(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    ) {
        test_suites::register_call_record_types(&mut resolved, target_name, unit_tables);
        let inner = TypeRef::Named(Ident {
            name: agent_name.to_string(),
            span: prop.span,
        });
        let mut throwaway_refs = RefSink::new();
        let step_ty = test_suites::check_history_binding(
            &inner,
            prop.span,
            &mut resolved,
            &mut throwaway_refs,
            tys,
        )
        .ok();

        // Per-handler argument generators (the slice-2 value generator over each
        // handler parameter), in declaration order — the sequence generator picks a
        // handler uniformly and draws its arguments here.
        if let Some(agent) = resolved.agents.get(agent_name) {
            for h in test_suites::history_handlers(agent) {
                let tag = test_suites::history_variant_name(&h.method_name.as_ref().unwrap().name);
                // P7.2: deferred — same reason as `__gens`'s own construction
                // above (`Int`'s internal `bigint` representation).
                let gens: Vec<TsExpr> = h
                    .params
                    .iter()
                    .map(|p| {
                        let bg = checker::resolve_type_ref(&p.type_ref, &resolved.types, tys)
                            .map(|t| binding_gen(t, &resolved.types, tys))
                            .unwrap_or(BindingGen {
                                boundaries: Vec::new(),
                                gen_ts: ident("undefined"),
                                shrink: TsExpr::array(vec![]),
                            });
                        gen_descriptor_entry(None, &bg)
                    })
                    .collect();
                handler_descs.push(TsExpr::object(vec![
                    ("tag".to_string(), str_lit(tag)),
                    ("gens".to_string(), TsExpr::array(gens)),
                ]));
            }
        }

        if let Some(step_ty) = step_ty {
            let mut scope: HashMap<String, checker::TyId> = HashMap::new();
            scope.insert(run_var.to_string(), tys.intern(step_ty));
            let mut throwaway_errors: Vec<CompileError> = Vec::new();
            let mut throwaway_refs2 = RefSink::new();
            // P6.21 review: `callees` is real output here now — see
            // `typecheck_case_body`'s own doc comment.
            (typed.expr_types, typed.callees) = test_suites::typecheck_case_body(
                target_name,
                &prop.forall.body,
                prop.span,
                unit_tables,
                &resolved,
                &mut throwaway_errors,
                &mut throwaway_refs2,
                scope,
                tys,
            );
        }
        // Carry the synthetic call/step/state types into the lowering commons so
        // `is` on the call sum and field access on the step resolve.
        for (n, d) in &resolved.types {
            typed.types.entry(n.clone()).or_insert_with(|| d.clone());
        }
    }

    let handlers_stmt = TsStmt::const_stmt(
        TsBindingName::Ident("__handlers".to_string()),
        None,
        TsExpr::multiline_array(handler_descs),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&handlers_stmt, 2));

    // The predicate body, as a closure over the driven history `run`.
    let cross = bynk_check::resolver::CrossContextInfo::default();
    let test_services: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.services.keys().cloned().collect())
        .unwrap_or_default();
    let test_agents: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.agents.keys().cloned().collect())
        .unwrap_or_default();
    // #2024: the driven history's binder shares the body closure's scope.
    harness_bound.insert(run_var.to_string());
    let (body_src, _body_smb) = emitter::lower_test_case_body(
        &prop.forall.body,
        &mut typed,
        &cross,
        test_services,
        target_service_handler_kinds(unit_tables.get(target_name)),
        target_sum_handlers(unit_tables.get(target_name)),
        test_agents,
        target_uses_send(target_name, target_kind, unit_tables),
        harness_bound,
        source,
        rel_path,
        runtime_use,
    );
    // P7.2: matches `__bynkDriveHistory_*`'s own real signature — see
    // `emit.rs`'s own driver-signature narrowing and the matching `__drive`
    // call site below (`{target_ns}.__bynkDriveHistory_{agent_name}`).
    out.push_str(
        "    const __body = async (__run: globalThis.Array<{ h: number, args: unknown[] }>) => {\n",
    );
    out.push_str(&format!("      const {run_var} = __run;\n"));
    for line in body_src.lines() {
        out.push_str("      ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("    };\n");

    // Drive a generated sequence through the real handlers via the agent module's
    // exported test driver, threading the test `deps` (real or `stub`-stubbed).
    let target_ns = test_ns(target_name);
    // P7.2: `seq` matches `__bynkDriveHistory_*`'s own real param type
    // (`emit.rs`'s own driver-signature narrowing). `(target_ns as any)` and
    // `deps` stay deferred — the callee's own `deps: any` param is itself
    // deferred (per-handler `given` sets can differ; see `emit.rs`), and
    // `target_ns`'s own generated namespace shape wasn't traced here. The
    // `Array<{ h: number, args: unknown[] }>` param type stays one opaque
    // `TsType::named` string — its own inline object type's real member
    // separator is `, ` (a comma), not `TsType::Object`'s own established
    // `; ` (semicolon) convention, so building it as a real `TsType::Object`
    // would silently change the emitted bytes; the same "odd, one-off shape
    // stays opaque text" posture this track already uses for `Query[T]`/
    // `this.state.storage.get<T>`/`ReturnType<typeof X>`.
    let drive_stmt = TsStmt::const_stmt(
        TsBindingName::Ident("__drive".to_string()),
        None,
        TsExpr::Arrow {
            params: vec![TsParam {
                name: "seq".to_string(),
                ty: Some(TsType::named(
                    "globalThis.Array<{ h: number, args: unknown[] }>",
                )),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(method_call(
                TsExpr::As {
                    expr: Box::new(ident(target_ns)),
                    ty: TsType::named("any"),
                },
                &format!("__bynkDriveHistory_{agent_name}"),
                vec![ident("seq"), ident("deps")],
            )))),
        },
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&drive_stmt, 2));

    let rel_path_fwd = rel_path.replace('\\', "/");
    let return_run = TsStmt::return_stmt(
        Some(await_expr(call(
            ident("__bynkRunHistory"),
            vec![TsExpr::object(vec![
                (
                    "seed".to_string(),
                    call(
                        ident("__bynkMix"),
                        vec![ident("__bynkSeed"), num_lit(prop_ordinal.to_string())],
                    ),
                ),
                ("cases".to_string(), num_lit("60")),
                ("maxLen".to_string(), num_lit("16")),
                ("handlers".to_string(), ident("__handlers")),
                ("drive".to_string(), ident("__drive")),
                ("body".to_string(), ident("__body")),
                ("name".to_string(), str_lit(prop.name.clone())),
                ("location".to_string(), str_lit(rel_path_fwd.clone())),
                ("file".to_string(), str_lit(rel_path_fwd)),
            ])],
        ))),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&return_run, 2));
    async_test_runner_fn(runner_name, out, None)
}

/// v0.115 (testing track slice 3): emit one async runner that *attacks* a
/// contracted free function — generate arguments over the parameter domains
/// (v0.114 engine), filter by the conjunction of `requires` (`__where`), call
/// the function (`__body`), and let the dev/test call-site guard assert each
/// `ensures`, throwing a shrinkable `BynkContractError` on violation. Mirrors
/// [`emit_test_property_function`], but the body is a direct call (the guard is
/// the assertion) with `Int` arguments coerced to `number` (the generator makes
/// `bigint`; functions do `number` arithmetic).
#[allow(clippy::too_many_arguments)]
fn emit_contract_attack_function(
    runner_name: &str,
    f: &FnDecl,
    resolved: &ResolvedCommons,
    prop_ordinal: usize,
    target_name: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    rel_path: &str,
    runtime_use: &RuntimeUse,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<Types>,
) -> Option<TsStmt> {
    let FnName::Free(fname) = &f.name else {
        return None;
    };
    let mut out = String::new();
    // The attack lowers no case body, so it has no `let` to rename.
    let _ = emit_test_scope_setup(
        &mut out,
        "__makeTestDeps",
        target_name,
        target_kind,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        false,
        None,
        None,
        hidden,
    );
    let _ = target_kind;

    // Generator descriptors, one per parameter, over the target's privileged view.
    //
    // P7.2: deferred, not narrowed. `v`'s real type varies per binding, and
    // for `Int` (and refined-`Int`) bindings the *internal* property-test
    // representation is `bigint` (see `rng.int(...)`'s own `n`-suffixed
    // literals and `__bynkShrinkInt(v: bigint, ...)` above) — not `number`,
    // which is what a general `ts_type_ref`/`ts_ty` rendering of the same
    // declared type would produce. Narrowing correctly needs threading the
    // resolved type (or its internal representation) out of `BindingGen`
    // itself, not a same-line text change, and a wrong guess here risks a
    // real `tsc --strict` failure specifically on the shrink helpers' own
    // typed parameters.
    let gens: Vec<TsExpr> = f
        .params
        .iter()
        .map(|p| {
            let bg = checker::resolve_type_ref(&p.type_ref, &resolved.types, tys)
                .map(|t| binding_gen(t, &resolved.types, tys))
                .unwrap_or(BindingGen {
                    boundaries: Vec::new(),
                    gen_ts: ident("undefined"),
                    shrink: TsExpr::array(vec![]),
                });
            gen_descriptor_entry(Some(str_lit(p.name.name.clone())), &bg)
        })
        .collect();
    let gens_stmt = TsStmt::const_stmt(
        TsBindingName::Ident("__gens".to_string()),
        None,
        TsExpr::multiline_array(gens),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&gens_stmt, 2));

    let param_names: Vec<String> = f.params.iter().map(|p| p.name.name.clone()).collect();
    let param_types: Vec<Option<checker::TyId>> = f
        .params
        .iter()
        .map(|p| checker::resolve_type_ref(&p.type_ref, &resolved.types, tys))
        .collect();
    // #1426: the same top-level Int-binding representation gap `emit_test_
    // property_function`'s own `for all` bindings have — an attacked
    // function's own fuzzed `Int` parameter draws `bigint` exactly the same
    // way, and its `requires`/body can do ordinary arithmetic on it.
    let destructure = destructure_vals(&param_names, &param_types, Some(&resolved.types), tys);

    // `__where` — the conjunction of `requires`, lowered over the parameter
    // tuple. #1426: `destructure` above already coerces every `Int`-drawing
    // param to `number` before this closure's own body runs, so a `requires`
    // clause doing real arithmetic (not just comparison) on a param is safe
    // too — comparisons alone tolerated the pre-fix `bigint`/`number` split,
    // but arithmetic never did.
    let where_pred = f.requires.iter().rev().fold(None, |acc: Option<Expr>, c| {
        Some(match acc {
            None => c.predicate.clone(),
            Some(rest) => Expr {
                id: ExprId::SYNTHETIC,
                kind: ExprKind::BinOp(BinOp::And, Box::new(c.predicate.clone()), Box::new(rest)),
                span: f.span,
            },
        })
    });
    if let Some(w) = where_pred {
        let mut typed = synthetic_typed_commons_for_target(
            target_name,
            unit_tables,
            unit_uses,
            unit_consumes,
            hidden,
            tys,
        );
        let cross = bynk_check::resolver::CrossContextInfo::default();
        let synth = Block {
            statements: Vec::new(),
            tail: Box::new(w.clone()),
            span: w.span,
            tail_leading_comments: Vec::new(),
            implicit_tail: false,
        };
        let (src, _) = emitter::lower_block_to_async_body(
            &synth,
            &TypeRef::Base(BaseType::Bool, w.span),
            &mut typed,
            &cross,
            runtime_use,
        );
        // P7.2: deferred — `__vals`'s elements are heterogeneous, one per binding,
        // some internally `bigint`-represented; same reason as `__gens`'s own
        // construction elsewhere in this function.
        out.push_str("    const __where = (__vals: any[]) => {\n");
        out.push_str(&format!("      {destructure}\n"));
        for line in src.lines() {
            out.push_str("      ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("    };\n");
    } else {
        let where_null = TsStmt::const_stmt(
            TsBindingName::Ident("__where".to_string()),
            None,
            TsExpr::Lit(TsLit::Null),
            None,
        );
        out.push_str(&bynk_ts::print_stmt(&where_null, 2));
    }

    // `__body` — call the (guarded) function. #1426: every param is already
    // coerced to `number` by `destructure_vals` at the bind site above (an
    // `Int`-drawing local is never left `bigint`), so the call passes each
    // param straight through — no separate per-argument coercion needed here
    // any more (this used to `Number(…)`-wrap `Int` args itself; redundant
    // now that the bind site handles it, and would double-wrap). The guard
    // asserts the `ensures`.
    let call_args: Vec<String> = f.params.iter().map(|p| p.name.name.clone()).collect();
    // P7.2: deferred, same reason as `__where`'s own `__vals` above.
    out.push_str("    const __body = async (__vals: any[]) => {\n");
    out.push_str(&format!("      {destructure}\n"));
    out.push_str(&format!(
        "      {}({});\n",
        fname.name,
        call_args.join(", ")
    ));
    out.push_str("    };\n");

    let rel_path_fwd = rel_path.replace('\\', "/");
    let name = format!("contract {}", fname.name);
    let return_run = TsStmt::return_stmt(
        Some(await_expr(call(
            ident("__bynkRunProperty"),
            vec![TsExpr::object(vec![
                (
                    "seed".to_string(),
                    call(
                        ident("__bynkMix"),
                        vec![ident("__bynkSeed"), num_lit(prop_ordinal.to_string())],
                    ),
                ),
                ("cases".to_string(), num_lit("100")),
                ("gens".to_string(), ident("__gens")),
                ("where".to_string(), ident("__where")),
                ("body".to_string(), ident("__body")),
                ("name".to_string(), str_lit(name)),
                ("location".to_string(), str_lit(rel_path_fwd.clone())),
                ("file".to_string(), str_lit(rel_path_fwd)),
            ])],
        ))),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&return_run, 2));
    Some(async_test_runner_fn(runner_name, out, None))
}

// -- Small tree-construction helpers (#1325) ------------------------------
//
// Mirrors `workers.rs`'s/`workers_entry.rs`'s own local helper sets (#1321/
// #1323) — this file's own private set, not shared, matching this track's
// own established per-file scoping.

fn ident(s: impl Into<String>) -> TsExpr {
    TsExpr::Ident(s.into())
}

fn str_lit(s: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Str(s.into()))
}

fn num_lit(n: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Num(n.into()))
}

fn member(object: TsExpr, property: impl Into<String>) -> TsExpr {
    TsExpr::Member {
        object: Box::new(object),
        property: property.into(),
    }
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

fn await_expr(expr: TsExpr) -> TsExpr {
    TsExpr::Await(Box::new(expr))
}

/// A zero-substitution template literal — `` `<text>` `` with `text` already
/// fully assembled (any `${...}` it contains is baked-in literal text, not a
/// real substitution `bynk_ts` tracks). Slice F (#1407): `emit_system_http_
/// support`'s own `concrete_path` (every `:name` path segment already
/// resolved to `${paramIdent}` text at Rust-`format!` time, per its own doc)
/// is the real site. Routes through the validated `TsExpr::template_lit`
/// constructor rather than the bare `TsExpr::TemplateLit { .. }` variant, the
/// same guard this file's own not-yet-refactored call sites already use
/// (review of #1408, finding 1).
fn template(text: impl Into<String>) -> TsExpr {
    TsExpr::template_lit(vec![text.into()], vec![])
}

/// One `async function __sysdrive_*` driver: an optional lead statement
/// (`const __body = JSON.stringify(...)`, typed/no-auth drivers only), `const
/// __h = __makeHarness();`, `const __req = new Request(<url>, <options>);`,
/// `const __res = await __h.env.<binding>.fetch(__req);`, then `return
/// <decode_fn>(__res, <payload>);`. Slice F (#1407): the one real shape
/// shared by all four per-route `__sysdrive_{,raw_,noauth_,rawnoauth_}*`
/// drivers in `emit_system_http_support`, differing only in name (built here
/// from `kind_prefix`/`sname`/`key`, one shared spot rather than one
/// `format!` call per driver kind), params, the optional body statement, the
/// options object (Decision A, see its own call sites), and the decode
/// function — every real difference is a parameter here, not a reason to
/// hand-write four near-identical bodies.
#[allow(clippy::too_many_arguments)]
fn sysdrive_driver(
    kind_prefix: &str,
    sname: &str,
    key: &str,
    mut params: Vec<TsParam>,
    body_stmt: Option<TsStmt>,
    url: TsExpr,
    options: TsExpr,
    binding: &str,
    decode_fn: &str,
    payload: &str,
    signs_claims: bool,
    // #2135: a `Signature` route's signing statements (`const __sig = …`,
    // or #2155 a sum's per-call `__auth`), after the body they sign is
    // serialised and before the request is built.
    sign_stmts: Vec<TsStmt>,
    // #2155: whether the driver takes the actor the call acts as (`__as`), on
    // an actor-sum route.
    takes_as: bool,
) -> TsStmt {
    params.push(TsParam {
        name: "__sub".to_string(),
        ty: Some(TsType::named("string")),
        optional: false,
    });
    // #2019: a refinement-guarded route signs the claims the acting actor
    // presents (`{}` for a non-refinement actor) next to `sub`.
    if signs_claims {
        params.push(TsParam {
            name: "__claims".to_string(),
            ty: Some(TsType::named_with_args(
                "globalThis.Record",
                vec![TsType::named("string"), TsType::named("unknown")],
            )),
            optional: false,
        });
    }
    if takes_as {
        params.push(TsParam {
            name: "__as".to_string(),
            ty: Some(TsType::named("string")),
            optional: false,
        });
    }
    let mut body = Vec::new();
    if let Some(stmt) = body_stmt {
        body.push(stmt);
    }
    body.extend(sign_stmts);
    body.push(TsStmt::const_stmt(
        TsBindingName::Ident("__h".to_string()),
        None,
        call(ident("__makeHarness"), vec![]),
        None,
    ));
    body.push(TsStmt::const_stmt(
        TsBindingName::Ident("__req".to_string()),
        None,
        TsExpr::New {
            callee: Box::new(ident("globalThis.Request")),
            args: vec![url, options],
        },
        None,
    ));
    body.push(TsStmt::const_stmt(
        TsBindingName::Ident("__res".to_string()),
        None,
        await_expr(method_call(
            member(member(ident("__h"), "env"), binding),
            "fetch",
            vec![ident("__req")],
        )),
        None,
    ));
    body.push(TsStmt::return_stmt(
        Some(call(ident(decode_fn), vec![ident("__res"), ident(payload)])),
        None,
    ));
    TsStmt::decl(
        TsDecl::Function {
            name: format!("__sysdrive_{kind_prefix}{sname}_{key}"),
            generics: Vec::new(),
            params,
            return_type: None,
            body,
            is_async: true,
            inline: false,
        },
        None,
    )
}

fn and_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::And,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn gt(left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::GreaterThan,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn strict_eq(left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::StrictEq,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn or_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::Or,
        left: Box::new(left),
        right: Box::new(right),
    }
}

/// The module's own runner, built from its real per-case/per-property/per-attack
/// `(name, runner)` pairs, in dispatch order:
///
/// ```ts
/// export const __bynkCases: readonly string[] = ["a", "b"];
/// export async function run(only?: string, onCase?: (a0: CaseResult) => void) {
///   const results = [];
///   const want = (n: string): boolean => only === undefined || only === n;
///   if (want("a")) {
///     const started = __bynkCaseNow();
///     const r = { name: "a", ...(await test_a()), durationMs: __bynkCaseSince(started) };
///     results.push(r);
///     if (onCase) onCase(r);
///   }
///   return results;
/// }
/// ```
///
/// `onCase` is called as each case finishes, so a consumer sees a result before the
/// next case runs (#2267 stage 2), and `__bynkCases` lists the case names so the
/// entry can count what a filter selects before anything runs. The case result carries
/// `durationMs`, measured around the runner. Slice G (#1409): carries no opaque lowered
/// content and no source-map splice sensitivity (unlike the per-case bodies each
/// dispatch calls out to), so it converts fully, shared by both
/// `emit_integration_module` and `emit_test_module`.
///
/// `name` is the raw, decoded case name: `str_lit` routes it through `TsLit::Str`'s own
/// printer escaping, which alone owns escaping (review of #1410, finding 1: an
/// already-escaped name would be escaped twice). The `...(await …())` spread wraps its
/// `Await` in an explicit `TsExpr::Paren`, since `TsObjectEntry::Spread` renders its
/// inner expression without operand parenthesisation.
fn build_run_function(entries: &[(String, String)]) -> Vec<TsStmt> {
    let dispatch = |name: &str, runner_name: &str| {
        TsStmt::if_stmt(
            call(ident("want"), vec![str_lit(name.to_string())]),
            block(vec![
                const_("started", call(ident("__bynkCaseNow"), vec![])),
                const_(
                    "r",
                    TsExpr::object_entries(vec![
                        TsObjectEntry::Prop("name".to_string(), str_lit(name.to_string())),
                        TsObjectEntry::Spread(TsExpr::Paren(Box::new(await_expr(call(
                            ident(runner_name),
                            vec![],
                        ))))),
                        TsObjectEntry::Prop(
                            "durationMs".to_string(),
                            call(ident("__bynkCaseSince"), vec![ident("started")]),
                        ),
                    ]),
                ),
                expr_stmt(method_call(ident("results"), "push", vec![ident("r")])),
                if_(
                    ident("onCase"),
                    expr_stmt(call(ident("onCase"), vec![ident("r")])),
                ),
            ]),
            None,
        )
    };
    let mut body = vec![
        const_("results", TsExpr::array(vec![])),
        const_(
            "want",
            TsExpr::Arrow {
                params: vec![TsParam {
                    name: "n".to_string(),
                    ty: Some(TsType::named("string")),
                    optional: false,
                }],
                is_async: false,
                generics: Vec::new(),
                return_type: Some(TsType::named("boolean")),
                body: Box::new(TsArrowBody::Expr(Box::new(or_expr(
                    strict_eq(ident("only"), ident("undefined")),
                    strict_eq(ident("only"), ident("n")),
                )))),
            },
        ),
    ];
    body.extend(entries.iter().map(|(name, runner)| dispatch(name, runner)));
    body.push(TsStmt::return_stmt(Some(ident("results")), None));
    let names = entries
        .iter()
        .map(|(name, _)| format!("\"{}\"", crate::emitter::escape_ts_string(name)))
        .collect::<Vec<_>>()
        .join(", ");
    vec![
        TsStmt::raw(CASE_TIMING_RUNTIME, None),
        TsStmt::raw(
            format!("export const __bynkCases: readonly string[] = [{names}];"),
            None,
        ),
        TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::Function {
                name: "run".to_string(),
                generics: Vec::new(),
                params: vec![
                    TsParam {
                        name: "only".to_string(),
                        ty: Some(TsType::named("string")),
                        optional: true,
                    },
                    TsParam {
                        name: "onCase".to_string(),
                        ty: Some(TsType::Fn {
                            params: vec![TsType::named("CaseResult")],
                            ret: Box::new(TsType::named("void")),
                        }),
                        optional: true,
                    },
                ],
                return_type: None,
                body,
                is_async: true,
                inline: false,
            })),
            None,
        ),
    ]
}

/// What every module's `run` shares (#2267 stage 2): the shape of a finished case
/// handed to `onCase`, and the clock that measures it. `performance.now()` where the
/// host has one (sub-millisecond), else `Date.now()`. It is read through
/// `globalThis`, so no DOM or Node typings are assumed, and is the host's own clock,
/// not the harness-owned test clock (`advance Clock`).
const CASE_TIMING_RUNTIME: &str = r#"type CaseResult = {
  name: string;
  pass: boolean;
  error?: { message: string; location: string; violation?: { kind: string; agent: string; name: string; location: string }; refusals?: { index: number; call: string; kind: string; agent: string; name: string; location: string }[] };
  durationMs: number;
  retries?: number;
  kind?: string;
  seed?: string;
  counterexample?: string;
};
const __bynkCasePerformance = (globalThis as unknown as { performance?: { now(): number } }).performance;
const __bynkCaseNow = (): number => (__bynkCasePerformance ? __bynkCasePerformance.now() : Date.now());
const __bynkCaseSince = (started: number): number => Math.round((__bynkCaseNow() - started) * 100) / 100;"#;

/// `test ? consequent : alternate`.
fn cond_expr(test: TsExpr, consequent: TsExpr, alternate: TsExpr) -> TsExpr {
    TsExpr::Conditional {
        test: Box::new(test),
        consequent: Box::new(consequent),
        alternate: Box::new(alternate),
    }
}

fn console_log(arg: TsExpr) -> TsStmt {
    expr_stmt(method_call(ident("globalThis.console"), "log", vec![arg]))
}

fn const_(name: impl Into<String>, init: TsExpr) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.into()), None, init, None)
}

fn expr_stmt(expr: TsExpr) -> TsStmt {
    TsStmt::expr_stmt(expr, None)
}

fn block(stmts: Vec<TsStmt>) -> TsStmt {
    TsStmt::block(stmts, None)
}

fn if_(cond: TsExpr, then_branch: TsStmt) -> TsStmt {
    TsStmt::if_stmt(cond, then_branch, None)
}

pub(crate) fn emit_test_main(tests: &[RunnableTest], import_ext: ImportExt) -> TsProgram {
    let ext = import_ext.as_str();
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynkc — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::comment("top-level test runner", None));
    // Node's `process` global isn't declared without @types/node. The runner
    // uses `process.exit` and reads `process.env.BYNK_TEST_FORMAT` (v0.59: set
    // to `ndjson` by `bynkc test --format json`), so we narrow the global with a
    // minimal ambient declaration rather than pulling in a dependency.
    program.push(TsStmt::decl(
        TsDecl::DeclareConst {
            name: "process".to_string(),
            ty: TsType::Object(vec![
                TsTypeMember::method(
                    "exit",
                    vec![TsParam {
                        name: "code".to_string(),
                        ty: Some(TsType::named("number")),
                        optional: false,
                    }],
                    TsType::named("never"),
                ),
                TsTypeMember::prop(
                    "env",
                    TsType::Object(vec![TsTypeMember::index(
                        "k",
                        TsType::named("string"),
                        TsType::union(vec![TsType::named("string"), TsType::named("undefined")]),
                    )]),
                ),
            ]),
        },
        None,
    ));
    let mut sorted: Vec<&RunnableTest> = tests.iter().collect();
    sorted.sort_by(|a, b| a.target_name.cmp(&b.target_name));
    for (i, t) in sorted.iter().enumerate() {
        let module_stem = t
            .module_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("test");
        program.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: format!("test_{i}"),
                from: format!("./{module_stem}.{ext}"),
            },
            None,
        ));
    }

    // v0.127 (editor-currency slice 6), #2273: `bynkc test --case <name>` and
    // `--suite <name>`, each repeatable, set BYNK_TEST_FILTER (see
    // `TEST_FILTER_RUNTIME`). `active` is the modules whose suite is selected;
    // each runs through `runSelected`, which calls its own `run(name)` once per
    // selected case name and concatenates the results, so a module is still one
    // suite record. Unset runs the whole project unchanged.
    let mut main_body: Vec<TsStmt> = Vec::new();
    let modules_items: Vec<TsExpr> = sorted
        .iter()
        .enumerate()
        .map(|(i, t)| {
            TsExpr::object(vec![
                ("name".to_string(), str_lit(t.target_name.clone())),
                (
                    "names".to_string(),
                    member(ident(format!("test_{i}")), "__bynkCases"),
                ),
                ("run".to_string(), member(ident(format!("test_{i}")), "run")),
            ])
        })
        .collect();
    main_body.push(const_("modules", TsExpr::multiline_array(modules_items)));
    main_body.push(TsStmt::let_stmt(
        TsBindingName::Ident("passed".to_string()),
        None,
        Some(num_lit("0")),
        None,
    ));
    main_body.push(TsStmt::let_stmt(
        TsBindingName::Ident("failed".to_string()),
        None,
        Some(num_lit("0")),
        None,
    ));
    // v0.59: `--format json` sets BYNK_TEST_FORMAT=ndjson and captures stdout;
    // the runner then emits one JSON event per line (an internal protocol the
    // driver re-renders into the pinned document). Otherwise the human ✓ / ✗
    // output is byte-for-byte unchanged.
    //
    // `TsLit::Raw`, not `TsLit::Str` — see its own doc: the literal already
    // spells the JS unicode escape as six ASCII characters, which `Str`'s own
    // escaper would double the backslash of.
    main_body.push(const_(
        "PREFIX",
        TsExpr::Lit(TsLit::Raw("\"system \\u00b7 \"".to_string())),
    ));
    main_body.push(const_(
        "active",
        method_call(
            ident("modules"),
            "filter",
            vec![TsExpr::Arrow {
                params: vec![TsParam {
                    name: "m".to_string(),
                    ty: None,
                    optional: false,
                }],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(call(
                    ident("selected"),
                    vec![cond_expr(
                        method_call(
                            member(ident("m"), "name"),
                            "startsWith",
                            vec![ident("PREFIX")],
                        ),
                        method_call(
                            member(ident("m"), "name"),
                            "slice",
                            vec![member(ident("PREFIX"), "length")],
                        ),
                        member(ident("m"), "name"),
                    )],
                )))),
            }],
        ),
    ));

    let emit_call = |entries: Vec<TsObjectEntry>| {
        expr_stmt(call(ident("emit"), vec![TsExpr::object_entries(entries)]))
    };
    // #2138 (track §3.6): a remote run's cases report how often `--settle`
    // retried them.
    let remote = tests.iter().any(|t| t.remote);
    let retries_entry = || {
        remote.then(|| TsObjectEntry::Prop("retries".to_string(), member(ident("r"), "retries")))
    };
    // Track C (#2266): a failed generative case reports how to reproduce it as
    // data — `kind`, `seed` and, when shrinking produced one, `counterexample` —
    // after the fields above. Gated on the build having a generative module, as
    // `retries` is on a remote one, so every other runner is unchanged;
    // `JSON.stringify` drops one a case leaves undefined.
    let generative = tests.iter().any(|t| t.generative);
    let generative_entries = || -> Vec<TsObjectEntry> {
        if !generative {
            return Vec::new();
        }
        // Every module's results are typed as the entry's `Result`, whose fields
        // these are optional, so a build mixing generative and plain modules
        // type-checks.
        ["kind", "seed", "counterexample"]
            .into_iter()
            .map(|k| TsObjectEntry::Prop(k.to_string(), member(ident("r"), k)))
            .collect()
    };

    // #2267 stage 2: how long the case ran, in milliseconds, measured by its module's
    // own `run`. Only the stream carries it; `--format json` keeps its document
    // deterministic and drops it.
    let duration_entry =
        || TsObjectEntry::Prop("durationMs".to_string(), member(ident("r"), "durationMs"));

    let ndjson_branch = block(vec![
        const_(
            "emit",
            TsExpr::Arrow {
                params: vec![TsParam {
                    name: "o".to_string(),
                    ty: Some(TsType::named("unknown")),
                    optional: false,
                }],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(method_call(
                    ident("globalThis.console"),
                    "log",
                    vec![method_call(
                        ident("globalThis.JSON"),
                        "stringify",
                        vec![ident("o")],
                    )],
                )))),
            },
        ),
        emit_call(vec![
            TsObjectEntry::Prop("type".to_string(), str_lit("run-begin")),
            TsObjectEntry::Prop("suites".to_string(), member(ident("active"), "length")),
        ]),
        TsStmt::for_of(
            "m",
            ident("active"),
            block(vec![
                const_(
                    "integration",
                    method_call(
                        member(ident("m"), "name"),
                        "startsWith",
                        vec![ident("PREFIX")],
                    ),
                ),
                const_(
                    "suite",
                    cond_expr(
                        ident("integration"),
                        method_call(
                            member(ident("m"), "name"),
                            "slice",
                            vec![member(ident("PREFIX"), "length")],
                        ),
                        member(ident("m"), "name"),
                    ),
                ),
                const_(
                    "kind",
                    cond_expr(
                        ident("integration"),
                        str_lit("integration"),
                        str_lit("unit"),
                    ),
                ),
                emit_call(vec![
                    TsObjectEntry::Prop("type".to_string(), str_lit("suite-begin")),
                    TsObjectEntry::Prop("name".to_string(), ident("suite")),
                    TsObjectEntry::Shorthand("kind".to_string()),
                    TsObjectEntry::Prop(
                        "tests".to_string(),
                        call(ident("planned"), vec![member(ident("m"), "names")]),
                    ),
                ]),
                TsStmt::decl(
                    TsDecl::Function {
                        name: "report".to_string(),
                        generics: Vec::new(),
                        params: vec![TsParam {
                            name: "r".to_string(),
                            ty: Some(TsType::named("Result")),
                            optional: false,
                        }],
                        return_type: None,
                        body: vec![TsStmt::if_else_same_line_stmt(
                            member(ident("r"), "pass"),
                            block(vec![
                                TsStmt::increment(ident("passed"), None),
                                emit_call(
                                    vec![
                                        TsObjectEntry::Prop("type".to_string(), str_lit("case")),
                                        TsObjectEntry::Shorthand("suite".to_string()),
                                        TsObjectEntry::Prop(
                                            "name".to_string(),
                                            member(ident("r"), "name"),
                                        ),
                                        TsObjectEntry::Prop("outcome".to_string(), str_lit("pass")),
                                    ]
                                    .into_iter()
                                    .chain(retries_entry())
                                    .chain(std::iter::once(duration_entry()))
                                    .collect(),
                                ),
                            ]),
                            block(vec![
                                TsStmt::increment(ident("failed"), None),
                                emit_call(
                                    vec![
                                        TsObjectEntry::Prop("type".to_string(), str_lit("case")),
                                        TsObjectEntry::Shorthand("suite".to_string()),
                                        TsObjectEntry::Prop(
                                            "name".to_string(),
                                            member(ident("r"), "name"),
                                        ),
                                        TsObjectEntry::Prop("outcome".to_string(), str_lit("fail")),
                                        TsObjectEntry::Prop(
                                            "message".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "message"),
                                            ),
                                        ),
                                        TsObjectEntry::Prop(
                                            "location".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "location"),
                                            ),
                                        ),
                                        // #2272: the declaration a violated invariant or transition
                                        // names; absent (`JSON.stringify` drops it) otherwise.
                                        TsObjectEntry::Prop(
                                            "violation".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "violation"),
                                            ),
                                        ),
                                        // #2272: the steps of a failed history that an agent refused.
                                        TsObjectEntry::Prop(
                                            "refusals".to_string(),
                                            and_expr(
                                                member(ident("r"), "error"),
                                                member(member(ident("r"), "error"), "refusals"),
                                            ),
                                        ),
                                    ]
                                    .into_iter()
                                    .chain(retries_entry())
                                    .chain(generative_entries())
                                    .chain(std::iter::once(duration_entry()))
                                    .collect(),
                                ),
                            ]),
                            None,
                        )],
                        is_async: false,
                        inline: false,
                    },
                    None,
                ),
                expr_stmt(await_expr(call(
                    ident("runSelected"),
                    vec![member(ident("m"), "run"), ident("report")],
                ))),
                emit_call(vec![
                    TsObjectEntry::Prop("type".to_string(), str_lit("suite-end")),
                    TsObjectEntry::Prop("name".to_string(), ident("suite")),
                ]),
            ]),
            None,
        ),
        emit_call(vec![
            TsObjectEntry::Prop("type".to_string(), str_lit("run-end")),
            TsObjectEntry::Shorthand("passed".to_string()),
            TsObjectEntry::Shorthand("failed".to_string()),
            // #2273: the `--suite`/`--case` values that selected nothing; absent
            // (`JSON.stringify` drops an undefined) unless a filter went unmatched.
            TsObjectEntry::Prop("unmatched".to_string(), call(ident("unmatched"), vec![])),
        ]),
    ]);

    let human_branch = block(vec![
        console_log(str_lit("Running tests...\n")),
        TsStmt::for_of(
            "m",
            ident("active"),
            block(vec![
                console_log(TsExpr::template_lit(
                    vec![String::new(), ":".to_string()],
                    vec![member(ident("m"), "name")],
                )),
                TsStmt::decl(
                    TsDecl::Function {
                        name: "report".to_string(),
                        generics: Vec::new(),
                        params: vec![TsParam {
                            name: "r".to_string(),
                            ty: Some(TsType::named("Result")),
                            optional: false,
                        }],
                        return_type: None,
                        body: vec![TsStmt::if_else_same_line_stmt(
                            member(ident("r"), "pass"),
                            TsStmt::inline_block(
                                vec![
                                    TsStmt::increment(ident("passed"), None),
                                    console_log(TsExpr::template_lit(
                                        vec!["  \\u2713 ".to_string(), String::new()],
                                        vec![member(ident("r"), "name")],
                                    )),
                                ],
                                None,
                            ),
                            TsStmt::inline_block(
                                vec![
                                    TsStmt::increment(ident("failed"), None),
                                    console_log(TsExpr::template_lit(
                                        vec!["  \\u2717 ".to_string(), String::new()],
                                        vec![member(ident("r"), "name")],
                                    )),
                                    if_(
                                        member(ident("r"), "error"),
                                        console_log(TsExpr::template_lit(
                                            vec!["    ".to_string(), String::new()],
                                            vec![member(member(ident("r"), "error"), "message")],
                                        )),
                                    ),
                                ],
                                None,
                            ),
                            None,
                        )],
                        is_async: false,
                        inline: false,
                    },
                    None,
                ),
                expr_stmt(await_expr(call(
                    ident("runSelected"),
                    vec![member(ident("m"), "run"), ident("report")],
                ))),
                console_log(str_lit("")),
            ]),
            None,
        ),
        console_log(TsExpr::template_lit(
            vec![
                String::new(),
                " passed, ".to_string(),
                " failed.".to_string(),
            ],
            vec![ident("passed"), ident("failed")],
        )),
        expr_stmt(call(ident("reportUnmatched"), vec![])),
    ]);

    main_body.push(TsStmt::if_else_same_line_stmt(
        strict_eq(
            member(member(ident("process"), "env"), "BYNK_TEST_FORMAT"),
            str_lit("ndjson"),
        ),
        ndjson_branch,
        human_branch,
        None,
    ));
    // A filter value that selected nothing fails the run, as a failed case does.
    main_body.push(if_(
        or_expr(
            gt(ident("failed"), num_lit("0")),
            TsExpr::Binary {
                op: TsBinaryOp::StrictNotEq,
                left: Box::new(call(ident("unmatched"), vec![])),
                right: Box::new(ident("undefined")),
            },
        ),
        expr_stmt(call(member(ident("process"), "exit"), vec![num_lit("1")])),
    ));

    program.push(TsStmt::raw(TEST_FILTER_RUNTIME, None));
    program.push(TsStmt::decl(
        TsDecl::Function {
            name: "main".to_string(),
            generics: Vec::new(),
            params: vec![],
            return_type: None,
            body: main_body,
            is_async: true,
            inline: false,
        },
        None,
    ));
    program.push(expr_stmt(call(ident("main"), vec![])));
    program
}

/// #2273: the entry's `--suite`/`--case` machinery. `BYNK_TEST_FILTER` is one
/// JSON value (`bynkc` builds it): `suites` and `cases` each combine their values
/// as "or", the two as "and". `selected` keeps a module whose suite is chosen,
/// `runSelected` runs one module's selected cases through its own `run(name)`
/// (the module is unchanged) and concatenates them, and `unmatched` names each
/// value that selected nothing, which is an error unless the run sees only part of
/// the selection (`--against dev` runs one process per `env` group).
const TEST_FILTER_RUNTIME: &str = r#"type Result = {
  name: string;
  pass: boolean;
  error?: { message: string; location: string; violation?: { kind: string; agent: string; name: string; location: string }; refusals?: { index: number; call: string; kind: string; agent: string; name: string; location: string }[] };
  durationMs?: number;
  retries?: number;
  kind?: string;
  seed?: string;
  counterexample?: string;
};
type Filter = {
  program?: string;
  suites?: string[];
  cases?: string[];
  partial?: boolean;
  skipped?: { suite: string; case: string }[];
};
const filter: Filter | undefined =
  process.env.BYNK_TEST_FILTER === undefined ? undefined : globalThis.JSON.parse(process.env.BYNK_TEST_FILTER);
const matchedSuites = new Set<string>();
const matchedCases = new Set<string>();

function selected(suite: string): boolean {
  if (filter?.suites === undefined) return true;
  if (!filter.suites.includes(suite)) return false;
  matchedSuites.add(suite);
  return true;
}

// How many cases the filter selects in a module, known before any of them runs: the
// `tests` of its `suite-begin`. A name is counted once per case that bears it.
function planned(names: readonly string[]): number {
  if (filter?.cases === undefined) return names.length;
  let count = 0;
  for (const name of filter.cases) count += names.filter((n) => n === name).length;
  return count;
}

// Runs a module's selected cases, handing each result to `onCase` as it finishes.
async function runSelected(
  run: (only?: string, onCase?: (r: Result) => void) => Promise<Result[]>,
  onCase: (r: Result) => void,
): Promise<Result[]> {
  if (filter?.cases === undefined) return run(undefined, onCase);
  const results: Result[] = [];
  for (const name of filter.cases) {
    const found = await run(name, onCase);
    if (found.length > 0) matchedCases.add(name);
    results.push(...found);
  }
  return results;
}

// A case the build skipped is not in it but exists, so a value naming one is matched.
for (const s of filter?.skipped ?? []) {
  if (selected(s.suite) && filter?.cases?.includes(s.case)) matchedCases.add(s.case);
}

function unmatched(): { suites?: string[]; cases?: string[] } | undefined {
  if (filter === undefined || filter.partial === true) return undefined;
  const suites = (filter.suites ?? []).filter((s) => !matchedSuites.has(s));
  const cases = (filter.cases ?? []).filter((c) => !matchedCases.has(c));
  if (suites.length === 0 && cases.length === 0) return undefined;
  return { ...(suites.length > 0 ? { suites } : {}), ...(cases.length > 0 ? { cases } : {}) };
}

function reportUnmatched(): void {
  const left = unmatched();
  if (left === undefined) return;
  const program = filter?.program ?? "bynkc";
  for (const s of left.suites ?? []) {
    globalThis.console.error(`${program} test: no suite named --suite ${globalThis.JSON.stringify(s)}`);
  }
  for (const c of left.cases ?? []) {
    globalThis.console.error(`${program} test: no case matched --case ${globalThis.JSON.stringify(c)}`);
  }
}"#;

fn sanitise_case_name(name: &str, index: &mut usize) -> String {
    let mut s = String::from("test_");
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch);
        } else {
            s.push('_');
        }
    }
    if s == "test_" {
        s.push_str(&index.to_string());
    }
    *index += 1;
    s
}

/// #1883, slice 3: the project's declared assets, for the test passes — each
/// declaration's served path ([`bynk_check::assets::declared_served_paths`])
/// and each declared file's bytes. Empty when no `assets/` folder was read.
pub(crate) struct TestAssetFiles<'a> {
    pub(crate) served: &'a BTreeMap<String, String>,
    pub(crate) bytes: &'a BTreeMap<String, Vec<u8>>,
}

/// #1883, slice 3 ([DECISION F]): the TypeScript that lets a case fetch an
/// asset — the files `target`'s Worker ships
/// ([`bynk_check::assets::deployed_assets`]), embedded base64 by served path
/// with the platform's content type, and `__bynkAssetGet(url)`, which answers
/// as the platform's asset tier does: `HttpResult.Raw(bytes, contentType)`
/// for a shipped file, `NotFound` otherwise. Embedding keeps a suite free of
/// disk access at test time; it is harness data, not the compile-time
/// `String` embedding [DECISION E] excludes. `None` when the Worker ships no
/// files, so a module without assets is unchanged.
fn asset_harness(
    target: &str,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_tables: &HashMap<String, UnitTable>,
    files: &TestAssetFiles,
) -> Option<String> {
    let deployed = bynk_check::assets::deployed_assets(
        target,
        unit_uses,
        unit_consumes,
        unit_tables,
        files.served,
    );
    if deployed.is_empty() {
        return None;
    }
    let mut out = String::from(
        "const __bynkAssetFiles: Record<string, { readonly body: string; readonly contentType: string }> = {\n",
    );
    for (served, source) in &deployed {
        // Review of #1922: every deployed file was read (`asset_served`
        // comes from the same bytes), so a miss is a broken invariant, not
        // something to degrade quietly into an undefined `__bynkAssetGet`.
        let bytes = files.bytes.get(source).unwrap_or_else(|| {
            panic!(
                "bynk internal error: deployed asset `{source}` has no bytes for the test harness"
            )
        });
        out.push_str(&format!(
            "  {}: {{ body: \"{}\", contentType: {} }},\n",
            bynk_project::json_string(served),
            base64(bytes),
            bynk_project::json_string(bynk_check::assets::content_type_for(served)),
        ));
    }
    out.push_str(
        "};\n\
         function __bynkAssetGet(url: string): HttpResult<never> {\n\
         \x20 const f = globalThis.Object.hasOwn(__bynkAssetFiles, url) ? __bynkAssetFiles[url] : undefined;\n\
         \x20 if (f === undefined) return HttpResult.NotFound;\n\
         \x20 const bin = atob(f.body);\n\
         \x20 const bytes = new Uint8Array(bin.length);\n\
         \x20 for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);\n\
         \x20 return HttpResult.Raw(bytes, f.contentType);\n\
         }\n",
    );
    Some(out)
}

/// #1883, slice 3: each asset a `system` case may name ([`SuiteView::assets`]),
/// grouped by the unit that declares it — a participant first (whose
/// `handlers.js` exports its own assets), then the commons that declare the
/// rest — sorted.
fn system_asset_owners(
    view: &test_suites::SuiteView,
    participants: &[String],
    unit_tables: &HashMap<String, UnitTable>,
) -> BTreeMap<String, Vec<String>> {
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, decl) in &view.assets {
        let owner = unit_tables
            .iter()
            .filter(|(_, t)| {
                t.assets
                    .get(name)
                    .is_some_and(|d| d.path == decl.path && d.at == decl.at)
            })
            .map(|(u, _)| u)
            .min_by_key(|u| (!participants.contains(u), (*u).clone()));
        if let Some(owner) = owner {
            owners.entry(owner.clone()).or_default().push(name.clone());
        }
    }
    for names in owners.values_mut() {
        names.sort();
    }
    owners
}

/// Standard base64 (RFC 4648, with padding) of `bytes`, for
/// [`asset_harness`]'s embedded files.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// #2138: a remote test build (`CompileOptions::remote_tests`).
#[cfg(test)]
mod remote_tests {
    use crate::project::{BuildTarget, RemoteTests, compile_files_in_memory_remote};

    const API: &str = "context shop.api\n\nconsumes bynk { Clock }\n\nservice api from http {\n  on GET(\"/n\") () -> Effect[HttpResult[Int]] by Visitor {\n    Ok(1)\n  }\n}\n\nservice tick from cron {\n  on schedule(\"*/5 * * * *\") () -> Effect[Result[(), String]] {\n    Ok(())\n  }\n}\n\nservice inbox from queue(\"q\") {\n  on message(n: Int) -> Effect[QueueResult] {\n    Ack\n  }\n}\n";

    fn suite(body: &str) -> String {
        format!("suite shop.api as system {{\n  case \"c\" {{\n{body}\n  }}\n}}\n")
    }

    fn build(
        body: &str,
        skip: bool,
    ) -> Result<crate::project::ProjectOutput, crate::project::ProjectFailure> {
        let s = suite(body);
        compile_files_in_memory_remote(
            &[("shop/api.bynk", API), ("tests/api.bynk", &s)],
            BuildTarget::Workers,
            Default::default(),
            true,
            Some(&RemoteTests {
                skip_unsupported: skip,
                ..RemoteTests::default()
            }),
            None,
        )
    }

    fn refusal(body: &str) -> String {
        match build(body, false) {
            Ok(_) => panic!("`{body}` should be refused remotely"),
            Err(f) => f
                .flatten()
                .iter()
                .map(|e| e.category.to_string())
                .collect::<Vec<_>>()
                .join(","),
        }
    }

    #[test]
    fn what_a_remote_run_cannot_honour_is_refused() {
        for (body, code) in [
            (
                "    advance Clock by 1.minutes\n    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)",
                "bynk.test.remote_unsupported_advance",
            ),
            (
                "    let r <- tick.schedule(\"*/5 * * * *\")\n    expect r is Ok(_)",
                "bynk.test.remote_unsupported_schedule",
            ),
            (
                "    let r <- inbox.message(1)\n    expect r is Ack",
                "bynk.test.remote_unsupported_queue",
            ),
        ] {
            assert_eq!(refusal(body), code, "{body}");
        }
    }

    #[test]
    fn a_case_env_that_disagrees_with_the_target_vars_is_refused() {
        let suite = "suite shop.api as system {\n  env { PAYMENTS = \"fake\" }\n  case \"c\" {\n    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)\n  }\n}\n";
        let build = |vars: &[(&str, &str)]| {
            compile_files_in_memory_remote(
                &[("shop/api.bynk", API), ("tests/api.bynk", suite)],
                BuildTarget::Workers,
                Default::default(),
                true,
                Some(&RemoteTests {
                    vars: vars
                        .iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                    ..RemoteTests::default()
                }),
                None,
            )
        };
        assert!(
            build(&[("PAYMENTS", "fake")]).is_ok(),
            "agreeing values run"
        );
        let codes = match build(&[("PAYMENTS", "real")]) {
            Ok(_) => panic!("a target deployed differently is refused"),
            Err(f) => f
                .flatten()
                .iter()
                .map(|e| e.category.to_string())
                .collect::<Vec<_>>(),
        };
        assert_eq!(codes, ["bynk.test.remote_unsupported_env"]);
    }

    #[test]
    fn a_dev_build_groups_cases_by_env_and_builds_one_group() {
        let suite = "suite shop.api as system {\n  env { P = \"a\" }\n  case \"one\" {\n    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)\n  }\n  case \"two\" {\n    env { P = \"b\" }\n    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)\n  }\n}\n";
        let build = |only: Option<&str>| {
            let out = compile_files_in_memory_remote(
                &[("shop/api.bynk", API), ("tests/api.bynk", suite)],
                BuildTarget::Workers,
                Default::default(),
                true,
                Some(&RemoteTests {
                    dev: true,
                    only_env: only.map(|v| [("P".to_string(), v.to_string())].into()),
                    ..RemoteTests::default()
                }),
                None,
            )
            .unwrap_or_else(|_| panic!("a dev build applies env rather than refusing it"));
            out.discovered
                .into_iter()
                .find(|s| s.name == "shop.api")
                .expect("the system suite")
        };
        let all = build(None);
        assert_eq!(all.envs.len(), 2, "two distinct env maps");
        assert_eq!(all.cases.len(), 2);
        let b = build(Some("b"));
        assert_eq!(
            b.cases.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
            ["two"],
            "only the group's cases are built"
        );
    }

    /// #2138: only a remote build reads `BYNK_TEST_RUN_ID`; an in-process
    /// module's `run.id` is `"local"` whatever the environment says.
    #[test]
    fn run_id_reads_the_environment_only_in_a_remote_build() {
        assert!(super::run_id_helper(true).contains("BYNK_TEST_RUN_ID"));
        let local = super::run_id_helper(false);
        assert!(!local.contains("BYNK_TEST_RUN_ID"));
        assert!(local.contains("\"local\""));
    }

    /// #2175: the clock warning follows what the run produced. Instants
    /// read from an address's result (a relation, a call on one) stay quiet;
    /// a constant instant warns wherever its `expect` sits — top level, in an
    /// `if` arm, or in the tail a `let … else` makes of the rest of the case.
    #[test]
    fn the_clock_warning_tracks_run_derived_instants_through_the_case() {
        const HOLDS: &str = "context shop.api\n\ntype Hold = { created: Instant, expires: Instant, ms: Int }\n\nservice api from http {\n  on GET(\"/h\") () -> Effect[HttpResult[Hold]] by Visitor {\n    Ok(Hold { created: Instant.fromEpochMillis(0), expires: Instant.fromEpochMillis(1), ms: 2 })\n  }\n}\n";
        let warned = |body: &str| {
            let s = suite(body);
            let out = compile_files_in_memory_remote(
                &[("shop/api.bynk", HOLDS), ("tests/api.bynk", &s)],
                BuildTarget::Workers,
                Default::default(),
                true,
                Some(&RemoteTests::default()),
                None,
            )
            .unwrap_or_else(|f| panic!("`{body}` builds remotely: {:?}", f.flatten()));
            out.warnings
                .iter()
                .any(|w| w.error.category == "bynk.test.remote_clock_dependent")
        };
        let read =
            "    let r <- api.GET(\"/h\") by Visitor\n    let Ok(h) = r else { expect false }\n";
        for quiet in [
            "    expect h.expires > h.created",
            "    expect Instant.fromEpochMillis(h.ms) > h.created",
            "    let later = h.created + 5.minutes\n    expect later > h.expires",
            // #2181: a binder rebinds a constant's name to a run value.
            "    let t = Instant.fromEpochMillis(0)\n    expect match Some(h.expires) { Some(t) => t > h.created, None => true }",
            "    let t = Instant.fromEpochMillis(0)\n    if Some(h.expires) is Some(t) {\n      expect t > h.created\n    } else {\n      expect false\n    }",
            "    let t = Instant.fromEpochMillis(0)\n    expect Some(h.expires) is Some(t) && t > h.created",
            "    let t = Instant.fromEpochMillis(0)\n    expect [h.expires].all((t) => t > h.created)",
            "    let t = Instant.fromEpochMillis(0)\n    match Some(h.expires) {\n      Some(t) => { expect t > h.created }\n      None => { expect false }\n    }",
        ] {
            assert!(!warned(&format!("{read}{quiet}")), "run-derived: {quiet}");
        }
        for loud in [
            // In the tail a `let … else` makes of the rest of the case.
            "    expect h.created > Instant.fromEpochMillis(0)",
            // A `let` bound to a constant.
            "    let epoch = Instant.fromEpochMillis(0)\n    expect h.created >= epoch",
            // In an `if` arm.
            "    if h.ms == 2 {\n      expect h.expires < Instant.fromEpochMillis(5)\n    } else {\n      expect false\n    }",
        ] {
            assert!(warned(&format!("{read}{loud}")), "constant: {loud}");
        }
    }

    #[test]
    fn an_absolute_instant_in_an_expect_warns_in_a_remote_build() {
        let warned = |body: &str| {
            let out = build(body, false)
                .unwrap_or_else(|_| panic!("a warning doesn't fail the build: {body}"));
            out.warnings
                .iter()
                .any(|w| w.error.category == "bynk.test.remote_clock_dependent")
        };
        assert!(warned(
            "    let n <- api.GET(\"/n\") by Visitor\n    let t = Instant.fromEpochMillis(0)\n    expect t < Instant.fromEpochMillis(1)"
        ));
        assert!(!warned(
            "    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)"
        ));
    }

    #[test]
    fn an_asset_fetch_is_refused() {
        const WEB: &str = "context t.web\n\nasset style = \"assets/board.css\" at \"/board.css\"\n\nservice api from http {\n  on GET(\"/\") () -> Effect[HttpResult[String]] by Visitor {\n    Ok(\"hi\")\n  }\n}\n";
        const SUITE: &str = "suite t.web as system {\n  case \"the stylesheet is served\" {\n    let css <- api.GET(style)\n    expect css is Raw(_, _)\n  }\n}\n";
        let folder = crate::project::assets::AssetFolder::new(
            std::collections::BTreeSet::from(["assets/board.css".to_string()]),
            std::sync::Arc::new(|_: &str| Ok(b"body {}".to_vec())),
        );
        let refused = compile_files_in_memory_remote(
            &[("t/web.bynk", WEB), ("tests/web.bynk", SUITE)],
            BuildTarget::Workers,
            Default::default(),
            true,
            Some(&RemoteTests::default()),
            Some(&folder),
        );
        let codes = match refused {
            Ok(_) => panic!("an asset fetch should be refused remotely"),
            Err(f) => f
                .flatten()
                .iter()
                .map(|e| e.category.to_string())
                .collect::<Vec<_>>(),
        };
        assert_eq!(codes, ["bynk.test.remote_unsupported_asset"]);
    }

    #[test]
    fn an_on_call_address_is_refused() {
        const PAY: &str = "context shop.pay\n\nservice charge {\n  on call(n: Int) -> Effect[Int] {\n    n\n  }\n}\n";
        const ORDERS: &str = "context shop.orders\n\nconsumes shop.pay\n\nservice place {\n  on call(n: Int) -> Effect[Int] {\n    let r <- shop.pay.charge(n)\n    r\n  }\n}\n";
        const SUITE: &str = "suite shop.orders as system {\n  case \"places across the wire\" {\n    let r <- shop.orders.place(1)\n    expect r == 1\n  }\n}\n";
        let refused = compile_files_in_memory_remote(
            &[
                ("shop/pay.bynk", PAY),
                ("shop/orders.bynk", ORDERS),
                ("tests/orders.bynk", SUITE),
            ],
            BuildTarget::Workers,
            Default::default(),
            true,
            Some(&RemoteTests::default()),
            None,
        );
        let codes = match refused {
            Ok(_) => panic!("an `on call` address should be refused remotely"),
            Err(f) => f
                .flatten()
                .iter()
                .map(|e| e.category.to_string())
                .collect::<Vec<_>>(),
        };
        assert_eq!(codes, ["bynk.test.remote_unsupported_call"]);
    }

    /// #2138 (track §3.6): a target with a safe and an unsafe route, for the
    /// `--settle` segmentation tests.
    const ORDERS: &str = "context shop.api\n\ntype Pair = { a: Int, b: Int }\n\nservice api from http {\n  on GET(\"/n\") () -> Effect[HttpResult[Int]] by Visitor {\n    Ok(1)\n  }\n\n  on POST(\"/n\") (body: Int) -> Effect[HttpResult[Int]] by Visitor {\n    Ok(body)\n  }\n\n  on GET(\"/p\") () -> Effect[HttpResult[Pair]] by Visitor {\n    Ok(Pair { a: 1, b: 2 })\n  }\n}\n";

    /// The system module a remote build emits for `body`.
    fn settle_ts(body: &str) -> String {
        let s = suite(body);
        let out = compile_files_in_memory_remote(
            &[("shop/api.bynk", ORDERS), ("tests/api.bynk", &s)],
            BuildTarget::Workers,
            Default::default(),
            true,
            Some(&RemoteTests::default()),
            None,
        )
        .unwrap_or_else(|f| panic!("`{body}` builds remotely: {:?}", f.flatten()));
        let (_, doc) = out
            .artefacts
            .docs
            .iter()
            .find(|(p, _)| {
                p.to_string_lossy()
                    .ends_with("integration_shop_api.test.ts")
            })
            .expect("the system module");
        doc.text().to_string()
    }

    /// The text of each `__bynkSettle` attempt in `ts`.
    /// (The case scaffold indents the body, so an attempt ends at the first
    /// line that is only `});`.)
    fn attempts(ts: &str) -> Vec<&str> {
        ts.match_indices("__bynkSettle(async () => {\n")
            .map(|(at, m)| {
                let body = &ts[at + m.len()..];
                let mut end = 0;
                for line in body.split_inclusive('\n') {
                    if line.trim() == "});" {
                        break;
                    }
                    end += line.len();
                }
                assert!(end < body.len(), "an attempt ends: {body}");
                &body[..end]
            })
            .collect()
    }

    /// #2138 review: classification fails closed. An address inside an
    /// effectful statement (an `if` or `match` around a `POST`) ends the
    /// segment, so it is never inside a replayed attempt.
    #[test]
    fn an_address_nested_in_an_effectful_statement_is_never_replayed() {
        for wrapped in [
            "if a is Ok(1) { api.POST(\"/n\", 2) } else { api.GET(\"/n\") }",
            "match a { Ok(_) => api.POST(\"/n\", 2), _ => api.GET(\"/n\") }",
        ] {
            let ts = settle_ts(&format!(
                "    let a <- api.GET(\"/n\") by Visitor\n    let b <- {wrapped} by Visitor\n    let c <- api.GET(\"/n\") by Visitor\n    expect c is Ok(1)"
            ));
            let tries = attempts(&ts);
            assert_eq!(tries.len(), 1, "{ts}");
            assert!(
                tries[0].contains("GET_n"),
                "the later read is retried: {ts}"
            );
            assert!(
                tries.iter().all(|t| !t.contains("POST_n")),
                "the write is not replayed: {ts}"
            );
        }
    }

    /// #2138 review: the attempt returns its bindings by name from the AST,
    /// as emitted — a name the harness already binds (`deps`) is renamed, and
    /// the attempt returns the renamed one.
    #[test]
    fn a_settle_attempt_returns_its_bindings_as_emitted() {
        let ts = settle_ts(
            "    let deps <- api.GET(\"/n\") by Visitor\n    let q = 1\n    expect deps is Ok(q)",
        );
        let tries = attempts(&ts);
        assert_eq!(tries.len(), 1, "{ts}");
        let ret = tries[0]
            .lines()
            .find_map(|l| l.trim().strip_prefix("return { "))
            .and_then(|r| r.strip_suffix(" };"))
            .unwrap_or_else(|| panic!("the attempt returns its bindings: {ts}"));
        let names: Vec<&str> = ret.split(", ").collect();
        assert_eq!(names.len(), 2, "{ts}");
        assert_ne!(names[0], "deps", "the harness's `deps` is renamed: {ts}");
        for n in names {
            assert!(
                tries[0].contains(&format!("const {n} = ")),
                "`{n}` is bound in the attempt: {ts}"
            );
        }
    }

    /// #2138 review: a `let … else` desugars into a hidden binding and a
    /// `match` tail; after a segment, the hidden binding and the pattern's
    /// own binding stay outside the attempt and read its returned names.
    #[test]
    fn a_let_else_and_a_pattern_binding_after_a_segment_see_its_bindings() {
        let ts = settle_ts(
            "    let a <- api.GET(\"/p\") by Visitor\n    let q = 1\n    expect a is Ok(_)\n    let Ok(p) = a else { expect false }\n    expect p.a == q",
        );
        let tries = attempts(&ts);
        assert_eq!(tries.len(), 1, "{ts}");
        assert!(tries[0].contains("return { a, q };"), "{ts}");
        assert!(ts.contains("const { a, q } = await __bynkSettle("), "{ts}");
        assert!(!tries[0].contains("__letElse"), "{ts}");

        let ts = settle_ts(
            "    let r <- api.POST(\"/n\", 2) by Visitor\n    let a <- api.GET(\"/n\") by Visitor\n    expect a is Ok(1)\n    let Ok(n) <- api.GET(\"/n\") by Visitor else { expect false }\n    expect n == 1",
        );
        let tries = attempts(&ts);
        assert_eq!(tries.len(), 1, "{ts}");
        assert!(tries[0].contains("return { a };"), "{ts}");
        assert!(
            !tries[0].contains("POST_n") && !tries[0].contains("__letElse"),
            "{ts}"
        );
    }

    #[test]
    fn an_http_case_is_driven_at_the_target_url_with_the_environment_s_secrets() {
        let out = build(
            "    let n <- api.GET(\"/n\") by Visitor\n    expect n is Ok(1)",
            false,
        )
        .unwrap_or_else(|_| panic!("an http-only case builds remotely"));
        let (_, doc) = out
            .artefacts
            .docs
            .iter()
            .find(|(p, _)| {
                p.to_string_lossy()
                    .ends_with("integration_shop_api.test.ts")
            })
            .expect("the system module");
        let ts = doc.text();
        assert!(
            ts.contains("rootEnv.SHOP_API = __bynkRemoteBinding(\"shop-api\");"),
            "{ts}"
        );
        assert!(ts.contains("BYNK_TEST_TARGETS"), "{ts}");
        assert!(
            !ts.contains("__makeIntegrationDoNamespace("),
            "no in-process Worker: {ts}"
        );
        assert!(
            !ts.contains("\"__bynk_test_secret\""),
            "no fixed test secret: {ts}"
        );
    }

    #[test]
    fn skip_unsupported_drops_the_case_and_says_why() {
        let out = build("    let r <- inbox.message(1)\n    expect r is Ack", true)
            .unwrap_or_else(|_| panic!("a skipped case does not fail the build"));
        let suite = out
            .discovered
            .iter()
            .find(|s| s.name == "shop.api")
            .expect("the system suite");
        assert!(suite.cases.is_empty(), "the case is not run");
        assert_eq!(suite.skipped.len(), 1);
        assert_eq!(suite.skipped[0].code, "bynk.test.remote_unsupported_queue");
        assert!(suite.skipped[0].reason.contains("delivers a queue message"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_syntax::ast::{BaseType, Ident, TypeRef};
    use std::collections::HashSet;

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

    #[test]
    fn appends_bindings_that_are_absent() {
        let mut names = vec!["Ok".to_string(), "Err".to_string()];
        append_missing_bindings(&mut names, emitter::BYTES_RUNTIME_IMPORTS);
        assert!(names.iter().any(|n| n == "__bynkBytesEqual"), "{names:?}");
        assert!(
            names.iter().any(|n| n == "__bynkBytesDecodeUtf8"),
            "{names:?}"
        );
    }

    #[test]
    fn skips_bindings_already_present() {
        let mut names = vec![
            "Ok".to_string(),
            "Err".to_string(),
            "type Result".to_string(),
        ];
        append_missing_bindings(&mut names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
        assert_eq!(
            names.iter().filter(|n| n.as_str() == "Ok").count(),
            1,
            "`Ok` was already present and must not repeat: {names:?}"
        );
        assert_eq!(
            names,
            vec!["Ok", "Err", "type Result", "type __BoundaryError"],
        );
    }

    /// The bare name is what collides, so `type BoundaryError` must match an
    /// existing `BoundaryError`.
    #[test]
    fn matches_a_type_prefixed_group_binding_against_a_bare_one() {
        let mut names = vec![
            "Ok".to_string(),
            "Err".to_string(),
            "type Result".to_string(),
            "__BoundaryError".to_string(),
        ];
        let before = names.clone();
        append_missing_bindings(&mut names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
        assert_eq!(
            names, before,
            "every binding was already present, so `names` is untouched"
        );
    }

    /// …and the other direction: a bare group binding against an existing
    /// `type`-prefixed one. This is the case that would regress if `bare`
    /// were applied to only one side of the comparison.
    #[test]
    fn matches_a_bare_group_binding_against_a_type_prefixed_one() {
        let mut names = vec!["type Ok".to_string(), "type Err".to_string()];
        let before = names.clone();
        append_missing_bindings(&mut names, ", Ok, Err");
        assert_eq!(
            names, before,
            "a bare binding must match an existing `type`-prefixed one: {names:?}"
        );
    }

    /// The two calls run back to back over the same `names`, so the second
    /// sees the first's own output as `present` — the overlap between the
    /// groups (`Ok`, `Err`, `type Result`) must not double up.
    #[test]
    fn composes_across_two_sequential_calls() {
        let mut names = vec![
            "Ok".to_string(),
            "Err".to_string(),
            "type Result".to_string(),
        ];
        append_missing_bindings(&mut names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
        append_missing_bindings(&mut names, emitter::JSON_CODEC_RUNTIME_IMPORTS);
        assert_eq!(
            names,
            vec![
                "Ok",
                "Err",
                "type Result",
                "type __BoundaryError",
                "type __JsonValue",
                "type JsonError",
            ],
            "the shared bindings must be added once: {names:?}"
        );
    }

    /// Review of #1410, finding 1: `run_dispatch_stmt` must escape a raw
    /// case/property name exactly once. Pins a name containing both a
    /// double quote and a backslash — the exact shape a double-escape
    /// (passing an already-`emitter::escape_ts_string`-escaped name into
    /// `str_lit`, which escapes again) would corrupt, and which no `.bynk`
    /// fixture exercises today.
    #[test]
    fn the_run_function_escapes_a_quoted_backslashed_name_exactly_once() {
        let stmts = build_run_function(&[(r#"say "hi" \ ok"#.to_string(), "test_a".to_string())]);
        let printed: String = stmts.iter().map(|s| bynk_ts::print_stmt(s, 0)).collect();
        // The name is escaped once in the case list and once in the dispatch.
        assert!(
            printed
                .contains(r#"export const __bynkCases: readonly string[] = ["say \"hi\" \\ ok"];"#),
            "{printed}"
        );
        assert!(
            printed.contains(r#"if (want("say \"hi\" \\ ok")) {"#),
            "{printed}"
        );
        assert!(printed.contains(r#"const r = { name: "say \"hi\" \\ ok", ...(await test_a()), durationMs: __bynkCaseSince(started) };"#), "{printed}");
    }

    /// #2267 stage 2: each case is timed, handed to `onCase` as it finishes and
    /// pushed to the results, in that order, and the module lists its case names.
    #[test]
    fn the_run_function_times_each_case_and_reports_it_as_it_finishes() {
        let stmts = build_run_function(&[
            ("one".to_string(), "test_one".to_string()),
            ("two".to_string(), "test_two".to_string()),
        ]);
        let printed: String = stmts.iter().map(|s| bynk_ts::print_stmt(s, 0)).collect();
        assert!(
            printed.contains(r#"export const __bynkCases: readonly string[] = ["one", "two"];"#)
        );
        assert!(printed.contains(
            "export async function run(only?: string, onCase?: (a0: CaseResult) => void)"
        ));
        let started = printed
            .find("const started = __bynkCaseNow();")
            .expect("timed");
        let ran = printed.find("...(await test_one())").expect("ran");
        let pushed = printed.find("results.push(r);").expect("pushed");
        let reported = printed.find("if (onCase) onCase(r);").expect("reported");
        assert!(
            started < ran && ran < pushed && pushed < reported,
            "{printed}"
        );
        assert_eq!(printed.matches("const started = ").count(), 2);
    }

    // v0.127 (editor-currency slice 6), #2273: the top-level runner reads
    // `BYNK_TEST_CASE` and threads it into every suite's `run(only)`, so a
    // `bynkc test --case <name>` filters which cases execute.
    #[test]
    fn emit_test_main_threads_the_case_and_suite_filter() {
        let tests = vec![RunnableTest {
            target_name: "commerce.money".to_string(),
            module_path: std::path::PathBuf::from("tests/commerce_money.test.ts"),
            kind: "unit",
            suite_name: "commerce.money".to_string(),
            cases: vec![DiscoveredCase {
                name: "rounds".to_string(),
                location: None,
                tier: None,
                generative: None,
                doc: false,
            }],
            skipped: Vec::new(),
            envs: Vec::new(),
            remote: false,
            generative: false,
        }];
        let program = emit_test_main(&tests, ImportExt::Js);
        let out = bynk_ts::print(&program, "", "", "").text;
        assert!(
            out.contains("process.env.BYNK_TEST_FILTER"),
            "the runner must read the filter from the environment, got:\n{out}"
        );
        // Both the NDJSON and the human loop walk the selected modules and run
        // each through the filter, so a module stays one suite record.
        assert_eq!(
            out.matches("await runSelected(m.run, report)").count(),
            2,
            "both dispatch branches must run each suite through the filter, got:\n{out}"
        );
        assert_eq!(
            out.matches("for (const m of active)").count(),
            2,
            "got:\n{out}"
        );
        assert!(
            out.contains("unmatched: unmatched()"),
            "run-end reports the values that selected nothing, got:\n{out}"
        );
    }

    /// #1325's own direct fixture test — `emit_test_main`'s signature
    /// (`&[RunnableTest]`/`ImportExt`) is simple enough to hand-construct
    /// directly, unlike `emit_worker_compose`'s/`emit_worker_entry`'s own
    /// whole-project `UnitTable`-derived data (#1321/#1323's own reason for
    /// relying on the full fixture corpus instead). Calls the real,
    /// converted function with real-shaped data and asserts the printed
    /// output is byte-identical to a real fixture's own `tests/main.ts`, not
    /// a hand-rebuilt approximation of it.
    #[test]
    fn matches_the_real_fixtures_own_tests_main_ts_byte_for_byte() {
        let expected = include_str!(
            "../../../bynkc/tests/fixtures/positive/109_test_with_opaque_construction/\
             expected/tests/main.ts"
        );
        let tests = vec![RunnableTest {
            target_name: "commerce.payment".to_string(),
            module_path: std::path::PathBuf::from("commerce_payment.test.js"),
            kind: "unit",
            suite_name: "commerce.payment".to_string(),
            cases: vec![DiscoveredCase {
                name: "opaque".to_string(),
                location: None,
                tier: None,
                generative: None,
                doc: false,
            }],
            skipped: Vec::new(),
            envs: Vec::new(),
            remote: false,
            generative: false,
        }];
        let program = emit_test_main(&tests, ImportExt::Js);
        let printed = bynk_ts::print(&program, "", "", "");
        assert_eq!(printed.text, expected);
    }

    /// #1329's own direct fixture test — `emit_commons_barrel`'s inputs
    /// (`groups`/`parsed`/`import_ext`/`emitted`) are hand-constructible via
    /// `ParsedFile::new`'s own documented "general constructor — `bynk-emit`'s
    /// own tests use this" purpose, unlike `emit_worker_compose`'s/
    /// `emit_worker_entry`'s/`emit_composition_root`'s own whole-project
    /// `UnitTable`-derived data (#1321/#1323/#1327's own reason for relying
    /// on the full fixture corpus instead). Calls the real, converted
    /// function with real-shaped data and asserts the printed output is
    /// byte-identical to `251_multi_file_commons_test`'s own real
    /// `expected/thing.ts`, not a hand-rebuilt approximation of it.
    #[test]
    fn matches_the_real_fixtures_own_commons_barrel_ts_byte_for_byte() {
        use bynk_syntax::ast::SourceUnit;

        let expected = include_str!(
            "../../../bynkc/tests/fixtures/positive/251_multi_file_commons_test/\
             expected/thing.ts"
        );
        let commons = |name: &str| {
            SourceUnit::Commons(Commons {
                name: QualifiedName {
                    parts: vec![Ident {
                        name: name.to_string(),
                        span: Span::new(0, 0),
                    }],
                    span: Span::new(0, 0),
                },
                items: vec![],
                uses: vec![],
                documentation: None,
                form: CommonsForm::Fragment,
                span: Span::new(0, 0),
                trivia: Trivia {
                    leading: vec![],
                    trailing: None,
                },
                trailing_comments: vec![],
            })
        };
        let parsed = vec![
            ParsedFile::new(
                PathBuf::from("thing/make.bynk"),
                PathBuf::from("thing/make.bynk"),
                None,
                String::new(),
                commons("thing"),
                UnitKind::Commons,
                false,
            ),
            ParsedFile::new(
                PathBuf::from("thing/widget.bynk"),
                PathBuf::from("thing/widget.bynk"),
                None,
                String::new(),
                commons("thing"),
                UnitKind::Commons,
                false,
            ),
        ];
        let mut groups = BTreeMap::new();
        groups.insert("thing".to_string(), vec![0, 1]);
        let mut emitted = HashSet::new();
        let staged =
            emit_commons_barrel("thing", &groups, &parsed, ImportExt::Js, &mut emitted, &[])
                .expect("a multi-file commons must produce a barrel");
        let Document::Ts(program) = staged.document else {
            panic!("emit_commons_barrel must build a TsProgram");
        };
        let printed = bynk_ts::print(&program, "", "", "");
        assert_eq!(printed.text, expected);
    }

    #[test]
    fn names_ident_ignores_literals_and_members_but_reads_template_holes() {
        // #2073 review: a name in a string, a template's text or after a `.`
        // is not a reference; one in a template's `${…}` hole is.
        assert!(names_ident("const r = Region.Here;", "Region"));
        assert!(!names_ident("x.Region", "Region"));
        assert!(!names_ident("__serialise_Region(v)", "Region"));
        assert!(!names_ident("fail(\"expect r == Region.Here\")", "Region"));
        assert!(!names_ident("fail('Region')", "Region"));
        assert!(!names_ident("fail(`expected Region`)", "Region"));
        assert!(names_ident("fail(`got ${show(Region.Here)}`)", "Region"));
        assert!(names_ident(
            "fail(`a ${ { k: 1 }.k } b`) + Region.Here",
            "Region"
        ));
        assert!(!names_ident("fail(`a ${ { k: 1 }.k } Region`)", "Region"));
        assert!(!names_ident("fail(\"a \\\" Region\")", "Region"));
        assert!(!names_ident("x(); // Region\n", "Region"));
        assert!(names_ident("x(); // c\nRegion.Here", "Region"));
        assert!(!names_ident("/* Region */ x()", "Region"));
        assert!(names_ident("/* a */ Region.Here", "Region"));
    }

    #[test]
    fn sanitise_case_name_prefixes_and_advances_index() {
        let mut idx = 0;
        assert_eq!(
            sanitise_case_name("hello world", &mut idx),
            "test_hello_world"
        );
        assert_eq!(idx, 1); // index advances on every call
        assert_eq!(sanitise_case_name("a-b.c", &mut idx), "test_a_b_c");
        assert_eq!(idx, 2);
    }

    #[test]
    fn sanitise_case_name_appends_index_only_for_empty_name() {
        let mut idx = 5;
        assert_eq!(sanitise_case_name("", &mut idx), "test_5"); // empty -> index suffix
        assert_eq!(idx, 6);
        // A name of only non-alphanumeric chars is NOT "test_", so no suffix.
        let mut idx2 = 9;
        assert_eq!(sanitise_case_name(" ", &mut idx2), "test__");
        assert_eq!(idx2, 10);
    }

    // -- coerce_int_field (#1398, review of #1428) ---------------------------

    fn refined_int_decl(name: &str) -> TypeDecl {
        TypeDecl {
            name: Ident {
                name: name.to_string(),
                span: Span::default(),
            },
            type_params: Vec::new(),
            body: TypeBody::Refined {
                base: BaseType::Int,
                base_span: Span::default(),
                refinement: None,
            },
            documentation: None,
            span: Span::default(),
            trivia: Trivia::default(),
        }
    }

    fn opaque_int_decl(name: &str) -> TypeDecl {
        TypeDecl {
            name: Ident {
                name: name.to_string(),
                span: Span::default(),
            },
            type_params: Vec::new(),
            body: TypeBody::Opaque {
                base: BaseType::Int,
                base_span: Span::default(),
                refinement: None,
            },
            documentation: None,
            span: Span::default(),
            trivia: Trivia::default(),
        }
    }

    #[test]
    fn coerce_int_field_wraps_a_bare_int_draw_in_number() {
        let tys = Arc::new(Types::new());
        let int_ty = tys.intern(checker::Ty::Base(BaseType::Int));
        let types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        let draw = ident("rng.int(-1000n, 1000n)");

        let coerced = coerce_int_field(int_ty, &types, &tys, draw);
        assert_eq!(
            bynk_ts::print_expr(&coerced),
            "globalThis.Number(rng.int(-1000n, 1000n))"
        );
    }

    #[test]
    fn coerce_int_field_wraps_a_refined_int_field_too() {
        // Review of #1428: `refined_gen_ts`'s own `(value as any)` cast is
        // compile-time only — the runtime value underneath is still a real
        // bigint, so a refined-Int payload/field needs the same coercion a
        // bare Int does, or the exact `TypeError: Cannot mix BigInt and
        // other types` #1398 names still reproduces one type-alias away.
        let tys = Arc::new(Types::new());
        let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        types.insert("Pct".to_string(), Arc::new(refined_int_decl("Pct")));
        let pct_ty = tys.intern(checker::Ty::Named {
            name: "Pct".to_string(),
            kind: checker::NamedKind::Refined(BaseType::Int),
            args: Vec::new(),
        });
        let draw = ident("(rng.int(0n, 100n) as any)");

        let coerced = coerce_int_field(pct_ty, &types, &tys, draw);
        assert_eq!(
            bynk_ts::print_expr(&coerced),
            "globalThis.Number((rng.int(0n, 100n) as any))"
        );
    }

    #[test]
    fn coerce_int_field_wraps_an_opaque_int_field_too() {
        let tys = Arc::new(Types::new());
        let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        types.insert("Pct".to_string(), Arc::new(opaque_int_decl("Pct")));
        let pct_ty = tys.intern(checker::Ty::Named {
            name: "Pct".to_string(),
            kind: checker::NamedKind::Opaque(BaseType::Int),
            args: Vec::new(),
        });
        let draw = ident("Pct.unsafe(rng.int(0n, 100n))");

        let coerced = coerce_int_field(pct_ty, &types, &tys, draw);
        assert_eq!(
            bynk_ts::print_expr(&coerced),
            "globalThis.Number(Pct.unsafe(rng.int(0n, 100n)))"
        );
    }

    #[test]
    fn coerce_int_field_passes_the_undefined_sentinel_through_unwrapped() {
        // Review of #1428: the depth-exhaustion `undefined` sentinel
        // (`canon_ts_for_ty`'s own `depth == 0` early return) must not
        // become `Number(undefined)` — a silent `NaN` where the unwrapped
        // sentinel at least fails loudly (a separate, pre-existing gap
        // tracked as #1429, not fixed here).
        let tys = Arc::new(Types::new());
        let int_ty = tys.intern(checker::Ty::Base(BaseType::Int));
        let types: HashMap<String, Arc<TypeDecl>> = HashMap::new();

        let coerced = coerce_int_field(int_ty, &types, &tys, ident("undefined"));
        assert_eq!(bynk_ts::print_expr(&coerced), "undefined");
    }

    #[test]
    fn coerce_int_field_leaves_a_non_int_value_untouched() {
        let tys = Arc::new(Types::new());
        let string_ty = tys.intern(checker::Ty::Base(BaseType::String));
        let types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        let draw = ident("rng.str(0, 8)");

        let coerced = coerce_int_field(string_ty, &types, &tys, draw);
        assert_eq!(bynk_ts::print_expr(&coerced), "rng.str(0, 8)");
    }

    // -- gen_ts_for_ty's depth-exhaustion fallback (#1429, review of #1434) --

    fn sum_decl(name: &str, variants: Vec<(&str, Vec<(&str, TypeRef)>)>) -> TypeDecl {
        TypeDecl {
            name: Ident {
                name: name.to_string(),
                span: Span::default(),
            },
            type_params: Vec::new(),
            body: TypeBody::Sum(bynk_syntax::ast::SumBody {
                variants: variants
                    .into_iter()
                    .map(|(vname, fields)| bynk_syntax::ast::Variant {
                        documentation: None,
                        name: Ident {
                            name: vname.to_string(),
                            span: Span::default(),
                        },
                        payload: fields
                            .into_iter()
                            .map(|(fname, type_ref)| bynk_syntax::ast::VariantField {
                                documentation: None,
                                name: Ident {
                                    name: fname.to_string(),
                                    span: Span::default(),
                                },
                                type_ref,
                                span: Span::default(),
                            })
                            .collect(),
                        span: Span::default(),
                        trivia: Default::default(),
                    })
                    .collect(),
                embeds: Vec::new(),
                span: Span::default(),
                trailing_comments: Default::default(),
            }),
            documentation: None,
            span: Span::default(),
            trivia: Trivia::default(),
        }
    }

    #[test]
    fn gen_ts_for_ty_bottoms_out_a_first_variant_recursive_sum_the_checker_validated() {
        // #1429's own repro: `T = Base(n: Int) | Cons(tail: T)`. The checker
        // accepts `T` as a `for all` binding (`prop_binding_generable` only
        // needs the first variant, `Base`, to be generable) — the fallback
        // must be able to actually build a `Base(n: Int)` once generation
        // depth is exhausted deep inside a `Cons` chain, not `undefined`.
        let tys = Arc::new(Types::new());
        let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        types.insert(
            "T".to_string(),
            Arc::new(sum_decl(
                "T",
                vec![
                    ("Base", vec![("n", base(BaseType::Int))]),
                    ("Cons", vec![("tail", named("T"))]),
                ],
            )),
        );
        let t_ty = tys.intern(checker::Ty::Named {
            name: "T".to_string(),
            kind: checker::NamedKind::Sum,
            args: Vec::new(),
        });

        // depth: 0 forces the fallback immediately, the same state
        // `gen_ts_for_ty`'s own recursion reaches once its budget runs out.
        let generated = gen_ts_for_ty(t_ty, &types, 0, &tys);
        let printed = bynk_ts::print_expr(&generated);
        assert!(
            !printed.contains("undefined"),
            "a checker-validated recursive sum must not fall back to `undefined`, got: {printed}"
        );
        assert!(
            printed.contains("T.Base("),
            "expected the fallback to build T's own first (validated) variant, got: {printed}"
        );
    }

    #[test]
    fn gen_ts_for_ty_stays_bounded_for_a_branching_variant_the_checker_never_validated() {
        // Review of #1434, finding 1: `prop_binding_generable` only ever
        // descends a sum's *first* variant, but `gen_ts_for_ty`'s own Sum
        // branch builds a thunk for *every* variant — so a non-first
        // variant's field type can reach the depth-0 fallback fully
        // unvalidated. `Bad`'s own first variant (`Node`) is branching and
        // never bottoms out on its own; handing the fallback the full
        // `PROP_GEN_DEPTH` budget for `Bad` (not just `T`, the type the
        // checker actually validated) would walk a full binary tree before
        // ever reaching `Leaf`'s escape — confirmed to reproduce a real
        // ~176MB, 50+ second emit for this exact shape before the guard
        // below. The fallback must recognise `Bad` itself was never
        // validated and fall back to the original, small budget for it.
        let tys = Arc::new(Types::new());
        let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        types.insert(
            "Bad".to_string(),
            Arc::new(sum_decl(
                "Bad",
                vec![
                    ("Node", vec![("l", named("Bad")), ("r", named("Bad"))]),
                    ("Leaf", vec![("n", base(BaseType::Int))]),
                ],
            )),
        );
        types.insert(
            "T".to_string(),
            Arc::new(sum_decl(
                "T",
                vec![
                    ("Plain", vec![("n", base(BaseType::Int))]),
                    ("Nope", vec![("b", named("Bad"))]),
                ],
            )),
        );
        let t_ty = tys.intern(checker::Ty::Named {
            name: "T".to_string(),
            kind: checker::NamedKind::Sum,
            args: Vec::new(),
        });

        // depth: 1 reaches `Nope`'s own `b: Bad` field at depth 0 — `T`
        // itself is still checker-validated (via `Plain`), so this isn't
        // testing the depth-0-at-the-top-level case above, only what
        // happens once the *nested*, unvalidated `Bad` field is reached.
        let generated = gen_ts_for_ty(t_ty, &types, 1, &tys);
        let printed = bynk_ts::print_expr(&generated);
        // A full `PROP_GEN_DEPTH`-deep expansion of `Bad`'s own branching
        // first variant is tens of thousands of characters at minimum
        // (confirmed empirically: ~200KB unguarded vs. ~200 bytes guarded,
        // for the equivalent whole-fixture emit); a generous bound well
        // below that catches a regression without pinning an exact byte
        // count against unrelated formatting changes.
        assert!(
            printed.len() < 2_000,
            "expected a bounded fallback for Bad's own unvalidated branching \
             variant, got {} chars (a regression here likely means the depth-0 \
             fallback stopped checking prop_binding_generable before spending \
             the full PROP_GEN_DEPTH budget): {printed}",
            printed.len()
        );
    }

    // -- the unified emitter type-ref renderers -------------------------------
    fn named(n: &str) -> TypeRef {
        TypeRef::Named(Ident {
            name: n.to_string(),
            span: Span::default(),
        })
    }
    fn base(b: BaseType) -> TypeRef {
        TypeRef::Base(b, Span::default())
    }

    #[test]
    fn ts_type_ref_bases_and_generics() {
        assert_eq!(emitter::ts_type_ref(&base(BaseType::Int)), "number");
        assert_eq!(emitter::ts_type_ref(&base(BaseType::Float)), "number");
        assert_eq!(emitter::ts_type_ref(&base(BaseType::String)), "string");
        assert_eq!(emitter::ts_type_ref(&base(BaseType::Bool)), "boolean");
        assert_eq!(emitter::ts_type_ref(&named("Order")), "Order");
        assert_eq!(
            emitter::ts_type_ref(&TypeRef::List(Box::new(named("Order")), Span::default())),
            "readonly Order[]"
        );
        assert_eq!(
            emitter::ts_type_ref(&TypeRef::Option(
                Box::new(base(BaseType::Int)),
                Span::default()
            )),
            "Option<number>"
        );
        assert_eq!(
            emitter::ts_type_ref(&TypeRef::Effect(
                Box::new(TypeRef::Unit(Span::default())),
                Span::default()
            )),
            "globalThis.Promise<void>"
        );
        assert_eq!(
            emitter::ts_type_ref(&TypeRef::Map(
                Box::new(base(BaseType::String)),
                Box::new(named("V")),
                Span::default()
            )),
            "ReadonlyMap<string, V>"
        );
        assert_eq!(
            emitter::ts_type_ref(&TypeRef::Result(
                Box::new(named("T")),
                Box::new(named("E")),
                Span::default()
            )),
            "Result<T, E>"
        );
        assert_eq!(
            emitter::ts_type_ref(&TypeRef::HttpResult(Box::new(named("T")), Span::default())),
            "HttpResult<T>"
        );
        assert_eq!(
            emitter::ts_type_ref(&TypeRef::ValidationError(Span::default())),
            "ValidationError"
        );
        assert_eq!(
            emitter::ts_type_ref(&TypeRef::JsonError(Span::default())),
            "JsonError"
        );
    }

    #[test]
    fn ts_type_ref_fn_uses_positional_param_names() {
        let f = TypeRef::Fn(
            vec![base(BaseType::Int), named("Order")],
            Box::new(TypeRef::Unit(Span::default())),
            Span::default(),
        );
        assert_eq!(emitter::ts_type_ref(&f), "(a0: number, a1: Order) => void");
    }

    #[test]
    fn ts_type_ref_qualified_prefixes_only_scoped_names() {
        let mut scope: HashSet<String> = HashSet::new();
        scope.insert("Order".to_string());
        // A named type in the privileged scope is qualified with the namespace.
        assert_eq!(
            bynk_ts::print_type(&emitter::ts_type_ref_qualified_ts_type(
                &named("Order"),
                &scope,
                "Ns"
            )),
            "Ns.Order"
        );
        // A named type outside the scope is left bare.
        assert_eq!(
            bynk_ts::print_type(&emitter::ts_type_ref_qualified_ts_type(
                &named("Other"),
                &scope,
                "Ns"
            )),
            "Other"
        );
        // Qualification recurses through generic arguments.
        assert_eq!(
            bynk_ts::print_type(&emitter::ts_type_ref_qualified_ts_type(
                &TypeRef::List(Box::new(named("Order")), Span::default()),
                &scope,
                "Ns"
            )),
            "readonly Ns.Order[]"
        );
        // Base types are unaffected by qualification.
        assert_eq!(
            bynk_ts::print_type(&emitter::ts_type_ref_qualified_ts_type(
                &base(BaseType::Int),
                &scope,
                "Ns"
            )),
            "number"
        );
    }
}
