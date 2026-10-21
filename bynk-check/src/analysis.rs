//! The project-level analysis entry point (P4.1, #1115): discovery
//! (`bynk-project`) → parse → resolve → check, returning the `bynk-ide`-facing
//! analogue of `bynk-emit`'s `ProjectAnalysis` — without ever emitting.
//!
//! `bynk-ide` is repointed at [`analyse_project`] as of P4.2 (#1122) —
//! `bynk_emit::project::analyse_project_with` was no longer reachable from
//! `bynk-ide` from that point on (it had no `bynk-emit` dependency left).
//! This entry point is what every real caller uses; #1541 deleted
//! `analyse_project_with` and its single-tree wrapper `analyse_project`
//! outright once nothing but `bynk-check/tests/differential_analysis.rs`'s
//! own comparison still called them — that fixture is now a golden test of
//! this entry point alone, not a differential against the retired one.
//!
//! ## The residual gap
//!
//! This entry point was diagnostically faithful to `bynk-emit`'s
//! `run_checks`'s `Mode::Analyse` arm **minus seven categories** of
//! whole-project checking at P4.2 (recorded on the tracking issue's own
//! scope-correction comments, not silently assumed). Categories 2, 3, 4 and 6
//! closed at P5.0/P5.1/P5.2 (`design/tracks/semantics-in-the-checker.md` §6)
//! — [`crate::project_model::phase_messages_bundles`]/
//! [`crate::project_model::phase_locale_bundle_ambiguity`]/
//! [`crate::project_model::phase_event_subscriptions`]/
//! [`crate::project_model::phase_function_type_boundaries`] are now called
//! from [`analyse_project`] at the same points `run_checks` calls them.
//! Categories 1 and 5 closed at P5.3, structurally rather than observably —
//! both were already unreachable from the editor before P4.2 even shipped,
//! so porting them changed nothing observable. Category 7 closed at P5.4, the
//! last of the seven and the one this doc comment's own author flagged as
//! needing more care (§9 of the design doc) — see below. All seven
//! categories are now closed:
//!
//! 1. ~~Schema-registry reconciliation~~ — **closed at P5.3**.
//!    [`crate::schema_registry::reconcile`] is now called from
//!    [`analyse_project`], right after [`crate::project_model::phase_validate_providers`]
//!    (the same relative point `run_checks` calls it). Unreachable on this
//!    path until #2081, which gave [`analyse_project`] a `schema_lock`
//!    argument: the LSP pre-reads `bynk.schema.lock` and passes it in, so a
//!    non-additive change now reaches the editor as it reaches `bynk check`.
//! 2. ~~`messages` bundle validation~~ — **closed at P5.0**, see above.
//! 3. ~~Locale bundle ambiguity~~ — **closed at P5.0**, see above.
//! 4. ~~Event-subscription validation~~ — **closed at P5.1**, see above.
//! 5. ~~Platform-lock enforcement~~ — **closed at P5.3**.
//!    [`crate::project_model::phase_platform_lock`] is now called from
//!    [`analyse_project`], right after the per-unit compose/check loop (the
//!    same relative point `run_checks` calls it, gated the same way on a
//!    clean error sink so far). Still unreachable on this path, for the same
//!    reason as before the relocation: `analyse_project` hardcodes
//!    `Platform::default()` (Cloudflare) and `BuildTarget::Bundle`, and
//!    `bynk.cloudflare` is the only platform-native unit that exists
//!    (`firstparty::platform_of`) — so `lock_violation` can never find a
//!    native platform disagreeing with the selected one, for any project, on
//!    this path. No fixture can observe this category regressing (or
//!    improving) because it never fired through this path to begin with, both
//!    before and after this relocation.
//! 6. ~~Function-type-boundary checks~~ — **closed at P5.2**. Formerly reached,
//!    in `bynk-emit`, only through `phase_group`'s optional boundary-check
//!    hook (`Some` from `run_checks`, `None` here); the hook is gone —
//!    [`crate::project_model::phase_group`] now calls
//!    [`crate::project_model::phase_function_type_boundaries`] directly, at
//!    the exact point the hook used to fire, so both callers see it in the
//!    same diagnostic-ordering position as before.
//! 7. ~~Test/integration-suite processing~~
//!    (`process_tests`/`process_integration_tests`) — **closed at P5.4**.
//!    Unlike categories 2-6, these run *unconditionally* in `run_checks`, in
//!    `Mode::Analyse` too, and push into the same shared error sink (`bynk-emit`'s own
//!    `check_project_reports_a_test_body_error_past_an_earlier_structural_error`
//!    pins a `bynk.types.let_annotation_mismatch` originating inside a
//!    `suite`/`test integration` body). The two functions were emission-coupled
//!    (`Document`, `RunnableTest`, `ImportExt`, `contracts`, a shared
//!    `emitted_barrels` set) deeply enough that P5.4 split them at the
//!    check/emit boundary rather than porting the whole thing: their checking
//!    half relocated to [`crate::test_suites::phase_test_bodies`]/
//!    [`crate::test_suites::phase_integration_bodies`], now called from
//!    [`analyse_project`] right after the per-unit compose/check loop (the
//!    same relative point `run_checks` calls the originals, unconditionally —
//!    unlike categories 2-6, neither is gated on a clean error sink), while
//!    emission itself stays in `bynk-emit::project::tests_emit`, which now
//!    calls the relocated checking phase too rather than duplicating it. Both
//!    functions still take `&mut RefSink`, so every binding edge inside a
//!    `.bynk` suite file is populated here again too — go-to-definition
//!    inside a test file works through this entry point once more.
//!
//! Emission itself is orthogonal rather than a gap: this entry point never
//! emits, by construction (it has no `BuildTarget`/`ImportExt`/`contracts`
//! concept at all), so there is no diagnostic-agreement question to ask of it.
//!
//! A fixture that exercises none of the seven categories above saw identical
//! diagnostics from this entry point and from `analyse_project_with` while
//! both existed — that is why `bynk-check/tests/differential_analysis.rs`'s
//! clean/broken golden cases still avoid all seven (#1541 retired the
//! comparison, not the fixture shape). A third case
//! (`new_entry_point_reports_a_test_bodys_own_type_error`, renamed from
//! `new_entry_point_omits_test_body_diagnostics` at #1541) pinned category
//! 7's divergence directly; now that P5.4 closed it, that test pins parity
//! with what `analyse_project_with` used to report instead (see its own doc
//! comment).
//!
//! ## Two sites outside the seven-category accounting
//!
//! `bynk-check/src/analysis.rs`'s own seven categories were `run_checks`'s
//! whole-project checks; two more registered diagnostics were still
//! constructed in `bynk-emit` and outside that accounting, found and closed
//! at P5.5 (`design/tracks/semantics-in-the-checker.md` §6, §9):
//!
//! - `bynk.project.schema_registry_corrupt` — a malformed on-disk
//!   `bynk.schema.lock`. [`crate::schema_registry::parse_or_diagnose`] now
//!   constructs it. Reachable from this entry point since #2081, under the
//!   caller's `SchemaLock::On`, same as category 1.
//! - `bynk.secrets.computed_name` — see
//!   [`crate::project_model::phase_secrets_computed_name`]'s own doc. Unlike
//!   the seven categories (scoped and confirmed live gaps or confirmed
//!   gap-in-name-only by this settling pass), this one's reachability from
//!   *this* entry point was still open at settling time — §9 named it a risk
//!   rather than a scoped item. It resolved the same way categories 1 and 5
//!   did: gap-in-name-only, since `run_checks`'s own gate
//!   (`target == BuildTarget::Workers`) can never pass against this entry
//!   point's hardcoded `BuildTarget::Bundle`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::check_pipeline::{check_file_core, prepare_unit_check_ctx, record_analyse_types};
use crate::checker::Types;
use crate::expr_types::{ExprTypeSink, FileExprTypes};
use crate::firstparty::Platform;
use crate::hints::{FileHints, HintSink};
use crate::index::{ProjectIndex, RefSink};
use crate::locals::{FileLocals, LocalsSink};
use crate::project_model::{
    self, ErrorSink, assemble_unit_info, close_reachable_types, collect_unit_methods,
    compose_unit_symbols, merge_consumed_exports, normalize_service_defaults,
};
use crate::requirements::{FileRequirements, RequirementSink};
use crate::symbols::{assemble_index, build_cross_context_info};
use bynk_project::{AttributedError, Roots, SchemaLock, UnitKind};
use bynk_syntax::ast::{AgentDecl, ServiceDecl, TypeDecl};

/// #846: the per-unit slice of resolution the sequence-diagram classifier
/// needs — see [`ProjectAnalysis::sequence_info`]. Moved verbatim (Decision
/// C, #1115) from `bynk-emit/src/project/diagnostics.rs`.
#[derive(Debug, Clone, Default)]
pub struct ContextSequenceInfo {
    pub cross_context: crate::resolver::CrossContextInfo,
    pub agents: HashMap<String, AgentDecl>,
}

/// #855: the per-unit slice of resolution the wire-contract peek needs —
/// see [`ProjectAnalysis::boundary_info`]. A sibling of
/// [`ContextSequenceInfo`], not a field on it: that struct is named and
/// documented for #846, and this is a separate retained table serving a
/// separate query (hover/panel over a single handler's boundary, not the
/// sequence-diagram classifier). Moved verbatim (Decision C, #1115).
#[derive(Debug, Clone, Default)]
pub struct ContextBoundaryInfo {
    /// `contract_types_for` (#2073): the unit's own declared types, the types
    /// of every commons it `uses` and those of the contexts it consumes — the
    /// same table `own_contract_hashes` hashes through, so the peek's hash and
    /// the emitted `X-Bynk-Contract` constant cannot disagree.
    pub types: HashMap<String, Arc<TypeDecl>>,
    pub services: HashMap<String, ServiceDecl>,
    pub agents: HashMap<String, AgentDecl>,
}

/// v0.24: the analyse-mode result — every discovered file's analysed text
/// snapshot (positions must convert against the text that was analysed, not
/// a newer buffer) plus the attributed diagnostics. Moved verbatim (Decision
/// C, #1115) from `bynk-emit/src/project/diagnostics.rs`; `bynk-emit`
/// re-exports this type at its old path (`bynk_emit::project::ProjectAnalysis`)
/// so `bynk-ide`'s existing destructuring needs no field-by-field rewrite.
pub struct ProjectAnalysis {
    /// `(project-relative source path, analysed text)` for every file read,
    /// including clean files (the LSP needs them to clear diagnostics).
    pub snapshots: Vec<(PathBuf, String)>,
    pub errors: Vec<AttributedError>,
    /// v0.25 (ADR 0053): the project-wide binding index. Empty when the
    /// pipeline bails before resolution (discovery/parse failures).
    pub index: ProjectIndex,
    /// v0.27 (ADR 0056): per-file inferred-type inlay hints — `(binding-name
    /// span, label)`, span-ordered, harvested from the checker's binding
    /// sites. Empty for files the pipeline never type-checked.
    pub hints: FileHints,
    /// v0.30.2 (ADR 0063): per-file expression types — `(expr span, Ty)`,
    /// for `.`-member completion's and signature help's receiver typing.
    /// Recorded at every per-file exit of the check, clean or not: a file
    /// with errors still carries the best-effort partial types the checker
    /// computed for its other expressions (ADR 0094, which lifted ADR 0063's
    /// clean-file ceiling — see [`record_analyse_types`]).
    /// Empty for files the pipeline never type-checked and for synthetic
    /// files (muted).
    pub expr_types: FileExprTypes,
    /// T3.6b (R4.1): the intern table every `TyId` in `expr_types` resolves
    /// against — one table shared across the whole analysis.
    pub ty_intern: Arc<Types>,
    /// v0.31 (ADR 0064): per-file local bindings with their scope ranges —
    /// `let`/`let <-`, fn/handler/lambda params — for the scope-at-offset
    /// query backing locals completion + navigation. Synthetic files muted.
    pub locals: FileLocals,
    /// v0.99: per-file capability-requirement ledger — every
    /// capability-consuming site (direct call, store op), covered or not,
    /// with its provenance. Empty for files the pipeline never type-checked,
    /// and for synthetic/test files (muted).
    pub requirements: FileRequirements,
    /// Slice 6b (ADR 0095): qualified unit name → the project source file(s)
    /// that comprise it, in discovery order. Excludes synthetic
    /// (toolchain-injected) units; empty when the pipeline bails before the
    /// checker.
    pub unit_sources: HashMap<String, Vec<PathBuf>>,
    /// #846: qualified context/adapter unit name → the cross-context and
    /// agent tables needed to classify a handler call as a lifeline for the
    /// sequence-diagram query. Only contexts/adapters have an entry; empty
    /// when the pipeline bails before the checker.
    pub sequence_info: HashMap<String, ContextSequenceInfo>,
    /// #855: qualified context/adapter unit name → the combined type table
    /// and per-context service/agent tables the wire-contract peek needs.
    /// Only contexts/adapters have an entry; empty when the pipeline bails
    /// before the checker.
    pub boundary_info: HashMap<String, ContextBoundaryInfo>,
    /// #848: qualified unit name → its doc-comment intra-doc-link search
    /// order — itself first, then its `uses` targets, then its `consumes`
    /// targets, in that order. Empty when the pipeline bails before the
    /// checker.
    pub doc_scope: HashMap<String, Vec<String>>,
}

/// The `ProjectAnalysis` shape for every "bailed before the checker ran"
/// exit — discovery/file-conflict/parse failures. Factored out so the three
/// early-return sites in [`analyse_project`] agree by construction, the same
/// role `RunChecks::Bailed` plays in `bynk-emit`'s `run_checks`.
fn bailed(
    errors: ErrorSink,
    snapshots: Vec<(PathBuf, String)>,
    mut hints: HintSink,
    mut locals: LocalsSink,
    mut exprs: ExprTypeSink,
    mut requirements: RequirementSink,
    tys: &Arc<Types>,
) -> ProjectAnalysis {
    ProjectAnalysis {
        snapshots,
        // ADR 0117: the LSP renders warnings alongside errors (severity is
        // applied downstream), so analyse surfaces the full diagnostic list.
        errors: errors.into_all(),
        index: ProjectIndex::default(),
        hints: hints.take_files(),
        locals: locals.take_files(),
        expr_types: exprs.take_files(),
        ty_intern: Arc::clone(tys),
        requirements: requirements.take_files(),
        unit_sources: HashMap::new(),
        sequence_info: HashMap::new(),
        boundary_info: HashMap::new(),
        doc_scope: HashMap::new(),
    }
}

/// The `bynk-check`-native discovery→parse→resolve→check entry point (P4.1,
/// #1115) — see this module's own doc comment for the residual-gap
/// accounting against `bynk-emit`'s now-deleted (#1541) `analyse_project_with`.
/// Mirrored its call shape exactly where the two overlapped:
/// `BuildTarget::Bundle`-equivalent (this entry point has no build target at
/// all — it never emits), `Platform::default()`.
///
/// `schema_lock` (#2081) is `bynk.schema.lock`'s content, pre-read by the
/// caller exactly as `bynk check` pre-reads it (this crate reads no lock from
/// disk). Under [`SchemaLock::On`] every event is reconciled against it and a
/// non-additive change, a corrupt lock or a mismatched `@schema(N)` is
/// reported as the build reports it; under [`SchemaLock::Off`] reconciliation
/// is skipped. Nothing is written either way.
///
/// Identity is project-relative (ADR 0198): a file's `source_path` here is
/// unique across `include` roots, same as `analyse_project_with` used to be.
pub fn analyse_project(
    roots: &Roots,
    overlay: &HashMap<PathBuf, String>,
    schema_lock: &SchemaLock,
) -> ProjectAnalysis {
    // S3 (#1963): the project's `[unstable]` opt-in, read the way the build
    // reads it, so the editor and `bynkc` agree about every file.
    let policy =
        crate::stability::StabilityPolicy::for_project(Some(roots.project_root()), overlay);
    analyse_project_with_policy(roots, overlay, policy, schema_lock)
}

/// [`analyse_project`] under a given [`crate::stability::StabilityPolicy`] —
/// how a test treats a shipped item as experimental, since none is yet.
pub fn analyse_project_with_policy(
    roots: &Roots,
    overlay: &HashMap<PathBuf, String>,
    policy: crate::stability::StabilityPolicy,
    schema_lock: &SchemaLock,
) -> ProjectAnalysis {
    let tys = &Arc::new(Types::new());
    // #2223: a manifest-backed project's path dependencies are walked too.
    let packages = matches!(roots, Roots::Split { .. });
    let mut trees = roots.trees();
    if packages {
        trees.extend(bynk_project::dependency_trees(
            roots.project_root(),
            overlay,
        ));
    }
    let excludes = roots.excludes();
    let policy = Arc::new(policy);

    let mut errors = ErrorSink::new();
    for err in policy.manifest_diagnostics() {
        errors.push_for(Some(Path::new(crate::stability::MANIFEST)), err);
    }
    let mut refs = RefSink::new();
    let mut hints = HintSink::new();
    let mut locals = LocalsSink::new();
    let mut requirements = RequirementSink::new();
    let mut exprs = ExprTypeSink::new();
    let mut snapshots: Vec<(PathBuf, String)> = Vec::new();

    // -- 1. Discovery. --
    let file_lists = match project_model::phase_discovery(&trees, &excludes, &mut errors) {
        Ok(files) => files,
        Err(()) => return bailed(errors, snapshots, hints, locals, exprs, requirements, tys),
    };
    if project_model::check_discovered_files(&trees, &file_lists, &mut errors).is_err() {
        return bailed(errors, snapshots, hints, locals, exprs, requirements, tys);
    }

    // -- 2. Parse. --
    let (mut parsed, injected, broken) =
        match project_model::phase_parse(&trees, &file_lists, overlay, &mut errors, &mut snapshots)
        {
            Ok(out) => out,
            Err(()) => return bailed(errors, snapshots, hints, locals, exprs, requirements, tys),
        };

    // -- 2a. #2223: fold the project's path dependencies in. --
    if packages {
        crate::packages::apply_graph(
            Some(roots.project_root()),
            overlay,
            &mut parsed,
            true,
            &mut errors,
        );
    }

    // -- 2b. Normalize service-level `by`/`given` defaults (v0.155). --
    normalize_service_defaults(&mut parsed);
    let parsed = parsed;

    // -- 3. Group. P5.2: closes category 6 of this module's own residual-gap
    //       accounting (see doc comment above) — `phase_group` now also
    //       confines function types to non-boundary positions directly, at
    //       the point its old optional hook used to fire. --
    let (groups, kinds, test_groups, integration_groups, _adapter_bindings, _npm_deps) =
        project_model::phase_group(
            &parsed,
            &trees,
            Platform::default(),
            &injected,
            overlay,
            &mut errors,
        );
    // #2207: `[package] exports` names only commons and adapters this project
    // declares, read the way the build reads it.
    for err in crate::exports::manifest_diagnostics(Some(roots.project_root()), overlay, &kinds) {
        errors.push_for(Some(Path::new(crate::stability::MANIFEST)), err);
    }

    // -- 4. Per-unit combined symbol tables. --
    let unit_tables = project_model::phase_symbol_tables(&groups, &kinds, &parsed, &mut errors);

    // -- 5. `uses` resolution. --
    let unit_uses = project_model::phase_resolve_uses(
        &groups,
        &kinds,
        &parsed,
        &unit_tables,
        &policy,
        &mut errors,
    );

    // -- 5b. `consumes` resolution. --
    let (unit_consumes, unit_flattened) = project_model::phase_resolve_consumes(
        &groups,
        &kinds,
        &parsed,
        &unit_tables,
        &policy,
        &mut errors,
        &mut refs,
    );
    let mut unit_tables = unit_tables;
    crate::symbols::record_flattened_caps(&mut unit_tables, &unit_flattened);
    crate::project_model::phase_resolve_derived(
        &groups,
        &kinds,
        &parsed,
        &mut unit_tables,
        &mut errors,
    );
    let unit_tables = unit_tables;

    // -- 5b'. `consumes` aliases. --
    let unit_consumes_aliases =
        project_model::phase_consumes_aliases(&groups, &kinds, &parsed, &unit_tables, &mut errors);

    // -- 5b''. v0.173 (ADR 0196 D1), P5.5 (`design/tracks/semantics-in-the-checker.md`
    //          §6, §9): warn where a `bynk.Secrets` read names its secret with
    //          a computed expression — closes the "ninth gap" that §9 flagged
    //          as unresolved risk rather than a scoped relocation. Mirrors
    //          `run_checks`'s own call at the same relative point. Gated on
    //          the Workers target, same as `run_checks`; this entry point
    //          hardcodes `BuildTarget::Bundle` (inherited from the
    //          now-deleted `analyse_project_with`'s own hardcoding, see this
    //          function's doc comment), so the call
    //          closes the category structurally (R3.5 — the diagnostic now
    //          originates in `bynk-check`), not observably, the same as
    //          categories 1 and 5. --
    project_model::phase_secrets_computed_name(
        project_model::BuildTarget::Bundle,
        &parsed,
        &groups,
        &kinds,
        &unit_flattened,
        &mut errors,
    );
    // #1822: Workers-only, so a no-op on this Bundle path, called for the
    // same structural reason as the phase above.
    project_model::phase_workers_unprovided_capabilities(
        project_model::BuildTarget::Bundle,
        &parsed,
        &groups,
        &kinds,
        &unit_tables,
        &mut errors,
    );

    // -- 5c. `consumes` cycles. --
    project_model::phase_detect_consumes_cycles(&groups, &parsed, &unit_consumes, &mut errors);

    // -- 6. `uses` name-conflict detection. --
    project_model::phase_uses_name_conflicts(
        &unit_uses,
        &unit_tables,
        &parsed,
        &groups,
        &mut errors,
    );

    // -- 6a0. #1883, slice 1b: every declared asset names a file in the
    //         `assets/` folder, listed from disk as discovery already reads
    //         it. An unreadable folder skips the check rather than flagging
    //         every asset in the editor.
    let available_assets = bynk_project::assets::list_asset_files(roots.project_root()).ok();
    crate::assets::phase_asset_files(&parsed, available_assets.as_ref(), &mut errors);
    crate::assets::phase_asset_routes(&parsed, &groups, &kinds, &unit_uses, &mut errors);

    // -- 6a'. message-bundles slice 1 (#859): messages-block legality,
    //         @reference cardinality, within-block duplicate codes, and the
    //         `uses bynk.locale` dependency. P5.0: closes category 2 of this
    //         module's own residual-gap accounting. --
    project_model::phase_messages_bundles(&parsed, &groups, &kinds, &unit_uses, &mut errors);

    // -- 6a''. Locale capability track, slice 2 (#882): a context reaching
    //          two or more message-bundle commons while consuming `Locale`
    //          has no single bundle to negotiate against. P5.0: closes
    //          category 3. --
    project_model::phase_locale_bundle_ambiguity(
        &parsed,
        &groups,
        &kinds,
        &unit_uses,
        &unit_flattened,
        &mut errors,
    );

    // -- 6a'''. Events track, slice 0 (spine #936): a `from Events(E)`
    //           subscription must name a real, declared event — needs
    //           `unit_tables` + `unit_consumes` together, so it runs here
    //           rather than in the per-context `check_service_protocols`.
    //           P5.1: closes category 4. --
    project_model::phase_event_subscriptions(
        &parsed,
        &groups,
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_uses,
        &mut errors,
    );

    // -- 6b. Type exports. --
    let exports_visibility = project_model::phase_validate_type_exports(
        &groups,
        &kinds,
        &parsed,
        &unit_tables,
        &mut errors,
        &mut refs,
    );

    // -- 6b'. Capability exports. --
    project_model::phase_validate_capability_exports(
        &groups,
        &kinds,
        &parsed,
        &unit_tables,
        &mut errors,
        &mut refs,
    );

    // -- 6c. Provider matching. --
    project_model::phase_validate_providers(
        &unit_tables,
        &groups,
        &parsed,
        &broken,
        &mut errors,
        tys,
    );

    // -- 6d. Events track, slice 3c (#980): schema-registry reconciliation,
    //        at the same point `run_checks` calls it and with the same two
    //        arms. #2081: the caller (the LSP) now pre-reads
    //        `bynk.schema.lock` and hands it in, so a non-additive change, a
    //        corrupt lock or a mismatched `@schema(N)` reaches the editor as
    //        it reaches `bynk check`. `Off` (no lock, or a caller with none)
    //        skips the sweep entirely rather than reconciling against an
    //        empty registry, which can report nothing — closing review
    //        #1133's cost note about paying for it on every keystroke. The
    //        reconciled registry is discarded: analysis never writes. --
    if let SchemaLock::On { existing } = schema_lock {
        match crate::schema_registry::parse_or_diagnose(existing.as_deref(), roots.project_root()) {
            Ok(existing_reg) => {
                let mut schema_errors: Vec<bynk_syntax::error::CompileError> = Vec::new();
                let (mut updated, _) = crate::schema_registry::reconcile(
                    &existing_reg,
                    &unit_tables,
                    &mut schema_errors,
                );
                errors.extend_for(None, schema_errors);
                // State-migrations S0 (#2204): agent state shapes, as
                // `run_checks` reconciles them.
                let mut agent_errors: Vec<bynk_syntax::error::CompileError> = Vec::new();
                crate::schema_registry::reconcile_agents(
                    &existing_reg,
                    &mut updated,
                    &unit_tables,
                    &unit_uses,
                    &mut agent_errors,
                );
                errors.extend_attributed_by_span(&parsed, agent_errors);
            }
            Err(err) => errors.push_for(None, err),
        }
    }

    // No bail gate: this entry point never bails after discovery (mirrors
    // `Mode::Analyse` — independent unit groups resolve/check past another
    // group's errors).

    // -- 7. Per-unit file index. --
    let unit_file_index = project_model::phase_file_index(&groups, &parsed);

    // -- 7b. Assemble per-unit info. --
    let unit_info = assemble_unit_info(
        &groups,
        &kinds,
        &unit_tables,
        &unit_uses,
        &unit_consumes,
        &unit_flattened,
        &unit_consumes_aliases,
        &exports_visibility,
        &unit_file_index,
    );

    // -- 8. For each unit, compose the symbol space and resolve+check every
    //       file. Test/integration processing is the residual gap after this
    //       loop — see this module's own doc comment. Category 5
    //       (platform-lock) closes right after, below the loop, at the same
    //       relative point `run_checks` calls it (after its own per-unit
    //       checking, gated on a clean error sink so far). --
    // #1702 review: `uses` targets first, so a generic callee's compared type
    // parameters are known before an importer's calls are checked.
    crate::checker::reset_compared_cache();
    for name in project_model::uses_first_order(unit_info.keys(), &unit_uses) {
        let info = &unit_info[name];
        let kind = info.kind;
        let indices = info.files.as_slice();
        let local_table = &info.table;
        let group_error_baseline = errors.len();

        let (
            mut combined_types,
            combined_fns,
            mut combined_methods,
            mut imported_from,
            mut imported_from_kind,
        ) = compose_unit_symbols(name, local_table, &unit_info);
        let consumed_types = merge_consumed_exports(
            name,
            &parsed,
            &unit_info,
            &mut combined_types,
            &mut combined_methods,
            &mut imported_from,
            &mut imported_from_kind,
            &mut errors,
        );
        // #1807: close over the types the imported declarations reach.
        let reached_types = close_reachable_types(
            name,
            &parsed,
            &unit_info,
            &mut combined_types,
            &combined_fns,
            &mut combined_methods,
            &mut imported_from,
            &mut imported_from_kind,
            &mut errors,
        );

        if errors.len() > group_error_baseline {
            continue;
        }

        let local_names: HashSet<String> = local_table.types.keys().cloned().collect();
        let local_methods_for_type = collect_unit_methods(indices, &parsed);
        let ctx = prepare_unit_check_ctx(
            name,
            kind,
            &broken,
            &unit_info,
            &combined_types,
            &imported_from_kind,
            &reached_types,
            &policy,
        );

        for &i in indices {
            let pf = &parsed[i];
            if let Some(crate::check_pipeline::FileCheckResult { typed, .. }) = check_file_core(
                name,
                kind,
                pf,
                &unit_info,
                &combined_types,
                &combined_fns,
                &combined_methods,
                &local_names,
                &local_methods_for_type,
                &consumed_types,
                &imported_from,
                &ctx,
                &mut errors,
                &mut refs,
                &mut hints,
                &mut locals,
                &mut exprs,
                &mut requirements,
                tys,
            ) {
                // This entry point never emits — every clean file's exit is
                // the Analyse-mode one (record best-effort/final types, move
                // on to the next file). Mirrors `check_unit_files`'s own
                // `mode == Mode::Analyse` clean-path branch.
                record_analyse_types(
                    &mut exprs,
                    &pf.identity_path(),
                    pf.is_synthetic(),
                    &typed.expr_types,
                );
            }
        }
    }

    // P5.4 (`design/tracks/semantics-in-the-checker.md` §6): test/
    // integration-suite processing — closes category 7 of this module's own
    // residual-gap accounting, the last of the seven. Mirrors `run_checks`'s
    // own call shape: both run unconditionally (unlike categories 2-6 above,
    // `run_checks` never gates these on a clean error sink), right after its
    // own per-unit `check_unit_files` loop and before platform-lock — the
    // same relative point this loop just occupied. Neither function's
    // returned "ready for emission" map is needed here — this entry point
    // never emits — only the diagnostic/`RefSink` side effects matter, so
    // both are discarded. Diagnostics are file-unattributed (`extend_for(None,
    // ...)`), matching `run_checks`'s own `#696`-noted gap (attributing them
    // means threading a file through many internal push sites — out of scope
    // here, same as there).
    let mut test_errors: Vec<bynk_syntax::error::CompileError> = Vec::new();
    let _ready_tests = crate::test_suites::phase_test_bodies(
        &test_groups,
        &parsed,
        &kinds,
        &unit_tables,
        &exports_visibility,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_uses,
        &mut test_errors,
        &mut refs,
        tys,
    );
    errors.extend_attributed_by_span(&parsed, test_errors);

    let mut integration_errors: Vec<bynk_syntax::error::CompileError> = Vec::new();
    let _ready_integration = crate::test_suites::phase_integration_bodies(
        &integration_groups,
        &parsed,
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_uses,
        &mut integration_errors,
        &mut refs,
        tys,
    );
    errors.extend_attributed_by_span(&parsed, integration_errors);

    // v0.19 (decisions 0017/0024), P5.3: platform-lock enforcement — closes
    // category 5 of this module's own residual-gap accounting. Inherited from
    // the now-deleted `analyse_project_with`'s own hardcoded
    // `Platform::default()` (Cloudflare) and `BuildTarget::Bundle`:
    // `bynk.cloudflare` is the only
    // platform-native unit that exists, and it matches the default
    // selection, so `lock_violation` can never fire here, for any project
    // (see `bynk-lsp/tests/analysis_residual_gap.rs`'s
    // `platform_lock_diagnostic_stays_absent`) — this call closes the
    // category structurally (R3.5), not observably.
    //
    // Cost (review #1133): a full provider-closure walk per context, same
    // shape as `run_checks`'s own gate, for a diagnostic that is provably
    // dead here. `collect_given_closure` is also unmemoised — a
    // diamond-shaped provider graph re-walks shared subtrees, so this is
    // worse than linear in the closure's depth, not just wasted. Same
    // trade-off and same "worth profiling if it ever shows up" note as the
    // schema-registry reconciliation call above.
    if errors.is_empty() {
        project_model::phase_platform_lock(
            project_model::BuildTarget::Bundle,
            Platform::default(),
            &parsed,
            &groups,
            &kinds,
            &unit_tables,
            &unit_consumes,
            &unit_consumes_aliases,
            &unit_flattened,
            &mut errors,
        );
    }

    // -- Assemble the `ProjectAnalysis`. Mirrored the now-deleted
    //    `analyse_project_with`'s own `RunChecks::Checked` arm exactly. --
    let index = assemble_index(
        &parsed,
        &unit_uses,
        &unit_consumes,
        std::mem::take(&mut refs),
    );

    let mut unit_sources: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for pf in &parsed {
        if pf.is_synthetic() {
            continue;
        }
        unit_sources
            .entry(pf.unit().name().joined())
            .or_default()
            .push(pf.identity_path());
    }

    let mut sequence_info: HashMap<String, ContextSequenceInfo> = HashMap::new();
    let mut boundary_info: HashMap<String, ContextBoundaryInfo> = HashMap::new();
    for (name, kind) in &kinds {
        if !matches!(kind, UnitKind::Context | UnitKind::Adapter) {
            continue;
        }
        let Some(table) = unit_tables.get(name) else {
            continue;
        };
        let mut cross_context = build_cross_context_info(
            name,
            &unit_consumes,
            &unit_consumes_aliases,
            &unit_uses,
            &unit_tables,
        );
        cross_context.flattened_caps = unit_flattened.get(name).cloned().unwrap_or_default();
        let agents: HashMap<String, AgentDecl> = table.agents.clone();
        sequence_info.insert(
            name.clone(),
            ContextSequenceInfo {
                cross_context,
                agents: agents.clone(),
            },
        );
        boundary_info.insert(
            name.clone(),
            ContextBoundaryInfo {
                // #2073: the table `own_contract_hashes` hashes through.
                types: crate::symbols::contract_types_for(
                    name,
                    &unit_tables,
                    &unit_uses,
                    &unit_consumes,
                )
                .0,
                services: table.services.clone(),
                agents,
            },
        );
    }

    let mut doc_scope: HashMap<String, Vec<String>> = HashMap::new();
    for name in unit_sources.keys() {
        let mut scope = vec![name.clone()];
        scope.extend(unit_uses.get(name).cloned().unwrap_or_default());
        scope.extend(unit_consumes.get(name).cloned().unwrap_or_default());
        doc_scope.insert(name.clone(), scope);
    }

    ProjectAnalysis {
        snapshots,
        errors: errors.into_all(),
        index,
        hints: hints.take_files(),
        locals: locals.take_files(),
        expr_types: exprs.take_files(),
        ty_intern: Arc::clone(tys),
        requirements: requirements.take_files(),
        unit_sources,
        sequence_info,
        boundary_info,
        doc_scope,
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    const MONEY: &str = "commons demo.money\n\ntype Cents = Int where NonNegative\n";

    const PAYMENT: &str = "context demo.payment\n\n\
exports transparent { Receipt }\n\n\
type Receipt = { id: String }\n\n\
service authorise {\n  on call(id: String) -> Effect[Result[Receipt, Int]] {\n    Ok(Receipt { id: id })\n  }\n}\n";

    const CHECKOUT: &str = "context demo.checkout\n\n\
uses demo.money\n\nconsumes demo.payment\n\n\
fn price() -> Cents { 100 }\n\n\
service pay {\n  on call(id: String) -> Effect[Result[Receipt, Int]] {\n    demo.payment.authorise(id)\n  }\n}\n";

    #[test]
    fn a_consuming_context_checks_clean_against_its_uses_and_consumes() {
        let a = analyse(&[
            ("demo/money.bynk", MONEY),
            ("demo/payment.bynk", PAYMENT),
            ("demo/checkout.bynk", CHECKOUT),
        ]);
        a.assert_clean();
        assert_eq!(
            a.type_at("demo/checkout.bynk", "demo.payment.authorise(id)"),
            "Effect[Result[Receipt, Int]]"
        );
    }

    #[test]
    fn doc_scope_is_the_unit_then_its_uses_then_its_consumes() {
        let a = analyse(&[
            ("demo/money.bynk", MONEY),
            ("demo/payment.bynk", PAYMENT),
            ("demo/checkout.bynk", CHECKOUT),
        ]);
        assert_eq!(
            a.analysis.doc_scope["demo.checkout"],
            ["demo.checkout", "demo.money", "demo.payment"]
        );
        assert_eq!(a.analysis.doc_scope["demo.money"], ["demo.money"]);
    }

    #[test]
    fn unit_sources_name_each_unit_s_own_files_and_no_synthetic_unit() {
        let a = analyse(&[("demo/money.bynk", MONEY), ("demo/payment.bynk", PAYMENT)]);
        let mut units: Vec<&str> = a.analysis.unit_sources.keys().map(String::as_str).collect();
        units.sort();
        assert_eq!(units, ["demo.money", "demo.payment"]);
        assert!(a.analysis.unit_sources["demo.money"][0].ends_with("demo/money.bynk"));
    }

    #[test]
    fn sequence_and_boundary_info_cover_contexts_only() {
        let a = analyse(&[
            ("demo/money.bynk", MONEY),
            ("demo/payment.bynk", PAYMENT),
            ("demo/checkout.bynk", CHECKOUT),
        ]);
        for info in [
            a.analysis.sequence_info.keys().collect::<Vec<_>>(),
            a.analysis.boundary_info.keys().collect::<Vec<_>>(),
        ] {
            let mut units: Vec<&str> = info.into_iter().map(String::as_str).collect();
            units.sort();
            assert_eq!(units, ["demo.checkout", "demo.payment"]);
        }
    }

    #[test]
    fn a_broken_unit_does_not_stop_an_independent_unit_from_checking() {
        let a = analyse(&[
            ("demo/money.bynk", MONEY),
            (
                "demo/broken.bynk",
                "commons demo.broken\n\nfn f() -> Int { \"not an int\" }\n",
            ),
            (
                "demo/fine.bynk",
                "commons demo.fine\n\nuses demo.money\n\nfn price() -> Cents { 100 }\n",
            ),
        ]);
        assert!(
            a.analysis.errors.iter().all(|e| e
                .source_path
                .as_deref()
                .is_some_and(|p| p.ends_with("demo/broken.bynk"))),
            "only the broken unit reports:\n{}",
            a.render()
        );
        assert_eq!(a.type_at("demo/fine.bynk", "100"), "Cents");
    }

    #[test]
    fn every_file_read_is_snapshotted_clean_or_not() {
        let a = analyse(&[
            ("demo/money.bynk", MONEY),
            (
                "demo/broken.bynk",
                "commons demo.broken\n\nfn f() -> Int { \"x\" }\n",
            ),
        ]);
        let mut paths: Vec<&std::path::Path> = a
            .analysis
            .snapshots
            .iter()
            .map(|(p, _)| p.as_path())
            .collect();
        paths.sort();
        assert_eq!(paths.len(), 2, "{paths:?}");
        assert!(paths[0].ends_with("demo/broken.bynk") && paths[1].ends_with("demo/money.bynk"));
    }
}
