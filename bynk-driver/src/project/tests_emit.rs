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
use bynk_emit::emitter::wrangler::WorkerBinding;
// #1759: every namespace a test module imports, from one definition.
use bynk_check::checker::Types;
use bynk_check::d1_migrations::D1Migrations;
use bynk_check::test_suites::{self};
use bynk_check::wire_format;
use bynk_syntax::ast::{Case, Visibility};
use std::sync::Arc;

// -- v0.7 / v0.118: test declaration processing --

mod harness;
mod integration_module;
mod property_generators;
mod property_tests;
mod stubs;
mod system_support;
mod test_cases;
mod test_deps;
mod test_main;
mod test_module;
pub(crate) use self::harness::RunnableTest;
pub(crate) use self::harness::discovery_manifest;
use self::harness::*;
use self::integration_module::*;
use self::property_generators::*;
use self::property_tests::*;
use self::stubs::*;
use self::system_support::*;
use self::test_cases::*;
pub(crate) use self::test_deps::TestAssetFiles;
use self::test_deps::*;
pub(crate) use self::test_main::emit_test_main;
use self::test_main::*;
// These shadow `project`'s own private tree-construction helpers of the same names,
// which the glob imports above would otherwise make ambiguous.
use self::test_main::{call, ident, member, method_call, str_lit};
pub(in crate::project) use self::test_module::duplicated_export_reexports;
pub(in crate::project) use self::test_module::emit_commons_barrel;
pub(in crate::project) use self::test_module::merged_unit_helpers;
use self::test_module::*;

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
    // #3162: the declarations recovery skipped, per unit.
    broken: &bynk_check::project_model::BrokenDeclNames,
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
    // the guard is (they are always paired — `bynk test` sets both).
    contracts: bool,
    // v0.132: running set of already-emitted barrel output paths, shared with the
    // integration pass so a commons imported by both is barrelled exactly once.
    emitted_barrels: &mut HashSet<PathBuf>,
    // #1883, slice 3: the declared assets, for `svc.GET(asset)` in a case.
    asset_files: &TestAssetFiles,
    // #2842: the stored snapshot files, by project-relative path.
    snapshot_files: Option<&BTreeMap<String, String>>,
    // P4b (#2608): each `D1` database's migrations, run on every test database.
    d1_migrations: &D1Migrations,
    // #3108: the schemas those migrations build, which a case's literal D1
    // statements are prepared against (`None` under `[check] d1_sql = false`).
    d1_sql: Option<&bynk_check::d1_sql::D1Sql>,
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
        broken,
        errors,
        refs,
        &mut bynk_check::locals::LocalsSink::new(),
        tys,
        d1_sql,
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
            fixture_consts(indices, parsed, asset_files),
            binding_modules,
            schema_versions,
            target_uses_emit(
                target_name,
                target_kind,
                unit_tables,
                unit_flattened,
                unit_callees,
            ),
            d1_migrations,
            snapshot_files,
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
    // #3162: the declarations recovery skipped, per unit.
    broken: &bynk_check::project_model::BrokenDeclNames,
    // v0.132: production unit name -> its `parsed` file indices (see `process_tests`).
    groups: &BTreeMap<String, Vec<usize>>,
    // v0.132: barrel-path dedup set shared with the unit-test pass.
    emitted_barrels: &mut HashSet<PathBuf>,
    // #1883, slice 3: the declared assets, for `svc.GET(asset)` in a case.
    asset_files: &TestAssetFiles,
    // P4b (#2608): each `D1` database's migrations, run on every test database.
    d1_migrations: &D1Migrations,
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
        broken,
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
                    suite_fns: test_suites::SuiteItems::of(d),
                    tier: super::case_effective_tier(case, d),
                    env: bynk_check::test_env::case_env(d, case, asset_files.fixtures),
                    env_values: bynk_check::test_env::case_env_values(d, case),
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
            fixture_consts(indices, parsed, asset_files),
            remote,
            d1_migrations,
            errors,
            tys,
            super::project_mentions_encode_plain(parsed),
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
    /// #2834: the declaring suite's helper `fn`s.
    suite_fns: test_suites::SuiteItems<'a>,
    /// The case's effective tier, which its suite default may supply.
    tier: bynk_syntax::ast::TestTier,
    /// #2134: the variables and secrets the case runs with (its suite's
    /// `env`, overlaid by its own), each fixture as its text.
    env: BTreeMap<String, String>,
    /// #2956: the same entries as written, for the case runner, which names
    /// a fixture's module-level constant rather than repeat its text.
    env_values: BTreeMap<String, bynk_check::test_env::EnvValue>,
    source: &'a str,
    rel_path: String,
    map_source: String,
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

/// Type-reference builders shared by the split-out test modules.
#[cfg(test)]
mod test_support {
    use bynk_syntax::ast::{BaseType, Ident, TypeRef};
    use bynk_syntax::span::Span;

    pub(super) fn named(n: &str) -> TypeRef {
        TypeRef::Named(Ident {
            name: n.to_string(),
            span: Span::default(),
        })
    }
    pub(super) fn base(b: BaseType) -> TypeRef {
        TypeRef::Base(b, Span::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_syntax::ast::{BaseType, TypeRef};
    use bynk_syntax::span::Span;

    // -- the unified emitter type-ref renderers -------------------------------
    use super::test_support::{base, named};

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
            emitter::ts_type_ref(&TypeRef::DecodeError(Span::default())),
            "DecodeError"
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
