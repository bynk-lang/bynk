use super::diagnostics::ProjectFailure;
use super::pipeline::ComposedBuild;
use super::pipeline::RunChecks;
use super::schema_registry;
use super::worker_bindings;
use bynk_check::checker::ExprId;
use bynk_check::d1_migrations::pglite_on;
use bynk_check::firstparty::{self};
use bynk_check::project_model::AdapterBinding;
use bynk_check::project_model::BuildTarget;
use bynk_check::symbols::UnitTable;
use bynk_emit::emit_model::Artefacts;
use bynk_emit::emit_model::Document;
use bynk_emit::emit_model::ImportExt;
use bynk_emit::emit_model::ProjectOutput;
use bynk_emit::emit_model::StagedFile;
use bynk_emit::emit_model::WASM_MODULES_DTS;
use bynk_emit::emit_model::WorkersBuild;
use bynk_emit::emit_model::harness_clock_contexts;
use bynk_emit::emit_model::sibling_path;
use bynk_emit::emit_model::wasm_shim_path;
use bynk_emit::emit_model::workers_wasm_shim;
use bynk_emit::emitter;
use bynk_emit::testgen::{RunnableTest, discovery_manifest, emit_test_main};
use bynk_project::AttributedError;
use bynk_project::UnitKind;
use bynk_project::discovery::ParsedFile;
use bynk_project::paths::render_package_json;
use bynk_project::worker_layout::WorkerLayout;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};

/// #1890: one `bynk.project.duplicate_compatibility_flag` warning per
/// duplicate in the manifest's `[workers] compatibility_flags` (`extra`),
/// shared by [`finish_build`] and [`check_project`](super::check_project) so `compile` and `check`
/// report the same thing. Attributed to `bynk.toml` by its root-relative
/// identity path, like every other [`AttributedError`]; the manifest is never
/// a snapshot, so renderers print that path (or none) rather than a source
/// excerpt, and the message names `bynk.toml` itself.
pub(super) fn compatibility_flag_warnings(extra: &[String]) -> Vec<AttributedError> {
    let (_, duplicates) = bynk_emit::compatibility_flags(extra);
    duplicates
        .into_iter()
        .map(|flag| {
            let default = bynk_emit::DEFAULT_COMPATIBILITY_FLAGS.contains(&flag.as_str());
            bynk_project::duplicate_compatibility_flag_warning(
                &flag,
                default,
                PathBuf::from("bynk.toml"),
            )
        })
        .collect()
}

/// Assemble a finished [`ProjectOutput`] (or a [`ProjectFailure`]) from a
/// [`RunChecks`] result — the shared tail of `compile_project` and
/// `compile_in_memory`.
///
/// #1890: `compatibility_flags` is the manifest's `[workers]` list
/// ([`CompileOptions::compatibility_flags`](super::CompileOptions::compatibility_flags)). It is merged with the defaults
/// once here, for every Worker, and each duplicate it held becomes one
/// [`compatibility_flag_warnings`] warning, whatever the build target: the
/// duplicate is a fact about the manifest.
pub(super) fn finish_build(
    run: RunChecks,
    import_ext: ImportExt,
    display_root: &Path,
    compatibility_flags: &[String],
    workers: emitter::workers_emit::WorkersSettings,
    // #3477: the directory the build is written to, when known.
    out_dir: Option<&Path>,
    // #2554: the pre-read `bynk.contracts.lock`; `None` is a project without
    // one, which accepts no older contract.
    contracts_lock: Option<&str>,
) -> Result<ProjectOutput, ProjectFailure> {
    // #2554: an unreadable contracts lock fails the build, as an unreadable
    // schema lock does: reading it as empty would have a callee silently stop
    // accepting the callers it was deployed to keep answering.
    let contracts_lock = match bynk_project::contracts_lock::parse(contracts_lock, display_root) {
        Ok(lock) => lock,
        Err(message) => {
            return Err(ProjectFailure {
                errors: vec![bynk_project::contracts_lock::corrupt_error(message)],
                snapshots: Vec::new(),
                display_root: display_root.to_path_buf(),
            });
        }
    };
    let (flags, _) = bynk_emit::compatibility_flags(compatibility_flags);
    let flag_warnings = compatibility_flag_warnings(compatibility_flags);
    match run {
        RunChecks::Bailed {
            errors, snapshots, ..
        } => Err(ProjectFailure {
            // ADR 0117: a failed build still renders any warnings it produced
            // (the sink yields errors then warnings).
            errors: errors.into_all().into_iter().chain(flag_warnings).collect(),
            snapshots,
            display_root: display_root.to_path_buf(),
        }),
        RunChecks::Checked {
            errors, snapshots, ..
        } if !errors.is_empty() => Err(ProjectFailure {
            errors: errors.into_all().into_iter().chain(flag_warnings).collect(),
            snapshots,
            display_root: display_root.to_path_buf(),
        }),
        RunChecks::Checked {
            errors,
            snapshots,
            parsed,
            compiled,
            runnable_tests,
            integration_outputs,
            integration_runnables,
            groups,
            kinds,
            unit_consumes,
            unit_consumes_aliases,
            unit_tables,
            unit_consumed_context_types,
            unit_callees,
            unit_uses,
            unit_flattened,
            adapter_bindings,
            npm_deps,
            target,
            trace_requests,
            inspect_agents,
            schema_registry,
            asset_bytes,
            agent_versions,
            d1_migrations,
            layout,
            composed,
            ..
        } => {
            let composed = composed.expect("a build with no errors has its compose plan");
            // #3477: a container's build paths are written from the output
            // directory, which has to be inside the project.
            let container_root = match worker_bindings::container_root(display_root, out_dir) {
                Ok(root) => root,
                Err(outside) => {
                    let errors = worker_bindings::container_output_errors(&unit_tables, &outside);
                    if !errors.is_empty() {
                        return Err(ProjectFailure {
                            errors,
                            snapshots,
                            display_root: display_root.to_path_buf(),
                        });
                    }
                    String::new()
                }
            };
            let mut out = build_output(
                parsed,
                compiled,
                runnable_tests,
                integration_outputs,
                integration_runnables,
                groups,
                kinds,
                unit_consumes,
                unit_consumes_aliases,
                unit_tables,
                &unit_consumed_context_types,
                unit_callees,
                unit_uses,
                unit_flattened,
                adapter_bindings,
                npm_deps,
                target,
                import_ext,
                &flags,
                workers,
                &container_root,
                asset_bytes.as_ref(),
                &agent_versions,
                &contracts_lock,
                &d1_migrations,
                &layout,
                trace_requests,
                inspect_agents,
                composed,
            );
            // ADR 0117: surface non-failing warnings on the successful build
            // (errors is empty here — the guard arm above caught any).
            // `build_output` put the contracts lock's warnings here (#2554).
            let lock_warnings = std::mem::take(&mut out.warnings);
            out.warnings = errors.into_warnings();
            out.warnings.extend(flag_warnings);
            out.warnings.extend(lock_warnings);
            out.snapshots = snapshots;
            out.display_root = display_root.to_path_buf();
            // #1078: the reconciled registry, if this build had one on —
            // bynk-emit computes it, the caller persists it.
            out.schema_lock = schema_registry.map(|reg| schema_registry::serialize(&reg));
            Ok(out)
        }
    }
}

// P4.1 (#1115): `normalize_service_defaults`/`inject_service_defaults`
// relocated to `bynk-check::project_model` (called from both `run_checks`
// here and the new `bynk-check`-native analysis entry point, ahead of
// `phase_group` in both).

// P4.1 (#1115): `record_analyse_types` relocated to
// `bynk-check::check_pipeline` (called from `check_file_core`'s own
// error-path exits, plus every clean-path caller — `check_unit_files`'s
// `Mode::Analyse` branch below and the new entry point's own).

// P4.1 (#1115): the whole discovery->parse->group->resolve pipeline
// (`phase_discovery` through `phase_file_index`/`assemble_unit_info`, plus
// per-unit symbol composition — `compose_unit_symbols`/
// `merge_consumed_exports`/`collect_unit_methods`) relocated to
// `bynk-check::project_model` — see that module's own doc comment for why
// (the same `extract, don't duplicate` move `bynk-project` itself was, P4.0).
// `run_checks`, below, is now a caller of `project_model::phase_*` instead of
// owning this logic inline.

/// `package.json`: always `"type": "module"` (#2148), so the emitted tsconfig's
/// `module: NodeNext` checks and builds the output as the ES modules it is, under
/// Node's ESM rules; plus the adapter-declared (v0.17) and first-party (ADR 0799
/// C) npm dependencies when there are any.
fn package_json_file(npm_deps: &std::collections::BTreeMap<String, String>) -> StagedFile {
    StagedFile {
        output_path: PathBuf::from("package.json"),
        document: Document::Json(render_package_json(npm_deps)),
        source_map: None,
        debug_metadata: None,
    }
}

/// ADR 0799 C: a runtime module that imports a first-party npm package brings it,
/// at its exact pin, into `package.json`.
fn runtime_with_npm(
    reached: &BTreeSet<(String, String)>,
    mut npm_deps: std::collections::BTreeMap<String, String>,
) -> (
    Vec<(PathBuf, String)>,
    std::collections::BTreeMap<String, String>,
) {
    let runtime_files = emitter::runtime_files::closure_files(reached);
    // A module's name is its path under the runtime directory without the extension, so a module
    // in a subdirectory (`testing/pglite`) keeps it (1B.3b, #4047).
    let modules: Vec<String> = runtime_files
        .iter()
        .filter_map(|(path, _)| {
            let rel = path.strip_prefix(emitter::RUNTIME_DIR).ok()?;
            Some(rel.with_extension("").to_string_lossy().replace('\\', "/"))
        })
        .collect();
    for dep in firstparty::first_party_npm_for(modules.iter().map(String::as_str)) {
        npm_deps.insert(dep.package.to_string(), dep.version.to_string());
    }
    (runtime_files, npm_deps)
}

/// 1C.3a (#4040): the model a Workers build's files were written from, for the
/// commands that act on its bindings; none for a Bundle build.
fn workers_build(
    target: BuildTarget,
    bindings: emitter::WorkerBindings,
    layout: &WorkerLayout,
) -> Option<WorkersBuild> {
    matches!(target, BuildTarget::Workers).then(|| WorkersBuild {
        bindings,
        layout: layout.clone(),
    })
}

/// v0.6 §6.3: emit a composition root when the project has at least one
/// context that consumes another context's service surface. The compose file
/// imports each context, instantiates its providers, assembles its deps
/// (capabilities + cross-context surfaces), and exports the top-level service
/// surface.
#[allow(clippy::too_many_arguments)]
fn emit_bundle_compose_root(
    compiled: &mut Vec<StagedFile>,
    plan: &emitter::ComposePlan,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_tables: &HashMap<String, UnitTable>,
    adapter_bindings: &HashMap<String, AdapterBinding>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    thread_env: bool,
) {
    if let Some(compose_program) = emitter::emit_composition_root(
        plan,
        unit_consumes,
        unit_consumes_aliases,
        unit_tables,
        adapter_bindings,
        unit_flattened,
        thread_env,
    ) {
        compiled.push(StagedFile {
            output_path: PathBuf::from("compose.ts"),
            document: Document::Ts(compose_program),
            source_map: None,
            debug_metadata: None,
        });
    }
}

/// Build-success tail (region 3): emit the composition/worker/runtime files
/// and assemble the final `ProjectOutput`. Reached only on build mode with a
/// clean error sink. Moved verbatim from the old pipeline; only the locals it
/// reads are now bound from the `Checked` variant.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_output(
    parsed: Vec<ParsedFile>,
    mut compiled: Vec<StagedFile>,
    mut runnable_tests: Vec<RunnableTest>,
    integration_outputs: Vec<StagedFile>,
    integration_runnables: Vec<RunnableTest>,
    groups: BTreeMap<String, Vec<usize>>,
    kinds: BTreeMap<String, UnitKind>,
    unit_consumes: HashMap<String, Vec<String>>,
    unit_consumes_aliases: HashMap<String, HashMap<String, String>>,
    unit_tables: HashMap<String, UnitTable>,
    unit_consumed_context_types: &HashMap<String, HashSet<String>>,
    unit_callees: HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
    // v0.177 (#643): needed to build each context's *own* combined type table,
    // so its contract hashes are computed from the same namespace a caller sees.
    unit_uses: HashMap<String, Vec<String>>,
    unit_flattened: HashMap<String, HashMap<String, String>>,
    adapter_bindings: HashMap<String, AdapterBinding>,
    npm_deps: std::collections::BTreeMap<String, String>,
    target: BuildTarget,
    import_ext: ImportExt,
    // #1890: the merged `wrangler.toml` flag list (`finish_build` merges it).
    compatibility_flags: &[String],
    workers_settings: emitter::workers_emit::WorkersSettings,
    // #3477: the path back to the project root from a Worker's output
    // directory, for a container's build paths.
    container_root: &str,
    // #1883, slice 1b: the declared asset files' bytes; each Worker ships
    // the files its `uses` closure declares.
    asset_files: Option<&BTreeMap<String, Vec<u8>>>,
    // State-migrations S4 (#2259): each agent class's lock version, keyed
    // `<unit>.<Agent>`, for its Worker's `/_bynk/migrate/` route.
    agent_versions: &HashMap<String, bynk_check::schema_registry::AgentVersions>,
    // #2554: the older contracts each callee accepts, from `bynk.contracts.lock`.
    contracts_lock: &bynk_project::contracts_lock::ContractsLock,
    // P4b (#2608): each `D1` database's migrations.
    d1_migrations: &bynk_check::d1_migrations::D1Migrations,
    // #3562: where each context's Worker is.
    layout: &WorkerLayout,
    // #3580: wrap each Worker's `fetch` in the request recorder.
    trace_requests: bool,
    inspect_agents: bool,
    // 1C.1a: the plan and binding model `run_checks` derived.
    composed: ComposedBuild,
) -> ProjectOutput {
    // #2554: the lock's warnings, returned on `ProjectOutput::warnings` for
    // `finish_build` to merge with the checker's.
    let mut lock_warnings: Vec<AttributedError> = Vec::new();
    // #1883, slice 1b: each declared asset's served path, and the files the
    // Workers below stage into their assets directories (empty for a Bundle
    // build: only a Worker serves files).
    let asset_served = asset_files
        .map(|files| bynk_check::assets::declared_served_paths(&parsed, files))
        .unwrap_or_default();
    let mut staged_assets: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();

    // #1655 (runtime-semantics track S6): a `--target workers` build writes the
    // workers layout (`workers/<ctx>/…`), but a *unit* test module imports the
    // bundle layout (`./../<ctx>.js`), so `tsc` over the output failed
    // (TS2307). Those modules had no consumer either: `bynk test` compiles the
    // bundle target for them and overlays a workers compile *excluding*
    // `tests/`. A workers build therefore drops the unit modules. Integration
    // modules stand their participants up as real Workers and import this
    // layout, so they stay, and `tests/main.ts` runs just them.
    let workers = target == BuildTarget::Workers;
    if workers {
        compiled.retain(|f| !f.output_path.starts_with("tests"));
    }
    compiled.extend(integration_outputs);
    // #2695: the entry handlers of the project's own contexts (first-party
    // injected units are the toolchain's, not the user's), described from the
    // checked symbol tables before they are moved into emission.
    let entry_handlers: Vec<bynk_model::entry::EntryHandler> = groups
        .iter()
        .filter(|(unit, idxs)| {
            kinds.get(*unit) == Some(&UnitKind::Context)
                && idxs.iter().any(|i| !parsed[*i].is_synthetic())
        })
        .flat_map(|(unit, _)| bynk_model::entry::entry_handlers(unit, &unit_tables, &unit_uses))
        .collect();
    // The integration runnables follow the unit ones.
    let unit_count = runnable_tests.len();
    runnable_tests.extend(integration_runnables);

    // v0.67: the discovery manifest — built from the combined runnable set before
    // anything consumes it, so `--no-run --format json` lists suites/cases without
    // running. Ordered by the runner's sort key to match a run's suite order.
    // On `workers` it lists only what this build writes and `tests/main.ts` runs
    // (#1655, review of #1695): the integration suites. `bynk test` reads the
    // manifest from its bundle compile, so the unit suites are still listed
    // there.
    let discovered = if workers {
        discovery_manifest(&runnable_tests[unit_count..])
    } else {
        discovery_manifest(&runnable_tests)
    };

    // v0.16: emit the combined top-level test runner once both passes are done,
    // so `tests/main.ts` aggregates unit and integration suites together (on
    // `workers`, the integration suites only: #1655, above).
    let main_tests = if workers {
        &runnable_tests[unit_count..]
    } else {
        &runnable_tests[..]
    };
    if !main_tests.is_empty() {
        let main_program = emit_test_main(main_tests, import_ext, pglite_on(d1_migrations));
        compiled.push(StagedFile {
            output_path: PathBuf::from("tests/main.ts"),
            document: Document::Ts(main_program),
            source_map: None,
            debug_metadata: None,
        });
    }

    // v0.19 (decision 0025): does any context's in-process closure reach a
    // platform-native unit? Drives env threading (bundle) and the per-Worker
    // Env/`wrangler.toml` resource derivation (workers).
    let context_native = worker_bindings::contexts_native(
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_flattened,
    );

    // Events track, slice 0 (spine #936): project-wide "who subscribes to
    // what", and 1B.1b (#3928) what each unit composes, both decided once in
    // `run_checks` for both targets; 1C.1a (#4031) adds each Worker's bindings,
    // with a container's build path from the project root set once (#3477).
    let (compose_plan, mut bindings) = (composed.plan, composed.bindings);
    bindings.locate_containers(container_root);
    // #2137: the contexts a `system` harness stands up (test build only).
    let harness_clock_contexts =
        harness_clock_contexts(&parsed, &unit_tables, &unit_consumes, &kinds);

    match target {
        BuildTarget::Bundle => emit_bundle_compose_root(
            &mut compiled,
            &compose_plan,
            &unit_consumes,
            &unit_consumes_aliases,
            &unit_tables,
            &adapter_bindings,
            &unit_flattened,
            // D1: thread `env` through composeApp only when a native
            // resource is consumed, so native-free programs are
            // byte-identical to v0.18 output.
            !context_native.is_empty(),
        ),
        BuildTarget::Workers => {
            // v0.8 §2.3: per-Worker entry point, compose.ts, and wrangler
            // configuration. One Worker per context.
            let emit = emitter::workers_emit::WorkersEmit {
                adapter_bindings: &adapter_bindings,
                agent_versions,
                asset_files,
                asset_served: &asset_served,
                bindings: &bindings,
                compatibility_flags,
                compose_plan: &compose_plan,
                contracts_lock,
                d1_migrations,
                groups: &groups,
                harness_clock_contexts: &harness_clock_contexts,
                import_ext,
                inspect_agents,
                kinds: &kinds,
                layout,
                parsed: &parsed,
                trace_requests,
                unit_callees: &unit_callees,
                unit_consumed_context_types,
                unit_consumes: &unit_consumes,
                unit_consumes_aliases: &unit_consumes_aliases,
                unit_flattened: &unit_flattened,
                unit_tables: &unit_tables,
                unit_uses: &unit_uses,
                workers: workers_settings,
            };
            emit.emit_all(&mut compiled, &mut lock_warnings, &mut staged_assets);
        }
    }
    let workers = workers_build(target, bindings, layout);

    // v0.17: copy each adapter binding verbatim into the output, beside the
    // adapter's emitted interface module, so compose's import resolves and the
    // `tsc` gate checks the `implements` contract.
    let mut binding_names: Vec<&String> = adapter_bindings.keys().collect();
    binding_names.sort();
    for name in binding_names {
        let b = &adapter_bindings[name];
        compiled.push(StagedFile {
            output_path: b.output_path.clone(),
            document: Document::Ts(bynk_ts::TsProgram {
                stmts: vec![bynk_ts::TsStmt::verbatim(
                    bynk_ts::VerbatimOrigin::NotYetConverted,
                    emitter::runtime_files::rewrite_runtime_imports(&b.content),
                    None,
                )],
            }),
            source_map: None,
            debug_metadata: None,
        });
    }

    // #2321: each included WebAssembly module — its bytes beside the binding
    // (a module file, never served) and the loader shim the binding imports.
    let mut staged_modules: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    for b in adapter_bindings.values() {
        for inc in &b.includes {
            staged_modules.insert(inc.output_path.clone(), inc.bytes.clone());
            compiled.push(StagedFile {
                output_path: wasm_shim_path(&inc.output_path),
                document: Document::Ts(bynk_ts::TsProgram {
                    stmts: vec![bynk_ts::TsStmt::verbatim(
                        bynk_ts::VerbatimOrigin::NotYetConverted,
                        workers_wasm_shim(&inc.output_path),
                        None,
                    )],
                }),
                source_map: None,
                debug_metadata: None,
            });
        }
    }
    if !staged_modules.is_empty() {
        compiled.push(StagedFile {
            output_path: PathBuf::from("wasm-modules.d.ts"),
            document: Document::Ts(bynk_ts::TsProgram {
                stmts: vec![bynk_ts::TsStmt::verbatim(
                    bynk_ts::VerbatimOrigin::NotYetConverted,
                    WASM_MODULES_DTS.to_string(),
                    None,
                )],
            }),
            source_map: None,
            debug_metadata: None,
        });
    }

    // Runtime (ADR 0615): exactly the modules the emitted files import, closed
    // over the modules' own imports, under `__bynk_runtime/` at the root of
    // `out/` so every file's relative specifier resolves to it.
    // `tsconfig.json` is also at the root so `tsc -p out/tsconfig.json`
    // discovers every `.ts` file in the tree.
    let mut reached = BTreeSet::new();
    for f in &compiled {
        if let Document::Ts(program) = &f.document {
            emitter::runtime_files::reached_in(program, &mut reached);
        }
    }
    let (runtime_files, npm_deps) = runtime_with_npm(&reached, npm_deps);
    for (path, source) in runtime_files {
        compiled.push(StagedFile {
            output_path: path,
            document: Document::Ts(bynk_ts::TsProgram {
                stmts: vec![bynk_ts::TsStmt::verbatim(
                    bynk_ts::VerbatimOrigin::NotYetConverted,
                    source,
                    None,
                )],
            }),
            source_map: None,
            debug_metadata: None,
        });
    }
    compiled.push(package_json_file(&npm_deps));
    compiled.push(StagedFile {
        output_path: PathBuf::from("tsconfig.json"),
        document: Document::Json(emitter::emit_tsconfig()),
        source_map: None,
        debug_metadata: None,
    });

    // `Artefacts.docs` is a `BTreeMap`, so it iterates in `output_path` order
    // naturally (Decision D, #1309) — no explicit sort needed here the way
    // the old `Vec<CompiledFile>` (sorted by `source_path`) required.
    let mut docs: BTreeMap<PathBuf, Document> = BTreeMap::new();
    // #2554: lock entries for a context the project no longer has.
    for (context, services) in contracts_lock.contexts() {
        if kinds.get(context) != Some(&UnitKind::Context) {
            for service in services.keys() {
                lock_warnings.push(bynk_project::contracts_lock::stale_warning(
                    context,
                    service,
                    "the context is no longer in the project",
                ));
            }
        }
    }

    for f in compiled {
        if let Some(sm) = &f.source_map {
            docs.insert(
                sibling_path(&f.output_path, "map"),
                Document::SourceMap(sm.clone()),
            );
        }
        if let Some(dbg) = &f.debug_metadata {
            docs.insert(
                sibling_path(&f.output_path, "bynkdbg.json"),
                Document::DebugSidecar(dbg.clone()),
            );
        }
        docs.insert(f.output_path, f.document);
    }
    ProjectOutput {
        artefacts: Artefacts {
            docs,
            assets: staged_assets,
            modules: staged_modules,
        },
        discovered,
        snapshot_files: runnable_tests
            .iter()
            .flat_map(|t| t.snapshot_files.iter().cloned())
            .collect(),
        // Populated by `compile_project` from the run's warning sink (ADR 0117);
        // until then, the contracts lock's own (#2554).
        warnings: lock_warnings,
        // Populated by `finish_build` from the same `RunChecks::Checked` this
        // whole `ProjectOutput` was built from.
        snapshots: Vec::new(),
        display_root: PathBuf::new(),
        // Likewise (#1078) — `Some` only when the registry was on.
        schema_lock: None,
        entry_handlers,
        workers,
    }
}

// P5.3 review (#1133): `resolve_consume_prefix` and `handler_cross_caps` used
// to have their own copies here, byte-identical to `bynk-check::project_model`'s
// (neither builds TypeScript, so neither had the codegen coupling that keeps
// `instantiate_provider_ts_expr` in `bynk-emit`) — deleted, every call site repointed at
// `project_model::{resolve_consume_prefix, handler_cross_caps}`.
