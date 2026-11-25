//! The per-suite test module, its barrel and the runtime-helper emitters.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
use crate::emitter::RuntimeUse;
// #1759: every namespace a test module imports, from one definition.
use crate::emitter::emit::test_scaffold_ns as test_ns;
use bynk_check::checker::Types;
use bynk_check::d1_migrations::{D1Migrations, d1_database_name, double_files, pglite_on};
use bynk_check::test_suites::{self, ResolvedStub};
use bynk_syntax::ast::{Block, Expr, ExprKind, FnName, StubClause, StubRhs, TypeRef, Visibility};
use bynk_ts::{
    SourceMapBuilder, TsBinaryOp, TsBindingName, TsDecl, TsExpr, TsObjectEntry, TsParam, TsProgram,
    TsStmt, TsType,
};
use std::sync::Arc;

/// The runtime factory of the fake a `bynk.cloudflare` double wraps its real
/// provider around, for the capabilities whose test double *is* the provider
/// (P4a #2602: `D1` over in-memory SQLite; #3302: `Email` over an outbox;
/// #3321: `Postgres` over PGlite, when the project opts in).
fn real_provider_double_factory(owner: &str, cap: &str, pglite: bool) -> Option<&'static str> {
    if owner != bynk_check::firstparty::CLOUDFLARE_UNIT {
        return None;
    }
    match cap {
        "D1" => Some("__makeIntegrationD1"),
        "Email" => Some("__makeIntegrationEmail"),
        "Vectorize" => Some("__makeIntegrationVectorize"),
        // #3321: only when the project opts in (`[test] postgres = "pglite"`).
        "Postgres" if pglite => Some("__makeIntegrationPostgres"),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_test_module(
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
    asset_harness: Option<Vec<TsStmt>>,
    // #2841: the module-level `string` constant for each fixture file a suite
    // here declares ([`fixture_consts`]).
    fixture_consts: Option<Vec<TsStmt>>,
    binding_modules: &HashMap<String, String>,
    schema_versions: &HashMap<String, i64>,
    // #2052: the target's own events services' shapes.
    event_shapes: Option<&HashMap<String, bynk_model::EventSubscriberShape>>,
    // #2050: whether the target's handlers emit ([`target_emits_events`]).
    uses_emit: bool,
    // P4b (#2608): each `D1` database's migrations.
    d1_migrations: &D1Migrations,
    // #2842: the stored snapshot files, inlined for the suites that take one.
    snapshot_files: Option<&BTreeMap<String, String>>,
    plain_json: bool,
    runtime_use: &RuntimeUse,
    tys: &Arc<Types>,
) -> Option<(PathBuf, TsProgram, Option<String>, RunnableTest)> {
    // #914: the test-scaffold module's runtime import list is emitted below as a
    // fixed set, but the bodies spliced into it lower through the ordinary `Bytes`
    // and boundary-codec paths. What those reach for is recorded here as they emit
    // and injected into the import line as a post-pass, the same shape
    // `emit_project` and `emit_worker_entry` use.
    // #3536: every suite type of this module, so a name a suite declares is not
    // also a first-party type the target's braces leave out.
    let all_suite_types: Vec<bynk_syntax::ast::TypeDecl> = indices
        .iter()
        .filter_map(|&i| parsed[i].test())
        .flat_map(|d| d.types.iter().cloned())
        .collect();
    // #2991: a suite calling `Json.encodePlain` needs its own `Option` encoders to
    // carry the plain branch.
    runtime_use.set_plain_json(plain_json);
    runtime_use.set_json_codec_qual(json_codec_qual_for_target(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        &all_suite_types,
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
        "Generated by bynk — do not edit by hand.",
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
    let runtime_import = emitter::runtime_import_for(&module_path, import_ext);
    let import_idx = stmts.len();

    // Compute relative import path from tests/ to the target's output dir.
    let import_target = relative_import_for_test(&target_dir);
    stmts.push(TsStmt::decl(
        TsDecl::ImportNamespace {
            type_only: false,
            alias: target_ns.clone(),
            from: sibling_specifier(&import_target, ext),
        },
        None,
    ));

    // The deps factory, built here for the units its providers name (#1863),
    // which are imported with the consumed contexts below.
    let uses_send = target_hands_off_to_exec(target_name, target_kind, unit_tables);
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
        None,
    );
    let has_integration_case = indices.iter().any(|&i| {
        parsed[i].test().is_some_and(|t| {
            t.cases
                .iter()
                .any(|c| case_effective_tier(c, t) != bynk_syntax::ast::TestTier::Unit)
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
            Some(runtime_use),
        )
    };
    let integration_deps = (target_kind == UnitKind::Context
        && has_integration_case
        && integration_factory("__makeTestDeps").stmt != deps.stmt)
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
                from: sibling_specifier(path, ext),
            },
            None,
        ));
    }
    // #4190: the codecs a stubbed consumed service's calls cross at `integration`.
    if integration_deps.is_some() {
        let stubbed: BTreeSet<&String> = stubs
            .values()
            .filter_map(|rp| rp.service_context.as_ref())
            .collect();
        for ctx in stubbed {
            stmts.push(super::stub_wire::wire_import(ctx, ext));
        }
    }
    // #1975: the binding module of each adapter a factory builds for real,
    // under the alias the composition root gives it. K4: and `bynk.crypto`'s,
    // whose test double is its own binding ([`platform_double`]). P4a
    // (#2602): and `bynk.cloudflare`'s for a `D1` double, which is the real
    // `WorkersD1` over an in-memory database.
    let pglite = pglite_on(d1_migrations);
    let all_doubles = || {
        deps.doubles
            .iter()
            .chain(integration_deps.iter().flat_map(|d| d.doubles.iter()))
    };
    let mut binding_units: BTreeSet<&String> = deps_units.clone();
    binding_units.extend(
        all_doubles()
            .filter(|(owner, cap, _)| {
                owner == bynk_check::firstparty::CRYPTO_UNIT
                    || owner == bynk_check::firstparty::TESTING_UNIT
                    || real_provider_double_factory(owner, cap, pglite).is_some()
            })
            .map(|(owner, _, _)| owner),
    );
    for unit in &binding_units {
        // P4a (#2602): `bynk.cloudflare`'s binding is first-party, so it is
        // never in `binding_modules` (a test keeps its doubles); the `D1`
        // double imports it from its fixed path.
        // #3458: so is `bynk.testing`'s, which is its own double.
        let first_party_binding = match unit.as_str() {
            bynk_check::firstparty::CLOUDFLARE_UNIT => {
                Some(bynk_check::firstparty::CLOUDFLARE_BINDING_FILENAME)
            }
            bynk_check::firstparty::TESTING_UNIT => {
                Some(bynk_check::firstparty::TESTING_BINDING_FILENAME)
            }
            _ => None,
        };
        let first_party = first_party_binding.map(|f| f.trim_end_matches(".ts").to_string());
        if let Some(module) = binding_modules.get(*unit).or(first_party.as_ref()) {
            stmts.push(TsStmt::decl(
                TsDecl::ImportNamespace {
                    type_only: false,
                    alias: format!("{}__binding", unit.replace('.', "_")),
                    from: parent_specifier(module, ext),
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
    // #2994: a stub class's method signature names the stubbed capability's
    // types by the namespace declaring them (`stubs::emit_stub_class`) — the
    // providing unit's own, or a commons it `uses` — which the target need not
    // `uses` itself.
    for rp in stubs.values() {
        let Some(owner) = unit_tables
            .get(target_name)
            .and_then(|t| t.flattened_caps.get(&rp.cap))
        else {
            continue;
        };
        for used in unit_uses.get(owner).into_iter().flatten() {
            let ns = test_ns(used);
            if used != target_name && !consumed_imports.iter().any(|(n, _)| *n == ns) {
                uses_imports.push((ns, relative_import_for_test(&commons_dir_for(used))));
            }
        }
    }
    uses_imports.sort();
    uses_imports.dedup();
    for (ns, path) in &uses_imports {
        stmts.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: ns.clone(),
                from: sibling_specifier(path, ext),
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
    stmts.push(TestRuntimeModule::Expectation.splice());
    // #1883, slice 3: the embedded files and `__bynkAssetGet`, for an asset
    // fetch in a case.
    if let Some(assets) = &asset_harness {
        stmts.extend(assets.iter().cloned());
    }
    if let Some(consts) = &fixture_consts {
        stmts.extend(consts.iter().cloned());
    }
    // #2842: the stored snapshots and their comparison, for a suite that takes one.
    let snapshot_keys = module_snapshot_keys(indices, parsed);
    if !snapshot_keys.is_empty() {
        stmts.extend(snapshot_harness(&snapshot_keys, snapshot_files));
    }
    // #1994: a target whose handlers use `detach` gets the execution-context
    // double its deps carry as `__exec`, and the settle helper that awaits
    // the collected `waitUntil` promises after each addressed call.
    if uses_send {
        stmts.push(TestRuntimeModule::Send.splice());
    }
    // #2137: a module whose cases `advance Clock` gets the harness clock, and
    // its `Clock` double reads it (fresh per case, from the epoch).
    let uses_advance = indices.iter().any(|&i| {
        parsed[i]
            .test()
            .is_some_and(|t| t.cases.iter().any(|c| case_advances_clock(&c.body)))
    });
    // #2138: `run.id`, for a module whose cases read it.
    if indices.iter().any(|&i| {
        parsed[i]
            .test()
            .is_some_and(|t| t.cases.iter().any(|c| case_reads_run_id(&c.body)))
    }) {
        stmts.push(run_id_stmt(false));
    }
    // #2683: a module on virtual time takes the timer-queue clock; the plain
    // counter clock is only for one that `advance`s and has no timers.
    let virtual_time = module_is_virtual_time(target_name, target_kind, stubs, unit_tables);
    if virtual_time {
        runtime_use.note_virtual_time();
        stmts.push(TestRuntimeModule::VirtualTime.splice());
    } else if uses_advance {
        stmts.push(TestRuntimeModule::Clock.splice());
    }
    let uses_advance = uses_advance || virtual_time;
    let mut suppress_next_blank = true;

    // v0.115: the target unit's contracted free functions the runner can attack
    // (each `ensures` is a generative check that is always on). Emitted only when
    // the contract guard is (the attack relies on the guard to assert `ensures`);
    // `bynk test` sets both, `bynk compile` neither.
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
        let mut stmt = TestRuntimeModule::Property.splice();
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
        let mut stmt = TestRuntimeModule::History.splice();
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
            .is_some_and(|t| t.cases.iter().any(|c| case_observes(&c.body)))
    });
    if uses_observation {
        let record_types = observation_call_record_types(
            target_name,
            unit_tables,
            unit_uses,
            unit_consumes,
            unit_consumes_aliases,
        );
        // Review of #1486, finding 2: pre-conversion, this whole section was
        // `out.push_str(observation_runtime_helpers()); out.push('\n');
        // extend_printed_at(&mut out, record_types); out.push('\n');` — the
        // *second* `push('\n')` ran unconditionally, so an empty
        // `record_types` (a `unit_tables` miss, or a target with no
        // capabilities/ops) still got two blank lines after the helpers, not
        // one. A blank statement after the helpers when there is nothing to
        // glue after them reproduces that; otherwise the printer's automatic
        // policy alone supplies the one blank before the first record type,
        // matching the old single `push('\n')` between them exactly.
        let mut stmt = TestRuntimeModule::Observation.splice();
        if suppress_next_blank {
            stmt.no_blank_before = true;
            suppress_next_blank = false;
        }
        stmts.push(stmt);
        if record_types.is_empty() {
            stmts.push(TsStmt::blank(None));
        }
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
            runtime_use,
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
        stmts.extend(env_declaration(
            "#2134: the running case's `env` entries, set by its runner.",
        ));
    }

    // #291: the overlay `stub` layers over a seam's tier default, and the
    // deterministic test double of each platform capability the target
    // consumes.
    if !stubs.is_empty() {
        stmts.push(TestRuntimeModule::StubOverlay.splice());
    }
    // #4190: and the round trip a stubbed consumed service makes at `integration`.
    if integration_deps.is_some() && stubs.values().any(|rp| rp.service_context.is_some()) {
        stmts.push(TestRuntimeModule::StubWire.splice());
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
        // P4b (#2608): a `D1` double runs its database's migrations.
        // #3321: and a `Postgres` double its `schema.sql`, when the project
        // opts in to PGlite.
        let database = unit_tables
            .get(target_name)
            .and_then(|t| t.derived.get(name))
            .map(|d| d1_database_name(&bynk_project::worker_dir_name(target_name), &d.binding));
        let migrations = double_files(d1_migrations, cap, database);
        let def = if with_env {
            env_secrets_double(owner, cap, &test_ns(owner), name).or_else(|| {
                platform_double(owner, cap, &test_ns(owner), name, uses_advance, migrations)
            })
        } else {
            platform_double(owner, cap, &test_ns(owner), name, uses_advance, migrations)
        };
        if let Some(def) = def {
            stmts.push(def);
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
                tier: Some(case_effective_tier(case, test_decl).as_str()),
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
                    case_effective_tier(case, test_decl).as_str()
                ),
                None,
            ));
            let tier = case_effective_tier(case, test_decl);
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
                test_suites::SuiteItems::of(test_decl),
                case,
                with_env.then(|| bynk_check::test_env::case_env_values(test_decl, case)),
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
                runtime_use,
                suite,
                hidden,
                (!test_suites::snapshot_expects(&case.body).is_empty())
                    .then(|| bynk_project::snapshots::snapshot_path(&parsed[i].identity_path()))
                    .as_deref(),
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
            case_stmt.nested_map_source_id = source_id_of(&mut map, &parsed[i]);
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
            let runner_name = prop_runner_name(&sanitise_case_name(&prop.name, &mut idx));
            let prop_ordinal = prop_runners.len();
            prop_runners.push(runner_name.clone());
            // v0.119: a `for all run: History[Agent]` property routes to the driven-
            // sequence runner; a value property keeps the existing path.
            let mut prop_stmt = if prop_is_history(prop) {
                emit_test_history_property_function(
                    &runner_name,
                    test_suites::SuiteItems::of(test_decl),
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
                    runtime_use,
                    suite,
                    hidden,
                    tys,
                )
            } else {
                emit_test_property_function(
                    &runner_name,
                    test_suites::SuiteItems::of(test_decl),
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
                    runtime_use,
                    suite,
                    hidden,
                    tys,
                )
            };
            // Its statements' spans are offsets into its own test file.
            prop_stmt.nested_map_source_id = source_id_of(&mut map, &parsed[i]);
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
        let attack_name = contract_case_name(&fname.name);
        discovered.push(DiscoveredCase {
            name: attack_name.clone(),
            location: None,
            tier: None,
            generative: Some("property"),
            doc: false,
        });
        let mut idx = prop_runners.len();
        let runner_name = prop_runner_name(&sanitise_case_name(&attack_name, &mut idx));
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
            runtime_use,
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
    // #3066: a root naming a suite's own type is served by that suite's runner
    // (its codecs are declared inside it), not by this module-level closure.
    // A name the module-level view also knows (a target or `uses` type that
    // another suite of this module reaches under that name) stays here: the
    // suite that declares its own copy shadows it locally, and the others
    // need this one.
    let synthetic = synthetic_typed_commons_for_target(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        hidden,
        tys,
        &all_suite_types,
    );
    let suite_type_names: HashSet<String> = indices
        .iter()
        .filter_map(|&i| parsed[i].test())
        .flat_map(|d| d.types.iter().map(|t| t.name.name.clone()))
        .filter(|n| !synthetic.types.contains_key(n))
        .collect();
    let json_codec_roots: Vec<TypeRef> = runtime_use
        .take_json_codec_roots()
        .into_iter()
        .filter_map(|ty| crate::emitter::ty_to_type_ref(ty, tys))
        .filter(|r| !crate::emitter::serialisation::root_names_any(r, &suite_type_names))
        .collect();
    if !json_codec_roots.is_empty() {
        // #3699: a `trace(Cap.op)` list's elements are the synthetic call-record
        // types the case bodies were checked with, so their codecs belong in the
        // closure beside the target's types.
        let mut closure_types = synthetic.types.clone();
        if uses_observation {
            closure_types.extend(call_record_type_decls(
                target_name,
                unit_tables,
                unit_uses,
                unit_consumes,
                unit_consumes_aliases,
                &synthetic.types,
            ));
        }
        stmts.extend(codec_closure_stmts(
            &json_codec_roots,
            &closure_types,
            target_name,
            runtime_use,
        ));
    }

    // #2052: a delivery helper per events service some body addressed with
    // `svc.event(e)`. Appended after every body has lowered, like the codec
    // closure above; a plain `function` declaration hoists.
    let event_services = runtime_use.test_event_services();
    if !event_services.is_empty() {
        stmts.extend(event_delivery_helpers(
            target_name,
            &event_services,
            unit_tables,
            unit_consumes,
            schema_versions,
            event_shapes,
            runtime_use,
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
        run_entries.push((contract_case_name(&fname.name), runner_name.clone()));
        prop_index += 1;
    }
    stmts.extend(build_run_function(&run_entries));

    // The runtime import is what the finished tree uses (1B.5e, #4054 Decision E): every body
    // above is a node, or text that names the runtime names it spells (`raw_using`), so the
    // closure sees each one. It splices in at its reserved position.
    let runtime_names = emitter::runtime_header::derived_test_runtime_names(&stmts);
    stmts.splice(
        import_idx..import_idx,
        emitter::runtime_files::runtime_import_stmts(false, &runtime_names, &runtime_import),
    );

    let module_file = module_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "module.ts".to_string());
    let program = checked_program(stmts);
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
            detach_reports: uses_send,
            snapshot_files: snapshot_keys.keys().cloned().collect(),
        },
    ))
}

/// Render the relative import path from the `tests/` output directory to the
/// directory holding a target unit's TypeScript output.
pub(super) fn relative_import_for_test(target_dir: &Path) -> String {
    let parts: Vec<String> = target_dir
        .components()
        .filter_map(|c| {
            if let Component::Normal(s) = c {
                Some(s.to_string_lossy().to_string())
            } else {
                None
            }
        })
        .collect();
    if parts.is_empty() {
        "../index".to_string()
    } else {
        format!("../{}", parts.join("/"))
    }
}

/// #2842: the snapshots each of the module's suite files takes, keyed by the
/// file's snapshot path ([`bynk_project::snapshots::snapshot_path`]): every
/// `(case, name)` pair its `expect … matches snapshot` claims name, in source
/// order. Empty when no suite of the module takes one.
pub(super) fn module_snapshot_keys(
    indices: &[usize],
    parsed: &[ParsedFile],
) -> BTreeMap<String, Vec<(String, String)>> {
    let mut out: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for &i in indices {
        let Some(test_decl) = parsed[i].test() else {
            continue;
        };
        let mut pairs: Vec<(String, String)> = Vec::new();
        for case in &test_decl.cases {
            for e in test_suites::snapshot_expects(&case.body) {
                let name = e.snapshot.as_ref().expect("filtered").name.clone();
                let pair = (case.name.clone(), name);
                if !pairs.contains(&pair) {
                    pairs.push(pair);
                }
            }
        }
        if !pairs.is_empty() {
            out.insert(
                bynk_project::snapshots::snapshot_path(&parsed[i].identity_path()),
                pairs,
            );
        }
    }
    out
}

/// #2842: the snapshot runtime and `__bynkSnapshotFiles`, the stored text of each
/// of the module's snapshot files (`null` for one not yet recorded) beside the
/// keys its claims name. Inlined so the harness reads nothing at test time.
pub(super) fn snapshot_harness(
    keys: &BTreeMap<String, Vec<(String, String)>>,
    stored: Option<&BTreeMap<String, String>>,
) -> Vec<TsStmt> {
    use crate::ts_build::raw_lit;
    // A file name and a stored text keep the JSON escaping they always had.
    let json = |text: &str| raw_lit(bynk_project::json_string(text));
    let entries: Vec<TsObjectEntry> = keys
        .iter()
        .map(|(file, pairs)| {
            let text = stored
                .and_then(|files| files.get(file))
                .map_or_else(crate::ts_build::null_lit, |t| json(t));
            let live = TsExpr::array(
                pairs
                    .iter()
                    .map(|(c, n)| TsExpr::array(vec![json(c), json(n)]))
                    .collect(),
            );
            TsObjectEntry::prop(
                bynk_project::json_string(file),
                TsExpr::object(vec![
                    ("stored".to_string(), text),
                    ("live".to_string(), live),
                ]),
            )
        })
        .collect();
    let mut table = TsStmt::const_stmt(
        TsBindingName::Ident("__bynkSnapshotFiles".to_string()),
        Some(TsType::named("globalThis.Record<string, __SnapshotFile>")),
        TsExpr::multiline_object_entries(entries),
        None,
    );
    // The table follows the runtime's last line directly.
    table.no_blank_before = true;
    vec![TestRuntimeModule::Snapshot.splice(), table]
}

/// The id under which `file` is a source of the module's map, so the statements lowered from it
/// record their spans against its text.
fn source_id_of(map: &mut SourceMapBuilder, file: &ParsedFile) -> usize {
    map.add_source(file.map_source_name(), file.source().to_string())
}

/// #2052: the `__bynkEvent_<svc>` helper behind each `svc.event(payload[,
/// envelope])` address in a test module. It delivers as the Worker's
/// `/_bynk/event/` route and the Bundle fan-out do: through the service's
/// generated `event` method, whose prologue runs the subscription's pattern
/// filter and `via schema(N)` guard, so the helper re-implements neither. The
/// method takes the envelope only when the handler declares `env` or the
/// service has a `via schema(N)` guard (`EventSubscriberShape::wants_envelope`), so
/// the helper forwards it only then.
///
/// An omitted envelope (`undefined`) is supplied as an emission would mint it:
/// a fresh `eventId` per call (`test-event-<n>`, counted across the module), the
/// publishing context as `publisherId`, the test clock's reading (`0`, the
/// epoch) as `emittedAt`, and the event's current schema version.
/// Whether `svc`, an events service of `target_name`, is handed the envelope.
/// A hosted events service of a program that passed checking always has a shape
/// (`bynk check` and `bynk build` both record them). A miss is a checker fact
/// the suite's lowering needed and did not find, recorded as an internal fault
/// at the service (`drain_internal_faults`, which stays silent when the project
/// already has errors, as a half-edited program lacks shapes by construction):
/// `None` then, and the caller drops the helper (the module's output is dropped).
fn hosted_takes_envelope(
    event_shapes: Option<&HashMap<String, bynk_model::EventSubscriberShape>>,
    target_name: &str,
    svc: &str,
    span: bynk_syntax::span::Span,
    runtime_use: &RuntimeUse,
) -> Option<bool> {
    match event_shapes.and_then(|shapes| shapes.get(svc)) {
        Some(shape) => Some(shape.wants_envelope()),
        None => {
            runtime_use.note_internal_fault(
                span,
                [
                    "the events service `",
                    svc,
                    "` of unit `",
                    target_name,
                    "` has no recorded subscriber shape",
                ]
                .concat(),
            );
            None
        }
    }
}

pub(super) fn event_delivery_helpers(
    target_name: &str,
    services: &[String],
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    schema_versions: &HashMap<String, i64>,
    event_shapes: Option<&HashMap<String, bynk_model::EventSubscriberShape>>,
    runtime_use: &RuntimeUse,
) -> Vec<TsStmt> {
    use crate::ts_build::{as_expr, const_, ident, member, method_call, ret};
    use bynk_syntax::ast::ServiceProtocol;
    let ns = test_ns(target_name);
    // The per-service helpers; the counter they share is declared ahead of them once known.
    let mut helpers: Vec<TsStmt> = Vec::new();
    let mut counts = false;
    for svc in services {
        let Some(decl) = unit_tables
            .get(target_name)
            .and_then(|t| t.services.get(svc))
        else {
            continue;
        };
        // M-1's domain clause (ADR 0735; crate-hygiene B4, #3012): this
        // function holds the target's symbol table, not its `TypedCommons`, so it
        // doesn't hold `bynk_model::lower_protocol_ir`'s input.
        let ServiceProtocol::Events { event_type, .. } = &decl.protocol else {
            continue;
        };
        let TypeRef::Named(id) = event_type else {
            continue;
        };
        let event = id.name.clone();
        // The one envelope rule (1B.1c, Decision F of #3928).
        let Some(takes_envelope) =
            hosted_takes_envelope(event_shapes, target_name, svc, decl.span, runtime_use)
        else {
            continue;
        };
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
        let helper = |envelope_name: &str, mut body: Vec<TsStmt>, args: Vec<TsExpr>| {
            body.push(ret(method_call(
                member(ident(ns.clone()), svc.clone()),
                "event",
                args,
            )));
            let unknown = || Some(TsType::named("unknown"));
            TsStmt::decl(
                TsDecl::Function {
                    name: format!("__bynkEvent_{svc}"),
                    generics: Vec::new(),
                    params: vec![
                        TsParam::new("payload".to_string(), unknown()),
                        TsParam::new(envelope_name.to_string(), unknown()),
                        TsParam::new("deps".to_string(), unknown()),
                    ],
                    return_type: None,
                    body,
                    is_async: false,
                    inline: false,
                },
                None,
            )
        };
        let never = |name: &str| as_expr(ident(name), TsType::named("never"));
        if takes_envelope {
            let env = match &defaults {
                Some((owner, version)) => {
                    counts = true;
                    // `envelope ?? { eventId: `test-event-${++__bynkEventSeq}`, … }`
                    crate::ts_build::binary(
                        TsBinaryOp::NullishCoalescing,
                        ident("envelope"),
                        TsExpr::object(vec![
                            (
                                "eventId".to_string(),
                                TsExpr::template_lit(
                                    vec!["test-event-".to_string(), String::new()],
                                    vec![TsExpr::Unary {
                                        op: bynk_ts::TsUnaryOp::PreIncrement,
                                        expr: Box::new(ident("__bynkEventSeq")),
                                    }],
                                ),
                            ),
                            (
                                "publisherId".to_string(),
                                crate::ts_build::str_lit(owner.clone()),
                            ),
                            ("emittedAt".to_string(), crate::ts_build::num_lit(0)),
                            (
                                "schemaVersion".to_string(),
                                crate::ts_build::num_lit(version),
                            ),
                        ]),
                    )
                }
                None => ident("envelope"),
            };
            helpers.push(helper(
                "envelope",
                vec![const_("env", env)],
                vec![never("payload"), never("env"), never("deps")],
            ));
        } else {
            // The handler cannot observe an envelope, so a case's own is dropped.
            helpers.push(helper(
                "_envelope",
                Vec::new(),
                vec![never("payload"), never("deps")],
            ));
        }
    }
    // The helpers sit on consecutive lines, the counter they share ahead of them.
    let mut out: Vec<TsStmt> = Vec::new();
    if counts {
        out.push(crate::ts_build::let_init(
            "__bynkEventSeq",
            crate::ts_build::num_lit(0),
        ));
    }
    out.extend(helpers);
    for stmt in out.iter_mut().skip(1) {
        stmt.no_blank_before = true;
    }
    out
}

/// #2138: whether a case body reads `run.id` anywhere — the signal to emit the module's
/// `__bynkRunId` constant. It decides emission, not an import (1B.5e, #4054 Decision G).
pub(super) fn case_reads_run_id(block: &Block) -> bool {
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
        bynk_model::walk_exprs(e, &mut check);
    }
    bynk_model::walk_exprs(&block.tail, &mut check);
    found
}

/// #2137: whether a case body (or a block nested in it) holds an `advance` — the signal to
/// emit the harness clock and the `Clock` double that reads it. It decides emission, not an
/// import (1B.5e, #4054 Decision G).
pub(super) fn case_advances_clock(b: &Block) -> bool {
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
        bynk_model::walk_exprs(e, &mut |x: &Expr| {
            found |= match &x.kind {
                ExprKind::Block(inner) => case_advances_clock(inner),
                ExprKind::If {
                    then_block,
                    else_block,
                    ..
                } => case_advances_clock(then_block) || case_advances_clock(else_block),
                ExprKind::Match { arms, .. } => arms.iter().any(|a| {
                    matches!(&a.body, bynk_syntax::ast::MatchBody::Block(b) if case_advances_clock(b))
                }),
                ExprKind::IntLit { .. } | ExprKind::FloatLit { .. } | ExprKind::DecimalLit { .. } | ExprKind::DurationLit { .. } | ExprKind::StrLit(..) | ExprKind::InterpStr(..) | ExprKind::BoolLit(..) | ExprKind::Ident(..) | ExprKind::Call { .. } | ExprKind::Lambda(..) | ExprKind::BinOp(..) | ExprKind::UnaryOp(..) | ExprKind::Paren(..) | ExprKind::Ok(..) | ExprKind::Err(..) | ExprKind::Question(..) | ExprKind::RecordConstruction { .. } | ExprKind::FieldAccess { .. } | ExprKind::MethodCall { .. } | ExprKind::Is { .. } | ExprKind::Some(..) | ExprKind::None | ExprKind::UnitLit | ExprKind::RecordSpread { .. } | ExprKind::EffectPure(..) | ExprKind::Expect(..) | ExprKind::Val { .. } | ExprKind::Wire(..) | ExprKind::ListLit(..) | ExprKind::Observation(..) | ExprKind::Faults(..) | ExprKind::Trace { .. } => false,
            };
        });
    }
    found
}

/// #1994: whether a context target has a handler that hands work to the
/// execution context (`detach`, `within` or `race`, directly or through a function it
/// calls, #2719), so its handlers' `deps` require `__exec` — the same gate the Worker entry and
/// `compose` thread the request's execution context on.
pub(super) fn target_hands_off_to_exec(
    target_name: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
) -> bool {
    // A case body may call a function that takes the context directly (#2719),
    // which a handler need not; the case is lowered after this gate is read, so
    // a build with any such function gives every module's deps the context.
    unit_tables.values().any(|t| t.exec_plan.any_fn())
        || (target_kind == UnitKind::Context
            && unit_tables.get(target_name).is_some_and(|t| {
                t.services
                    .values()
                    .any(|s| s.handlers.iter().any(|h| t.exec_plan.handler_needs(h)))
            }))
}

/// #2683: whether a context target has a service handler that reaches
/// `e.within(d)` or `a.race(b)`, directly or through a function it calls
/// (#2719), so its `deps` carry an execution context whose timers the harness
/// supplies. A case body that calls such a function directly is read as the
/// same (see [`target_hands_off_to_exec`]).
pub(super) fn target_has_timers(
    target_name: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
) -> bool {
    unit_tables.values().any(|t| t.exec_plan.any_timed_fn())
        || (target_kind == UnitKind::Context
            && unit_tables.get(target_name).is_some_and(|t| {
                t.services
                    .values()
                    .any(|s| s.handlers.iter().any(|h| t.exec_plan.handler_timed(h)))
            }))
}

/// #2683: whether a `stub` takes virtual time: `returns <v> after <d>`, or
/// `never`.
pub(super) fn stub_takes_time(c: &StubClause) -> bool {
    c.after.is_some() || matches!(c.rhs, StubRhs::Never(_))
}

/// #2683: whether a test module runs on virtual time (a timer queue on the
/// harness clock, and auto-advance) rather than the plain counter clock: its
/// target calls `within`, or one of its `stub`s takes time. Any other module
/// keeps its output byte for byte.
pub(super) fn module_is_virtual_time(
    target_name: &str,
    target_kind: UnitKind,
    stubs: &HashMap<String, ResolvedStub>,
    unit_tables: &HashMap<String, UnitTable>,
) -> bool {
    target_has_timers(target_name, target_kind, unit_tables)
        || stubs
            .values()
            .any(|rp| rp.clauses.iter().any(stub_takes_time))
}

/// #2050: whether a context target has a handler — a service's or an agent's
/// — that emits through the first-party `Events`, so the `deps` its handlers
/// run on require `__eventsDispatch`: the same gate (`unit_table_uses_emit`)
/// the composition root builds its own dispatch closure on.
pub(super) fn target_emits_events(
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

/// #3699: the synthetic call-record types (`__Cap_op_Call`) a `trace(Cap.op)`
/// list's elements carry, as the case bodies were checked with them
/// ([`test_suites::register_call_record_types`]), less any name `known` already
/// declares.
fn call_record_type_decls(
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    known: &HashMap<String, Arc<bynk_syntax::ast::TypeDecl>>,
) -> Vec<(String, Arc<bynk_syntax::ast::TypeDecl>)> {
    let Some((mut resolved, _)) = test_suites::build_privileged_resolved(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    ) else {
        return Vec::new();
    };
    let before: HashSet<String> = resolved.types.keys().cloned().collect();
    test_suites::register_call_record_types(&mut resolved, target_name, unit_tables);
    resolved
        .types
        .into_iter()
        .filter(|(n, _)| !before.contains(n) && !known.contains_key(n))
        .collect()
}

/// #917: the module's own codec closure for `roots` over `types`: each named
/// type's `serialise_*`/`deserialise_*` helpers and each generic instance's.
/// Empty when the closure is.
fn codec_closure_stmts(
    roots: &[TypeRef],
    types: &HashMap<String, Arc<bynk_syntax::ast::TypeDecl>>,
    target_name: &str,
    runtime_use: &RuntimeUse,
) -> Vec<TsStmt> {
    use crate::emitter::serialisation as ser;
    let (codec_names, codec_insts) = ser::collect_codec_closure(roots, types);
    if codec_names.is_empty() && codec_insts.is_empty() {
        return Vec::new();
    }
    // The helpers below always name `JsonValue`/`BoundaryError` in their
    // own signatures, regardless of which arm (if any) the case bodies'
    // own `Json.decode`/`Json.encode` calls happened to trip.
    let qual = runtime_use.json_codec_qual();
    let mut stmts = ser::decls_as_stmts_block(ser::emit_helpers_for_owner_qualified(
        &codec_names,
        types,
        target_name,
        &qual,
        runtime_use,
    ));
    stmts.extend(ser::decls_as_stmts(ser::emit_generic_helpers_qualified(
        &codec_insts,
        types,
        &qual,
        runtime_use,
    )));
    stmts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emitter::commons_barrel::emit_commons_barrel;
    use bynk_syntax::ast::Ident;
    use bynk_syntax::ast::{Commons, CommonsForm, QualifiedName, Trivia};
    use bynk_syntax::span::Span;
    use std::collections::{BTreeMap, HashSet};
    use std::path::PathBuf;

    /// This test names AST types, so it lives under `testgen`, which the `ast_importers` probe does
    /// not count, though it tests the production barrel (`emitter::commons_barrel`).
    ///
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
            "../../../bynk-tests/tests/fixtures/positive/251_multi_file_commons_test/\
             expected/thing.ts"
        );
        let commons = |name: &str| {
            SourceUnit::Commons(Commons {
                test_only: false,
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
}

#[cfg(test)]
mod hosted_envelope_tests {
    use super::*;

    #[test]
    fn a_hosted_events_service_without_a_shape_is_an_internal_fault() {
        let ru = RuntimeUse::default();
        let span = bynk_syntax::span::Span::new(0, 0);
        assert_eq!(
            hosted_takes_envelope(Some(&HashMap::new()), "shop", "On", span, &ru),
            None
        );
        let faults = ru.take_faults();
        assert_eq!(faults.len(), 1);
        assert!(
            faults[0].what.contains(
                "the events service `On` of unit `shop` has no recorded subscriber shape"
            )
        );
    }
}
