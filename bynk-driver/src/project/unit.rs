use super::agent_deps::AgentDepsPlan;
use super::diagnostics::ErrorSink;
use super::diagnostics::Mode;
use bynk_check::check_pipeline::{self, prepare_unit_check_ctx};
use bynk_check::checker;
use bynk_check::checker::{ExprId, Types};
use bynk_check::expr_types::ExprTypeSink;
use bynk_check::firstparty::FirstPartyCap;
use bynk_check::hints::HintSink;
use bynk_check::index::RefSink;
use bynk_check::locals::LocalsSink;
use bynk_check::project_model::BuildTarget;
use bynk_check::project_model::{self, FnDecl, TypeDecl, UnitInfo, Visibility};
use bynk_check::requirements::RequirementSink;
use bynk_check::resolver::{self, MethodTable as ResolverMethodTable};
use bynk_check::symbols::ConsumedType;
use bynk_check::symbols::FileDeclIndex;
use bynk_emit::emit_model::Document;
use bynk_emit::emit_model::EmitProjectCtx;
use bynk_emit::emit_model::ExtraImport;
use bynk_emit::emit_model::ImportExt;
use bynk_emit::emit_model::SchemaVersions;
use bynk_emit::emit_model::StagedFile;
use bynk_emit::emit_model::UnitSource;
use bynk_emit::emitter;
use bynk_model::EventSubscriberShape;
use bynk_model::FnSig;
use bynk_model::{lower_attached_fn_sig_ir_from_types, lower_handler_given_ir};
use bynk_project::UnitKind;
use bynk_project::discovery::ParsedFile;
use bynk_project::paths::ts_output_path;
use bynk_project::worker_layout::WorkerLayout;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Phase 8e: build the emitter context for one checked source file and render
/// its TypeScript, pushing the result onto `compiled`. Reached only in build
/// mode (the caller's analyse-mode `continue` gates this off); the block is
/// straight-line with no `continue`s of its own.
#[allow(clippy::too_many_arguments)]
/// Emit-prologue tables that depend only on the *unit* (`name`/`unit_info`/
/// `target`) — never on which file within the unit is being emitted. Building
/// one of these once per unit, ahead of the per-file loop, replaces what used
/// to be an identical rebuild (several nested nested loops over `unit_info`)
/// on every emitted file of a multi-file context.
pub(super) struct EmitUnitCtx {
    imported_methods: HashMap<String, Vec<FnSig>>,
    /// The workers-mode-rewritten view is the only one `emit_unit` reads —
    /// the pre-rewrite table is an intermediate of computing it, not exposed
    /// separately.
    imported_decl_paths_emit: HashMap<String, HashMap<String, PathBuf>>,
    exports_for_consumed: HashMap<String, HashMap<String, Visibility>>,
    file_decl_index: FileDeclIndex,
    /// #1820: a context split across files, built for Workers. Each file is
    /// emitted as its own module ([`worker_file_source_path`](bynk_project::paths::worker_file_source_path)) and
    /// `handlers.ts` re-exports them ([`emit_worker_handlers_barrel`]), rather
    /// than every file being written to `handlers.ts` in turn, each
    /// overwriting the last. `file_decl_index` then names each declaration's
    /// module, so one file imports what it uses from another.
    split_worker: bool,
    /// #3562: where each context's files go.
    layout: WorkerLayout,
}

fn build_emit_unit_ctx(
    name: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
    hidden_types: &BTreeMap<String, String>,
    target: BuildTarget,
    tys: &Arc<Types>,
    split_worker: bool,
    layout: &WorkerLayout,
) -> EmitUnitCtx {
    let info = &unit_info[name];
    // v0.132.1 (#481): gather the attached methods of every `uses`-imported type
    // (one level, matching the symbol-table merge). `emit_context_rebrands`
    // forwards these onto the consumer's rebranded const so a call like
    // `Cents.fromInt(n)` type-checks. Sorted by method name for deterministic
    // emission (the resolver stores instance/static methods in `HashMap`s).
    //
    // P6.18: each method's own `params`/`return_type` now resolve to a real
    // `TyId` (`bynk_model::lower_attached_fn_sig_ir_from_types`) against the
    // *declaring* unit's own visible types, rather than carrying the raw
    // `FnDecl` (and its unresolved `TypeRef`s) all the way to
    // `emit_forwarded_methods`. `Free`-named entries (never present in
    // practice — `ResolverMethodTable` only ever collects attached methods,
    // `bynk-check/src/resolver.rs:47`) are skipped, matching
    // `emit_forwarded_methods`'s own pre-existing `FnName::Method` filter
    // one step earlier rather than lowering a signature nothing renders.
    let mut imported_methods: HashMap<String, Vec<FnSig>> = HashMap::new();
    for t in &info.uses {
        let Some(used) = unit_info.get(t) else {
            continue;
        };
        let used_types = bynk_check::symbols::combined_types_for_unit_info(t, unit_info);
        // #2751: only what an importer can reach — a `private` type's methods
        // (and a private method) would otherwise merge into a same-named
        // public type's forwarded const.
        for (type_name, mt) in &used.table.importable().methods {
            let entry = imported_methods.entry(type_name.clone()).or_default();
            entry.extend(lower_attached_fn_sig_ir_from_types(mt, &used_types, tys));
        }
    }
    // #1807: a hidden type's methods are forwarded like a `uses` type's,
    // resolved in its owning commons' scope.
    for (type_name, owner) in hidden_types {
        let Some(mt) = unit_info
            .get(owner)
            .and_then(|o| o.table.methods.get(type_name))
        else {
            continue;
        };
        let owner_types = bynk_check::symbols::combined_types_for_unit_info(owner, unit_info);
        imported_methods
            .entry(type_name.clone())
            .or_default()
            .extend(lower_attached_fn_sig_ir_from_types(mt, &owner_types, tys));
    }
    for decls in imported_methods.values_mut() {
        decls.sort_by_key(|sig| sig.name.clone());
    }
    let mut imported_decl_paths: HashMap<String, HashMap<String, PathBuf>> = HashMap::new();
    for t in &info.uses {
        if let Some(target_info) = unit_info.get(t) {
            let target_index = &target_info.file_index;
            let mut paths: HashMap<String, PathBuf> = HashMap::new();
            for (n, p) in &target_index.types {
                paths.insert(n.clone(), p.clone());
            }
            for (n, p) in &target_index.fns {
                paths.insert(n.clone(), p.clone());
            }
            imported_decl_paths.insert(t.clone(), paths);
        }
    }
    for t in &info.consumes {
        if let Some(target_info) = unit_info.get(t) {
            let target_index = &target_info.file_index;
            let mut paths: HashMap<String, PathBuf> = HashMap::new();
            // Only expose exported names — the emitter needs to know
            // which file declares them so it can render the import.
            let exports_for_target = &target_info.exports;
            for n in exports_for_target.keys() {
                if let Some(p) = target_index.types.get(n) {
                    paths.insert(n.clone(), p.clone());
                }
            }
            // #2887: an aliased export is imported by its original name, from
            // the file that declares it, but referred to by its alias.
            for (alias, a) in &info.table.consumed_type_aliases {
                if a.target == *t
                    && let Some(p) = target_index.types.get(&a.original)
                {
                    paths.insert(alias.clone(), p.clone());
                }
            }
            imported_decl_paths.insert(t.clone(), paths);
        }
    }
    // #1807: a hidden type is imported from its owning commons, which this
    // unit need not `uses` itself.
    for (type_name, owner) in hidden_types {
        if let Some(p) = unit_info
            .get(owner)
            .and_then(|o| o.file_index.types.get(type_name))
        {
            imported_decl_paths
                .entry(owner.clone())
                .or_default()
                .insert(type_name.clone(), p.clone());
        }
    }

    let exports_for_consumed = info
        .consumes
        .iter()
        .map(|t| {
            let mut exports = unit_info
                .get(t)
                .map(|i| i.exports.clone())
                .unwrap_or_default();
            // #2887: a renamed export is exported under its alias, here.
            for (alias, a) in &info.table.consumed_type_aliases {
                if a.target == *t
                    && let Some(vis) = exports.get(&a.original).copied()
                {
                    exports.insert(alias.clone(), vis);
                }
            }
            (t.clone(), exports)
        })
        .collect();

    // In workers mode, rewrite imported_decl_paths for consumed
    // contexts to point at the consumed Worker's handlers.ts.
    let mut imported_decl_paths_emit = imported_decl_paths.clone();
    if matches!(target, BuildTarget::Workers) {
        for (unit, decls) in imported_decl_paths.iter() {
            let target_kind = unit_info.get(unit).map(|i| i.kind);
            if target_kind == Some(UnitKind::Context) {
                let handlers_path = layout.handlers_source_path(unit);
                let mut rewritten = HashMap::new();
                for n in decls.keys() {
                    rewritten.insert(n.clone(), handlers_path.clone());
                }
                imported_decl_paths_emit.insert(unit.clone(), rewritten);
            }
        }
    }

    let mut file_decl_index = info.file_index.clone();
    if split_worker {
        let module = |p: &mut PathBuf| *p = layout.file_source_path(name, p);
        file_decl_index.types.values_mut().for_each(module);
        file_decl_index.fns.values_mut().for_each(module);
        file_decl_index.agents.values_mut().for_each(module);
        file_decl_index.capabilities.values_mut().for_each(module);
        file_decl_index
            .methods
            .values_mut()
            .flat_map(HashMap::values_mut)
            .for_each(module);
    }

    EmitUnitCtx {
        imported_methods,
        imported_decl_paths_emit,
        exports_for_consumed,
        file_decl_index,
        split_worker,
        layout: layout.clone(),
    }
}

/// #2991: whether any source file of the project mentions `Json.encodePlain`.
/// Textual on purpose: it only decides whether the generated `Option` encoders
/// carry their (inert unless raised) plain branch, so a mention in a comment
/// costs a few bytes and nothing else.
pub(crate) fn project_mentions_encode_plain(parsed: &[ParsedFile]) -> bool {
    parsed.iter().any(|p| p.source().contains("encodePlain"))
}

/// #1927: a `uses`-imported type this unit hides behind its own value is spelled as an inline
/// import type while the module is emitted (`EmitProjectCtx::type_spellings`).
fn hidden_type_spellings(
    name: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
    unit_ctx: &EmitUnitCtx,
    source_path: &Path,
    import_ext: ImportExt,
) -> HashMap<String, String> {
    bynk_check::project_model::shadowed_imports(name, unit_info)
        .into_iter()
        .filter(|(_, s)| s.imported == "type")
        .map(|(type_name, s)| {
            let path = unit_ctx
                .imported_decl_paths_emit
                .get(&s.unit)
                .and_then(|m| m.get(&type_name))
                .cloned()
                .unwrap_or_else(|| EmitProjectCtx::commons_path(&s.unit));
            let spec =
                emitter::cross_commons_import_specifier_for_path(source_path, &path, import_ext);
            let spelling = format!("import(\"{spec}\").{type_name}");
            (type_name, spelling)
        })
        .collect()
}

/// 1A.34: emit the unit's module. A checker fact the emitter needed and did not find is a
/// compiler fault, reported as a `bynk.internal.missing_checker_fact` diagnostic at the `.bynk`
/// span of each (see [`report_internal_faults`]), and the unit's output is dropped (`None`).
fn emit_unit_program(
    program: &checker::CheckedProgram,
    emit_ctx: &EmitProjectCtx,
    pf: &ParsedFile,
    source_name: &str,
    errors: &mut ErrorSink,
) -> Option<(bynk_ts::TsProgram, Option<String>)> {
    match emitter::emit_project(program, emit_ctx, pf.source(), source_name) {
        Ok(emitted) => Some(emitted),
        Err(faults) => {
            report_internal_faults(errors, &pf.identity_path(), faults);
            None
        }
    }
}

/// 1A.34: turn the checker facts the emitter needed and did not find into
/// `bynk.internal.missing_checker_fact` diagnostics, one per fault, each at the
/// `.bynk` span of the construct being lowered and attributed to `file`.
pub(super) fn report_internal_faults(
    errors: &mut ErrorSink,
    file: &Path,
    faults: Vec<emitter::InternalFault>,
) {
    errors.extend_for(
        Some(file),
        faults.into_iter().map(|f| f.into_compile_error()),
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_unit(
    name: &str,
    kind: UnitKind,
    pf: &ParsedFile,
    unit_ctx: &EmitUnitCtx,
    history_target_agents: &HashSet<String>,
    asset_served: &BTreeMap<String, String>,
    embed_text: &BTreeMap<String, String>,
    unit_info: &BTreeMap<String, UnitInfo>,
    imported_from: &HashMap<String, String>,
    imported_from_kind: &HashMap<String, UnitKind>,
    owning_context_for_emit: &Option<String>,
    cross_context_for_file: &resolver::CrossContextInfo,
    program: &checker::CheckedProgram,
    target: BuildTarget,
    import_ext: ImportExt,
    contracts: bool,
    trace_requests: bool,
    inspect_agents: bool,
    agent_deps_plan: Option<&AgentDepsPlan>,
    compiled: &mut Vec<StagedFile>,
    errors: &mut ErrorSink,
    schema_effective_versions: &SchemaVersions,
    plain_encode_in_project: bool,
) {
    let typed = program.program();
    // Build the emitter context.
    let info = &unit_info[name];
    let mut cross_context_info = cross_context_for_file.clone();
    // #1932: the names the consumer gives a consumed context's unexported
    // types' codecs, for the call sites, from the same function its codec
    // generator uses.
    for c in cross_context_info.consumed_contexts.clone() {
        if let Some(types) = cross_context_info.consumed_types.get(&c) {
            let renames = emitter::unexported_type_renames(
                &c,
                types,
                unit_ctx.exports_for_consumed.get(&c),
                &typed.types,
                imported_from_kind,
            );
            cross_context_info.consumed_type_renames.insert(c, renames);
        }
    }

    // v0.8: in workers mode, a context's *output* lands under
    // workers/<dashes>/handlers.ts. Use that path as the synthetic
    // source_path so the emitter's depth/relative-path logic and
    // imported_decl_paths produce correct relative imports.
    let workers_mode = matches!(target, BuildTarget::Workers);
    let emit_source_path = if unit_ctx.split_worker {
        unit_ctx.layout.file_source_path(name, &pf.source_path())
    } else if workers_mode && kind == UnitKind::Context {
        unit_ctx.layout.handlers_source_path(name)
    } else {
        pf.source_path()
    };

    // message-bundles slice 1 (#859): a `messages` block's generated `render`
    // needs `bynk.locale`'s own `render` in scope for its fallback rung, but
    // under a private alias — this file's own `export function render` would
    // otherwise collide with a plain `import { render }`. Injected as a hand-
    // written extra import line rather than through the usual reference-
    // collection path (`collect_external_references`/`record_name_ref`),
    // which has no per-name aliasing of its own and would emit a colliding,
    // unaliased `render`.
    // #1820: in a split context, only a file declaring one of the plan's
    // agents builds its deps, so only that file imports what they need.
    let mut extra_imports: Vec<ExtraImport> = agent_deps_plan
        .filter(|p| {
            !unit_ctx.split_worker
                || p.exprs.keys().any(|agent| {
                    unit_ctx.file_decl_index.agents.get(agent) == Some(&emit_source_path)
                })
        })
        .map(|p| p.imports.clone())
        .unwrap_or_default();
    if pf.declares_messages() {
        let render_path = unit_ctx
            .imported_decl_paths_emit
            .get("bynk.locale")
            .and_then(|m| m.get("render"))
            .cloned()
            .unwrap_or_else(|| EmitProjectCtx::commons_path("bynk.locale"));
        let import = emitter::cross_commons_import_specifier_for_path(
            &emit_source_path,
            &render_path,
            import_ext,
        );
        // #1697: the generated table and `render` name the `bynk.locale.types`
        // types under private aliases too, so a user's own `Message` or
        // `LocaleTag` in this unit (local shadows `uses`) can't capture them.
        let types_path = unit_ctx
            .imported_decl_paths_emit
            .get("bynk.locale.types")
            .and_then(|m| m.get("LocaleTag"))
            .cloned()
            .unwrap_or_else(|| EmitProjectCtx::commons_path("bynk.locale.types"));
        let types_import = emitter::cross_commons_import_specifier_for_path(
            &emit_source_path,
            &types_path,
            import_ext,
        );
        extra_imports.push(ExtraImport::Named {
            names: vec![
                "render as __bynkLocaleRender".to_string(),
                "renderArg as __bynkRenderArg".to_string(),
            ],
            from: import,
            type_only: false,
        });
        extra_imports.push(ExtraImport::Named {
            names: vec![
                "LocaleTag as __bynkLocaleTag".to_string(),
                "Message as __bynkMessage".to_string(),
                "MessageArg as __bynkMessageArg".to_string(),
            ],
            from: types_import,
            type_only: true,
        });
    }

    let agent_called_contexts: Vec<String> =
        emitter::agent_cross_context_targets(&info.table, |c| cross_context_info.resolve_prefix(c))
            .into_iter()
            .collect();
    // #4045: a qualifier a `uses` clause introduces (`Y`, `lib.y`) is imported where
    // the unit reads a name through it.
    for (qualifier, used) in use_qualifiers(&info.table) {
        let path = unit_ctx
            .imported_decl_paths_emit
            .get(&used)
            .and_then(|m| m.values().min().cloned())
            .unwrap_or_else(|| EmitProjectCtx::commons_path(&used));
        extra_imports.push(ExtraImport::Qualifier {
            qualifier,
            from: emitter::cross_commons_import_specifier_for_path(
                &emit_source_path,
                &path,
                import_ext,
            ),
        });
    }
    let mut emit_ctx = EmitProjectCtx {
        source_path: emit_source_path,
        identity_path: pf.identity_path().to_string_lossy().into_owned(),
        commons_name: name.to_string(),
        file_decl_index: unit_ctx.file_decl_index.clone(),
        imported_from: imported_from.clone(),
        imported_from_kind: imported_from_kind.clone(),
        imported_decl_paths: unit_ctx.imported_decl_paths_emit.clone(),
        consumed_type_aliases: info
            .table
            .consumed_type_aliases
            .iter()
            .map(|(alias, a)| (alias.clone(), a.original.clone()))
            // #4045: a name a `uses` clause renames is imported the same way, under
            // the name its commons declares and bound to the spelling used here.
            .chain(
                info.table
                    .use_views
                    .values()
                    .flat_map(|v| v.use_origin.iter().map(|(k, o)| (k.clone(), o.clone()))),
            )
            .collect(),
        unit_kind: kind,
        owning_context: owning_context_for_emit.clone(),
        exports_for_consumed: unit_ctx.exports_for_consumed.clone(),
        imported_methods: unit_ctx.imported_methods.clone(),
        cross_context: cross_context_info,
        target,
        local_agents: info.table.agents.keys().cloned().collect(),
        agent_given_deps: agent_deps_plan.map(|p| p.exprs.clone()).unwrap_or_default(),
        agent_given_deps_names: agent_deps_plan.map(|p| p.names.clone()).unwrap_or_default(),
        extra_imports,
        agent_method_givens: info
            .table
            .agents
            .iter()
            .map(|(agent, a)| {
                (
                    agent.clone(),
                    a.handlers
                        .iter()
                        .filter_map(|h| {
                            h.method_name
                                .as_ref()
                                .map(|m| (m.name.clone(), lower_handler_given_ir(h)))
                        })
                        .collect(),
                )
            })
            .collect(),
        agent_called_contexts,
        // v0.47: the context's actors (merged across files), so the Bearer
        // verification seam resolves even when the actor and handler are in
        // different files of the same context.
        actors: info.table.actors.clone(),
        // Events slice 3b (#978), verified by slice 3c (#980): resolved once
        // per unit, merged across files the same way `actors` is above. The
        // registry's reconciled version wins when present (it is the
        // auto-bumped or `@schema(N)`-verified truth); `decl.schema_version()`
        // is the fallback for when the registry is off, matching every
        // event's pre-3c behaviour exactly.
        agent_schema_versions: info
            .table
            .agents
            .keys()
            .filter_map(|agent_name| {
                schema_effective_versions
                    .agents
                    .get(&format!("{name}.{agent_name}"))
                    .map(|v| (agent_name.clone(), v.clone()))
            })
            .collect(),
        event_schema_versions: info
            .table
            .events
            .iter()
            .map(|(event_name, decl)| {
                let key = format!("{name}.{event_name}");
                let version = schema_effective_versions
                    .events
                    .get(&key)
                    .copied()
                    .unwrap_or_else(|| decl.schema_version());
                (event_name.clone(), version)
            })
            .collect(),
        consumed_adapters: info
            .consumes
            .iter()
            .filter(|t| unit_info.get(*t).map(|i| i.kind) == Some(UnitKind::Adapter))
            .cloned()
            .collect(),
        consumed_types_with_methods: info
            .consumes
            .iter()
            .filter_map(|t| unit_info.get(t))
            .filter(|i| i.kind == UnitKind::Context)
            .flat_map(|i| {
                i.table
                    .methods
                    .iter()
                    .filter(|(_, m)| !m.instance.is_empty() || !m.statics.is_empty())
                    .map(|(n, _)| n.clone())
            })
            .collect(),
        import_ext,
        contracts,
        // #3580: the file's source and project-relative path name each traced call
        // site; `None` (every build but `bynk dev --trace-events`) records nothing.
        trace: trace_requests.then(|| bynk_emit::emit_model::TraceLoc {
            source: std::sync::Arc::from(pf.source()),
            rel_path: pf.identity_path().to_string_lossy().into_owned(),
        }),
        inspect_agents,
        history_target_agents: history_target_agents.clone(),
        asset_served: asset_served.clone(),
        embed_text: embed_text.clone(),
        runtime_use: Default::default(),
        worker_layout: unit_ctx.layout.clone(),
        // #2620: the unit's own source, so a fault lowering names `path:line:col`.
        unit_source: Some(UnitSource::new(
            pf.source(),
            &pf.identity_path().to_string_lossy(),
        )),
        // #1927: filled below, once the import specifiers can be built from
        // `source_path`.
        type_spellings: HashMap::new(),
    };
    // #2858: any plain-JSON route in the project (project-wide, not per Worker;
    // the branch is inert unless `withPlainJson` sets the flag) switches the generated `Option`
    // encoders to carry the plain branch (a response type can live in a commons
    // another unit's route serialises through).
    // #2991: so does any `Json.encodePlain` call in the project's sources (a
    // textual over-approximation: the branch is inert unless a call raises the flag).
    emit_ctx.runtime_use.set_plain_json(
        plain_encode_in_project
            || unit_info
                .values()
                .any(|i| emitter::table_has_plain_json(&i.table)),
    );
    // v0.72: the map's `source` is the absolute path the compiler read the file
    // from, so an editor breakpoint set on the real `.bynk` resolves to the same
    // path the debugger loads (project-relative would resolve against the output
    // `.ts`'s directory — the wrong place). Synthetic units fall back to relative.
    let source_name = pf.map_source_name();
    // #1486: `emit_project` now returns the real `TsProgram` (printed once
    // at the write boundary via `Document::text`) plus the source map
    // `bynk_ts::print` already computed while building it — no more
    // `Verbatim`-wrapping a pre-rendered string.
    emit_ctx.type_spellings =
        hidden_type_spellings(name, unit_info, unit_ctx, &emit_ctx.source_path, import_ext);
    let Some((program, source_map)) =
        emit_unit_program(program, &emit_ctx, pf, &source_name, errors)
    else {
        return;
    };
    // Slice 3: the handler-label sidecar for this unit (ADR 0105) — names stack
    // frames by their Bynk operation. `None` for units with no handlers.
    let debug_metadata = emitter::collect_handler_labels(typed);
    let output_path = if unit_ctx.split_worker {
        ts_output_path(&unit_ctx.layout.file_source_path(name, &pf.source_path()))
    } else if workers_mode && kind == UnitKind::Context {
        unit_ctx.layout.handlers_output_path(name)
    } else {
        ts_output_path(&pf.source_path())
    };
    compiled.push(StagedFile {
        output_path,
        document: Document::Ts(program),
        source_map,
        debug_metadata,
    });
}

/// Phase 8d/8e: resolve + check (and, in build mode, emit) every source file in
/// one production unit. The per-file `continue`s stay internal to this loop, so
/// a file that fails resolution/checking is skipped without abandoning the unit.
///
/// P4.1 (#1115): the resolve+check+context-checks core — identical for both
/// `Mode`s except for the four `record_analyse_types` call sites — moved to
/// `bynk_check::check_pipeline::check_file_core` (see that module's own doc
/// comment), used by both this function and the new `bynk-check`-native
/// analysis entry point. This function now owns only: the per-unit
/// `EmitUnitCtx`/`prepare_unit_check_ctx` prelude, the `Mode`-conditional
/// exit (`Mode::Analyse` records and stops; `Mode::Build` proceeds to
/// `certify`+`emit_unit`) and the emission tail itself.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::type_complexity)]
pub(super) fn check_unit_files(
    name: &str,
    kind: UnitKind,
    // #1710: each unit's declarations recovery skipped (`phase_parse`).
    broken: &project_model::BrokenDeclNames,
    indices: &[usize],
    parsed: &[ParsedFile],
    unit_info: &BTreeMap<String, UnitInfo>,
    // #3229: the project's cross-context views, built once across all units.
    views: &check_pipeline::CrossContextViewsCache,
    combined_types: &HashMap<String, Arc<TypeDecl>>,
    combined_fns: &HashMap<String, Arc<FnDecl>>,
    combined_methods: &HashMap<String, ResolverMethodTable>,
    local_names: &HashSet<String>,
    local_methods_for_type: &HashMap<String, Vec<FnDecl>>,
    consumed_types: &HashMap<String, ConsumedType>,
    imported_from: &HashMap<String, String>,
    imported_from_kind: &HashMap<String, UnitKind>,
    // #1807: the types this unit reaches through imported declarations
    // (#1824: and the shadows among them).
    reached_types: &project_model::ReachedTypes,
    owning_context_for_emit: &Option<String>,
    target: BuildTarget,
    import_ext: ImportExt,
    contracts: bool,
    trace_requests: bool,
    inspect_agents: bool,
    agent_deps_plan: Option<&AgentDepsPlan>,
    history_target_agents: &HashSet<String>,
    asset_served: &BTreeMap<String, String>,
    embed_text: &BTreeMap<String, String>,
    mode: Mode,
    policy: &Arc<bynk_check::stability::StabilityPolicy>,
    // #3070: the schemas the D1 migrations build, which literal D1 SQL is
    // prepared against.
    d1_sql: &Option<Arc<bynk_check::d1_sql::D1Sql>>,
    errors: &mut ErrorSink,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    exprs: &mut ExprTypeSink,
    requirements: &mut RequirementSink,
    compiled: &mut Vec<StagedFile>,
    // Events track, slice 3c (#980): each locally-declared event's *effective*
    // schema version, keyed `<unit>.<EventName>` — the schema registry's
    // reconciled value when the registry is on, empty (so every lookup falls
    // through to `EventDecl::schema_version()`) when it is off.
    schema_effective_versions: &SchemaVersions,
    tys: &Arc<Types>,
    // #1187's slice 6 plumbing — this unit's own accumulator; merged into
    // per file below, from each file's own certified `CheckedProgram`
    // (`RunChecks::Checked::unit_callees`'s own doc comment has the full
    // grounding for why this exists).
    unit_callees: &mut HashMap<ExprId, bynk_check::checker::Callee>,
    // P6.x (#1232): this unit's own declared event-subscriber service
    // shapes, keyed by service name — see `EventSubscriberShape`'s own doc
    // comment. Populated the same way as `unit_callees` above: merged from
    // each file's own `CheckedProgram` before it is dropped at the end of
    // this loop's iteration.
    event_subscriber_shapes: &mut HashMap<String, EventSubscriberShape>,
    // #2719: the build's needs-context set so far; this unit extends it.
    exec_plan: &mut bynk_check::exec_plan::ExecPlan,
    layout: &WorkerLayout,
) {
    // Emit-prologue tables invariant across every file of this unit — built
    // once here rather than once per file (see `EmitUnitCtx`).
    let split_worker =
        target == BuildTarget::Workers && kind == UnitKind::Context && indices.len() > 1;
    let unit_ctx = build_emit_unit_ctx(
        name,
        unit_info,
        &reached_types.hidden,
        target,
        tys,
        split_worker,
        layout,
    );
    let first_emitted = compiled.len();
    let mut check_ctx = prepare_unit_check_ctx(
        name,
        kind,
        broken,
        unit_info,
        views,
        combined_types,
        imported_from_kind,
        reached_types,
        policy,
    );
    check_ctx.d1_sql = d1_sql.clone();

    // #2719: check every file of the unit before emitting any. Whether a
    // function takes the hidden execution-context parameter depends on the
    // functions it calls, which may sit in a sibling file, so the unit's
    // needs-context set is settled between the two.
    let mut checked: Vec<(
        usize,
        bynk_check::checker::TypedCommons,
        bynk_check::resolver::CrossContextInfo,
    )> = Vec::new();
    for &i in indices {
        let pf = &parsed[i];
        let Some(check_pipeline::FileCheckResult {
            typed,
            cross_context: cross_context_for_file,
        }) = check_pipeline::check_file_core(
            name,
            kind,
            pf,
            unit_info,
            combined_types,
            combined_fns,
            combined_methods,
            local_names,
            local_methods_for_type,
            consumed_types,
            imported_from,
            &check_ctx,
            errors,
            refs,
            hints,
            locals,
            exprs,
            requirements,
            tys,
        )
        else {
            continue;
        };

        // Analyse mode stops at checked: emission is build-only. Capture the
        // file's expression types on the way out (Ok path only — this point is
        // past every per-file error exit inside `check_file_core`), for
        // `.`-member completion.
        if mode == Mode::Analyse {
            check_pipeline::record_analyse_types(
                exprs,
                &pf.identity_path(),
                pf.is_synthetic(),
                &typed.expr_types,
            );
        }
        checked.push((i, typed, cross_context_for_file));
    }
    let plan_errors = {
        let files: Vec<&bynk_check::checker::TypedCommons> =
            checked.iter().map(|(_, t, _)| t).collect();
        exec_plan.extend(&files)
    };
    let plan_failed = !plan_errors.is_empty();
    errors.extend_attributed(plan_errors);
    if mode == Mode::Analyse {
        // #3142: an analysis keeps just the D1 SQL sites, for `bynk test
        // --mutate`; emission below is build-only.
        for (_, typed, _) in &checked {
            unit_callees.extend(typed.callees.iter().filter_map(|(id, c)| {
                let keep = match c {
                    bynk_check::checker::Callee::D1Sql(_) => true,
                    // #3890: the cross-context call edges, for the call-admission phase.
                    bynk_check::checker::Callee::Cross { .. } => true,
                    bynk_check::checker::Callee::Const(_, site) => site.is_some(),
                    // #3355: an inbound email handler's `Forward(...)`.
                    bynk_check::checker::Callee::Ctor { sum, tag } => {
                        tag == "Forward" && sum.name.name == "EmailVerdict"
                    }
                    // #3879: a send to a queue, for the producer check.
                    c if bynk_check::queue_producers::is_queue_send(c) => true,
                    // `Events.emit`: whether the bundle has a composition
                    // root (`project_model::phase_bundle_composition_unprovided`),
                    // which `bynk check` decides as the build does.
                    c => project_model::is_events_emit(c),
                };
                keep.then(|| (*id, c.clone()))
            }));
        }
    }
    if mode == Mode::Analyse && !plan_failed {
        // #4233: `bynk check` still emits each suite's test module, and a
        // case's `Svc.event(e, env)` reads the subscriber shapes; Build mode
        // records them below, so an analysis records them here too.
        for (_, typed, _) in checked {
            if let Ok(program) = checker::certify(typed, Vec::new()) {
                event_subscriber_shapes
                    .extend(bynk_model::lower_event_subscriber_shapes_ir(&program));
            }
        }
        return;
    }
    if plan_failed {
        return;
    }
    let exec_snapshot = Arc::new(exec_plan.clone());
    for (i, mut typed, cross_context_for_file) in checked {
        let pf = &parsed[i];
        typed.exec_plan = Arc::clone(&exec_snapshot);
        let program = checker::certify(typed, Vec::new()).unwrap_or_else(|_| {
            panic!("bynk internal error: unit already passed every per-unit gate above")
        });
        // #1187's slice 6 plumbing: merge this file's own resolved `Callee`
        // classification into the unit's accumulator before `program` (and
        // the `TypedCommons` it wraps) is dropped at the end of this
        // iteration — the only point in this pipeline that ever holds it.
        // Filtered to the two variants either reader actually matches on
        // (review of #1202): every other `Callee` variant would otherwise
        // sit retained project-wide, for the rest of the build, to answer
        // the questions its readers ask (since W30, whether a body calls
        // `Idempotency.claim`, which declares the implicit claims database) — a real `String`/`Arc` cost on a large
        // project with nothing reading the rest yet. Widen this filter (or
        // drop it) the moment a future reader needs a different variant.
        unit_callees.extend(program.program().callees.iter().filter_map(|(id, c)| {
            let keep = match c {
                bynk_check::checker::Callee::Cross { .. } => true,
                bynk_check::checker::Callee::Capability {
                    op, first_party, ..
                } => {
                    (*first_party == Some(FirstPartyCap::Events) && op == "emit")
                        || *first_party == Some(FirstPartyCap::Idempotency)
                        || bynk_check::queue_producers::is_send_op(op)
                }
                // W30 (#2840): an `Idempotency` call through a consumed context's qualified name.
                bynk_check::checker::Callee::CrossCap { first_party, .. } => {
                    *first_party == Some(FirstPartyCap::Idempotency)
                }
                _ => false,
            };
            keep.then(|| (*id, c.clone()))
        }));
        // P6.23 (review of #1254): captures this file's own
        // event-subscriber service shapes the same way as before —
        // before `program` is dropped at the end of this iteration, same
        // insertion point as `unit_callees.extend` above. P6.47 (#1137):
        // the walk itself (including the `ServiceProtocol::Events`
        // pre-filter guarding `lower_service_item_ir` — see that
        // function's own doc comment for why the guard stays) relocated to
        // `bynk_model::lower_event_subscriber_shapes_ir`, an excluded file.
        event_subscriber_shapes.extend(bynk_model::lower_event_subscriber_shapes_ir(&program));
        emit_unit(
            name,
            kind,
            pf,
            &unit_ctx,
            history_target_agents,
            asset_served,
            embed_text,
            unit_info,
            imported_from,
            imported_from_kind,
            owning_context_for_emit,
            &cross_context_for_file,
            &program,
            target,
            import_ext,
            contracts,
            trace_requests,
            inspect_agents,
            agent_deps_plan,
            compiled,
            errors,
            schema_effective_versions,
            project_mentions_encode_plain(parsed),
        );
    }
    if mode == Mode::Build && split_worker {
        let barrel = emitter::commons_barrel::emit_worker_handlers_barrel(
            name,
            indices,
            parsed,
            &compiled[first_emitted..],
            import_ext,
            layout,
        );
        compiled.push(barrel);
    }
}

/// #4045: each qualifier (`Y`, `lib.y`) the unit's `uses` views file a name
/// under, with the commons it qualifies, in a stable order.
fn use_qualifiers(table: &bynk_check::symbols::UnitTable) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (target, view) in &table.use_views {
        let keys = view
            .types
            .keys()
            .chain(view.fns.keys())
            .chain(view.consts.keys())
            .chain(view.assets.keys())
            .chain(view.embeds.keys());
        for key in keys {
            if let Some((qualifier, _)) = key.rsplit_once('.')
                && !out.iter().any(|(q, _)| q == qualifier)
            {
                out.push((qualifier.to_string(), target.clone()));
            }
        }
    }
    out.sort();
    out
}
