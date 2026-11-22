//! Test-harness generation: `suite`/`test integration` processing, the test entry
//! (`tests/main.ts`), the discovery manifest and the embedded test-runtime fragments. The
//! fragments are authored as modules under `bynk-emit/runtime/src/testing/` and reach the harness
//! only through `runtime_splice` (1B.3a, #4047).
//!
//! 1B.2a (#4028, ADR "Where test generation lives"): moved here from the driver's
//! `project/tests_emit{.rs,/}`, beside `emitter`. It reads no file, spawns no process and holds
//! no thread-local; the driver keeps orchestration, file I/O and running the tests, and calls
//! [`process_tests`], [`process_integration_tests`], [`discovery_manifest`] and
//! [`emit_test_main`]. Production output never lives here: the commons barrel is
//! `emitter::commons_barrel`.
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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::emit_model::*;
use crate::emitter;
use crate::emitter::RuntimeUse;
use crate::emitter::bindings::WorkerBinding;
use crate::emitter::harness_names::{
    SysdriveKind, route_with, sysdrive_name, wrong_method_driver_name,
};
use bynk_check::checker;
use bynk_check::checker::ExprId;
use bynk_check::hints::HintSink;
use bynk_check::index::RefSink;
use bynk_check::locals::LocalsSink;
use bynk_check::project_model::handler_cross_caps;
use bynk_check::requirements::RequirementSink;
use bynk_check::resolver::{self, ResolvedCommons};
use bynk_check::symbols::*;
use bynk_project::UnitKind;
use bynk_project::discovery::*;
use bynk_project::paths::*;
use bynk_syntax::error::CompileError;
// #1759: every namespace a test module imports, from one definition.
use crate::ts_build::*;
use bynk_check::checker::Types;
use bynk_check::d1_migrations::D1Migrations;
use bynk_check::test_suites::{self};
use bynk_check::wire_format;
use bynk_syntax::ast::{Case, Visibility};

// -- v0.7 / v0.118: test declaration processing --

mod case_body;
mod harness;
mod imports;
mod integration_module;
mod names;
mod property_generators;
mod property_tests;
mod runtime_splice;
mod stubs;
mod system_jobs;
mod system_support;
mod test_cases;
mod test_deps;
mod test_main;
mod test_module;
mod test_runtime_modules;
pub use self::harness::RunnableTest;
pub use self::harness::discovery_manifest;
use self::harness::*;
use self::imports::*;
use self::integration_module::*;
use self::names::*;
use self::property_generators::*;
use self::property_tests::*;
use self::runtime_splice::*;
use self::stubs::*;
use self::system_jobs::*;
use self::system_support::*;
use self::test_cases::*;
pub use self::test_deps::TestAssetFiles;
use self::test_deps::*;
pub use self::test_main::emit_test_main;
use self::test_main::*;
use self::test_module::*;

/// #2685: the tail of the synthesised smoke suite's file name (`<stem>.smoke.bynk`), which a
/// test build reads to name its typed routes to the decoder.
pub const SMOKE_SUITE_SUFFIX: &str = ".smoke.bynk";

/// #3386: the fixed signing key a `SignedCookie` actor's key variable takes in a test run (base64url of
/// 32 bytes), as the verifier refuses the short `TEST_SECRET`.
pub const TEST_SESSION_KEY: &str = "Ynluay10ZXN0LXNlc3Npb24ta2V5LTAxMjM0NTY3ODk";

/// The name of the synthetic case. `bynk-driver` lowers a case of this name
/// with its tail reported, and the run selects it by this name.
pub const EVAL_CASE: &str = "bynk eval";

/// 1A.34: a checker fact a suite's lowering needed and did not find is a compiler fault, reported
/// as a `bynk.internal.missing_checker_fact` diagnostic at the `.bynk` span (the case body's), and
/// the module's output is not used. Read once the module has lowered; the result says whether any
/// fault was recorded, so the caller drops the module's output rather than stage it.
///
/// A project that already has an error-severity diagnostic (in any suite: `errors` is shared by
/// every suite) is not reported: a program that failed checking lacks facts by construction (a
/// mixed-tier suite has no receiver namespace), so a fault there is noise on top of the real error.
/// The faults are still drained and the output still dropped. Warnings do not count: a build with
/// only warnings succeeds, so its fault must be reported, or the dropped module would go unexplained.
fn drain_internal_faults(errors: &mut Vec<CompileError>, runtime_use: &RuntimeUse) -> bool {
    let faults = runtime_use.take_faults();
    let faulted = !faults.is_empty();
    let failed = errors
        .iter()
        .any(|e| bynk_syntax::Severity::for_error(e) == bynk_syntax::Severity::Error);
    if !failed {
        errors.extend(
            faults
                .into_iter()
                .map(crate::emitter::InternalFault::into_compile_error),
        );
    }
    faulted
}

/// P5.4: `test <target>` suite processing. The checking half (target/`stub`
/// resolution, duplicate-case-name detection, case/property body
/// type-checking — formerly this function's own Phases 2-4) now lives in
/// [`test_suites::phase_test_bodies`]; only a target that passed every gate
/// comes back, keyed by its resolved `stub` map, so the loop below is Phase
/// 5 (TypeScript emission) alone, over exactly the targets ready for it.
#[allow(clippy::too_many_arguments)]
pub fn process_tests(
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
    // #2052: each events service's shape, for whether `svc.event(e)` forwards the
    // envelope (`EventSubscriberShape::wants_envelope`).
    unit_event_subscriber_shapes: &HashMap<
        String,
        HashMap<String, bynk_model::EventSubscriberShape>,
    >,
    // The driver's `project_mentions_encode_plain(parsed)` (#2991), which stays there: a suite
    // calling `Json.encodePlain` needs its own `Option` encoders to carry the plain branch.
    plain_json: bool,
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
        let runtime_use = RuntimeUse::default();
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
            unit_event_subscriber_shapes.get(target_name),
            target_uses_emit(
                target_name,
                target_kind,
                unit_tables,
                unit_flattened,
                unit_callees,
            ),
            d1_migrations,
            snapshot_files,
            plain_json,
            &runtime_use,
            tys,
        );
        let emit_out = if drain_internal_faults(errors, &runtime_use) {
            None
        } else {
            emit_out
        };
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
                if let Some(barrel) = crate::emitter::commons_barrel::emit_commons_barrel(
                    name,
                    groups,
                    parsed,
                    import_ext,
                    emitted_barrels,
                    &[],
                ) {
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
/// The `D1` inputs an integration suite is processed with: each database's
/// migrations, run on every test database (P4b, #2608), and the schemas they
/// build, which a `D1.peek` read in a case is prepared against (#3147; `None`
/// under `[check] d1_sql = false`).
#[derive(Clone, Copy)]
pub struct IntegrationD1<'a> {
    pub migrations: &'a D1Migrations,
    pub sql: Option<&'a bynk_check::d1_sql::D1Sql>,
}

impl<'a> IntegrationD1<'a> {
    pub fn new(migrations: &'a D1Migrations, sql: Option<&'a bynk_check::d1_sql::D1Sql>) -> Self {
        Self { migrations, sql }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn process_integration_tests(
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
    d1: IntegrationD1<'_>,
    // #1959: each context's Worker bindings, bound on each participant's
    // `env`. `None` when earlier errors stop the build anyway.
    context_bindings: Option<&BTreeMap<String, Vec<WorkerBinding>>>,
    // `bynk_check::symbols::discover_event_subscribers`' table.
    event_subscribers: &BTreeMap<(String, String), Vec<(String, String)>>,
    // #2138: a remote test build (`CompileOptions::remote_tests`).
    remote: Option<&crate::emit_model::RemoteTests>,
    // The driver's `project_mentions_encode_plain(parsed)` (#2991), which stays there: a suite
    // calling `Json.encodePlain` needs its own `Option` encoders to carry the plain branch.
    plain_json: bool,
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
        d1.sql,
    );
    let d1_migrations = d1.migrations;

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
                    tier: case_effective_tier(case, d),
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
        let runtime_use = RuntimeUse::default();
        let emitted = emit_integration_module(
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
            plain_json,
            &runtime_use,
        );
        let emitted = if drain_internal_faults(errors, &runtime_use) {
            None
        } else {
            emitted
        };
        if let Some((path, program, source_map, runnable)) = emitted {
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
                if let Some(barrel) = crate::emitter::commons_barrel::emit_commons_barrel(
                    name,
                    groups,
                    parsed,
                    ImportExt::Js,
                    emitted_barrels,
                    &[],
                ) {
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

    /// A type reference as the text of its `TsType` node.
    fn ts_type_ref(r: &TypeRef) -> String {
        bynk_ts::print_type(&emitter::ts_types::ts_type_of(r))
    }

    fn faulted_use() -> RuntimeUse {
        let ru = RuntimeUse::default();
        ru.note_internal_fault(Span::new(1, 2), "receiver namespace".to_string());
        ru
    }

    #[test]
    fn a_fault_is_reported_when_the_project_has_no_other_error() {
        let mut errors = Vec::new();
        assert!(drain_internal_faults(&mut errors, &faulted_use()));
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].category, crate::emitter::MISSING_CHECKER_FACT);
    }

    #[test]
    fn a_fault_in_a_project_that_already_has_errors_is_drained_not_reported() {
        let mut errors = vec![CompileError::new("test.other", Span::new(0, 1), "x")];
        let ru = faulted_use();
        assert!(
            drain_internal_faults(&mut errors, &ru),
            "the output is still dropped"
        );
        assert_eq!(errors.len(), 1, "no internal error on top of the real one");
        assert!(ru.take_faults().is_empty(), "drained");
    }

    #[test]
    fn a_fault_beside_only_warnings_is_still_reported() {
        let warning = bynk_syntax::diagnostics::REGISTRY
            .iter()
            .find(|d| d.severity == bynk_syntax::Severity::Warning)
            .expect("the registry has a warning")
            .code;
        let mut errors = vec![CompileError::new(warning, Span::new(0, 1), "x")];
        let ru = faulted_use();
        assert!(drain_internal_faults(&mut errors, &ru));
        assert_eq!(errors.len(), 2, "the warning, then the reported fault");
        assert_eq!(errors[1].category, crate::emitter::MISSING_CHECKER_FACT);
    }

    #[test]
    fn ts_type_ref_bases_and_generics() {
        assert_eq!(ts_type_ref(&base(BaseType::Int)), "number");
        assert_eq!(ts_type_ref(&base(BaseType::Float)), "number");
        assert_eq!(ts_type_ref(&base(BaseType::String)), "string");
        assert_eq!(ts_type_ref(&base(BaseType::Bool)), "boolean");
        assert_eq!(ts_type_ref(&named("Order")), "Order");
        assert_eq!(
            ts_type_ref(&TypeRef::List(Box::new(named("Order")), Span::default())),
            "readonly Order[]"
        );
        assert_eq!(
            ts_type_ref(&TypeRef::Option(
                Box::new(base(BaseType::Int)),
                Span::default()
            )),
            "Option<number>"
        );
        assert_eq!(
            ts_type_ref(&TypeRef::Effect(
                Box::new(TypeRef::Unit(Span::default())),
                Span::default()
            )),
            "globalThis.Promise<void>"
        );
        assert_eq!(
            ts_type_ref(&TypeRef::Map(
                Box::new(base(BaseType::String)),
                Box::new(named("V")),
                Span::default()
            )),
            "ReadonlyMap<string, V>"
        );
        assert_eq!(
            ts_type_ref(&TypeRef::Result(
                Box::new(named("T")),
                Box::new(named("E")),
                Span::default()
            )),
            "Result<T, E>"
        );
        assert_eq!(
            ts_type_ref(&TypeRef::HttpResult(Box::new(named("T")), Span::default())),
            "HttpResult<T>"
        );
        assert_eq!(
            ts_type_ref(&TypeRef::ValidationError(Span::default())),
            "ValidationError"
        );
        assert_eq!(
            ts_type_ref(&TypeRef::DecodeError(Span::default())),
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
        assert_eq!(ts_type_ref(&f), "(a0: number, a1: Order) => void");
    }

    #[test]
    fn ts_type_ref_qualified_prefixes_only_scoped_names() {
        let mut scope: HashSet<String> = HashSet::new();
        scope.insert("Order".to_string());
        // A named type in the privileged scope is qualified with the namespace.
        assert_eq!(
            bynk_ts::print_type(&emitter::ts_types::ts_type_ref_qualified_ts_type(
                &named("Order"),
                &scope,
                "Ns"
            )),
            "Ns.Order"
        );
        // A named type outside the scope is left bare.
        assert_eq!(
            bynk_ts::print_type(&emitter::ts_types::ts_type_ref_qualified_ts_type(
                &named("Other"),
                &scope,
                "Ns"
            )),
            "Other"
        );
        // Qualification recurses through generic arguments.
        assert_eq!(
            bynk_ts::print_type(&emitter::ts_types::ts_type_ref_qualified_ts_type(
                &TypeRef::List(Box::new(named("Order")), Span::default()),
                &scope,
                "Ns"
            )),
            "readonly Ns.Order[]"
        );
        // Base types are unaffected by qualification.
        assert_eq!(
            bynk_ts::print_type(&emitter::ts_types::ts_type_ref_qualified_ts_type(
                &base(BaseType::Int),
                &scope,
                "Ns"
            )),
            "number"
        );
    }

    /// #2138: only a remote build reads `BYNK_TEST_RUN_ID`; an in-process
    /// module's `run.id` is `"local"` whatever the environment says.
    #[test]
    fn run_id_reads_the_environment_only_in_a_remote_build() {
        let remote = bynk_ts::print_stmt(&run_id_stmt(true), 0);
        assert!(remote.contains("BYNK_TEST_RUN_ID"));
        let local = bynk_ts::print_stmt(&run_id_stmt(false), 0);
        assert!(!local.contains("BYNK_TEST_RUN_ID"));
        assert!(local.contains("\"local\""));
    }
}
