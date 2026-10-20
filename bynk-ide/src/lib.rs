//! Bynk's IDE/LSP analysis surface.
//!
//! The non-bailing diagnostics the language server consumes — single-file
//! ([`diagnose`]) and whole-project ([`diagnose_project_with`], with
//! [`diagnose_project`] the single-tree convenience) — plus the result
//! types ([`Diagnostic`], [`FileDiagnostics`], [`ProjectDiagnostics`]). These
//! are *queries* over the captured tables produced during analysis (the binding
//! index, inlay hints, expression types, locals — all in `bynk-check`); the
//! project analysis itself ([`bynk_check::analysis::analyse_project`]) is the
//! non-bailing counterpart to `compile_project`.
//!
//! Extracted from `bynkc` as slice 5 of the crate-decomposition track over
//! `bynk-syntax` + `bynk-check` + `bynk-emit` (P4.2, #1122: this crate no
//! longer depends on `bynk-emit` at all — it reaches `bynk-check` and
//! `bynk-project` directly). Behaviour is unchanged; the
//! language server (`bynk-lsp`) depends on this crate directly instead of the
//! whole `bynkc` compiler crate, and `bynkc` re-exports these items so its own
//! tests and public API are unchanged.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bynk_check::{checker, expr_types, hints, index, locals, requirements, resolver};
use bynk_syntax::error::{CompileError, Severity};
use bynk_syntax::{ast, lexer, parser};

/// #855: same rationale as [`ContextSequenceInfo`] above — a plain
/// re-export, since `ContextBoundaryInfo`'s fields are already
/// IDE-appropriate as-is.
pub use bynk_check::analysis::ContextBoundaryInfo;
/// #846: re-exported rather than left as a raw `bynk_check::analysis` path —
/// `bynk-lsp` links `bynk-ide`/`bynk-check`/`bynk-syntax` directly and
/// deliberately does not depend on `bynk-emit` (the whole-compiler crate);
/// see `bynk-lsp/Cargo.toml`'s dependency comment. Unlike `Roots`/
/// `AnalysisRoots` (which the IDE layer re-shapes because the raw type
/// carries build-only concerns), `ContextSequenceInfo`'s fields are already
/// IDE-appropriate as-is, so this is a plain re-export rather than a lowering.
pub use bynk_check::analysis::ContextSequenceInfo;
pub use bynk_project::LabelFile;
/// #2081: how a caller hands [`diagnose_project_with`] the project's
/// `bynk.schema.lock` (ADR 0321's type, shared with the build).
pub use bynk_project::SchemaLock;
/// #2207: a workspace root groups projects and isn't one itself, so the
/// language server never analyses it as a project.
pub use bynk_project::is_workspace_root;

pub mod agent_effects;
/// #2220: the agent state model lives in `bynk-check`, shared with the
/// checker; re-exported here so existing paths keep working.
pub use bynk_check::agent_state;
pub mod architecture;
pub mod completion;
pub mod documentation;
pub mod locals_nav;
pub mod packages;
pub mod sequence;
pub mod signature_help;
pub mod symbols;
pub mod wire_contract;

/// One diagnostic produced from a recovery-mode compile of a single file.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub error: CompileError,
    pub severity: Severity,
    /// The file each of `error.labels` lies in, index for index, as
    /// [`bynk_project::AttributedError::label_files`] says — a label in a
    /// sibling file belongs to that file's URI, not this diagnostic's. Empty
    /// (every label in this file) for a single-file diagnosis.
    pub label_files: Vec<LabelFile>,
}

impl Diagnostic {
    /// The file `error.labels[index]` lies in.
    pub fn label_file(&self, index: usize) -> &LabelFile {
        self.label_files.get(index).unwrap_or(&LabelFile::Primary)
    }
}

/// Best-effort single-file compilation that always returns diagnostics.
///
/// Used by the LSP server: lex → parse-with-recovery → resolve → check, with
/// each phase accumulating its diagnostics. The returned `SourceUnit` is
/// `Some` whenever the parser produced one (which is true for any file with a
/// recognisable header, even if individual items failed). Resolve and check
/// run only when both the lexer and parser produced a unit; their errors are
/// added to the same diagnostic list.
///
/// The TypeScript output is intentionally not produced here — the LSP only
/// needs diagnostics; the CLI uses `compile` / `compile_project`.
pub fn diagnose(source: &str) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let tokens = match lexer::tokenize(source) {
        Ok(t) => t,
        Err(e) => {
            diagnostics.push(Diagnostic {
                severity: Severity::for_error(&e),
                error: e,
                label_files: Vec::new(),
            });
            return diagnostics;
        }
    };
    let parser::Recovered {
        units,
        errors: parse_errors,
        broken_decl_names,
    } = parser::parse_units_recovering(&tokens, source);
    let unit_opt = units.into_iter().next();
    for e in parse_errors {
        diagnostics.push(Diagnostic {
            severity: Severity::for_error(&e),
            error: e,
            label_files: Vec::new(),
        });
    }
    let Some(unit) = unit_opt else {
        return diagnostics;
    };
    // Resolution and checking are only well-defined for self-contained
    // commons units in single-file mode — contexts go through compile_project
    // which has the cross-file machinery. Match the same restriction here.
    if let ast::SourceUnit::Commons(c) = unit {
        let push = |diagnostics: &mut Vec<Diagnostic>, errs: Vec<CompileError>| {
            diagnostics.extend(errs.into_iter().map(|e| Diagnostic {
                severity: Severity::for_error(&e),
                error: e,
                label_files: Vec::new(),
            }));
        };
        // #1663 (Decision A): a resolve error no longer stops the checker. It
        // still checks every declaration; its diagnostics in a declaration the
        // resolver rejected are echoes and are dropped.
        let item_spans: Vec<_> = c.items.iter().map(|i| i.span()).collect();
        let (resolved, resolve_errors) = resolver::resolve_recovering(c);
        if resolve_errors.is_empty()
            && let Err(errs) = resolver::resolve_file(&resolved)
        {
            push(&mut diagnostics, errs);
        }
        // #1663 (Decision B): an unknown name that is a declaration the parser
        // skipped is not reported again; it still rejects its declaration.
        let (shown, _hidden) =
            resolver::split_broken_decl_echoes(resolve_errors.clone(), &broken_decl_names);
        push(&mut diagnostics, shown);
        // ADR 0117: a clean check may still carry non-failing warnings
        // (`Ok` now), so surface those too — not only the `Err` path.
        // #1710: the checker reports some echoes of a skipped declaration
        // under its own codes (a method, a capability, an actor), so its
        // diagnostics are split the same way.
        let unechoed = |errs| resolver::split_broken_decl_echoes(errs, &broken_decl_names).0;
        match checker::check(resolved) {
            Ok(typed) => push(&mut diagnostics, unechoed(typed.warnings)),
            Err(errs) => push(
                &mut diagnostics,
                unechoed(resolver::without_resolve_echoes(
                    errs,
                    &resolve_errors,
                    &item_spans,
                )),
            ),
        }
    }
    diagnostics
}

/// Per-file diagnostics from a whole-project analysis.
/// v0.24 (ADR 0052): `text` is the **analysed snapshot** — positions must
/// convert against it, not a newer buffer (the analyse→publish window is real).
pub struct FileDiagnostics {
    /// Project-root-relative source path.
    pub source_path: PathBuf,
    /// The exact text that was analysed (overlay or disk).
    pub text: String,
    pub diagnostics: Vec<Diagnostic>,
}

/// v0.24: the result of [`diagnose_project`]. Every discovered file appears
/// in `files` — clean files with an empty list — so a consumer can clear
/// stale diagnostics. `unattributed` holds project-level diagnostics with
/// no single owning file (group/cycle/directory validations).
pub struct ProjectDiagnostics {
    pub files: Vec<FileDiagnostics>,
    pub unattributed: Vec<Diagnostic>,
    /// v0.25 (ADR 0053): the project-wide binding index — every in-scope
    /// symbol's definition and reference sites, spans against the analysed
    /// snapshots in `files`.
    pub index: index::ProjectIndex,
    /// v0.27 (ADR 0056): per-file inferred-type inlay hints — `(binding-name
    /// span, label)`, span-ordered, spans against the analysed snapshots.
    pub hints: hints::FileHints,
    /// v0.30.2 (ADR 0063): per-file expression types — `(expr span, Ty)`,
    /// for `.`-member completion's receiver typing. A file with errors still
    /// carries the best-effort partial types of its other expressions
    /// (ADR 0094, which lifted ADR 0063's clean-file ceiling).
    pub expr_types: expr_types::FileExprTypes,
    /// T3.6b (R4.1): the intern table `expr_types`' `TyId`s resolve against —
    /// one per analysis, shared across every unit it checked.
    pub ty_intern: std::sync::Arc<bynk_check::checker::Types>,
    /// v0.31 (ADR 0064): per-file local bindings with scope ranges, for the
    /// scope-at-offset query backing locals completion + navigation.
    pub locals: locals::FileLocals,
    /// v0.99: per-file capability-requirement ledger — every capability-consuming
    /// site with its provenance, driving the ghost `given` inlay hint and hover.
    pub requirements: requirements::FileRequirements,
    /// Slice 6b (ADR 0095): qualified unit name → its project source file(s),
    /// in discovery order — the unit→file map backing document links and
    /// consumed-context navigation. Synthetic units excluded; empty on a bail.
    pub unit_sources: HashMap<String, Vec<PathBuf>>,
    /// #846: qualified context/adapter unit name → the cross-context/agent
    /// tables the sequence-diagram query classifies handler calls against.
    /// See `bynk_check::analysis::ProjectAnalysis::sequence_info`.
    pub sequence_info: HashMap<String, ContextSequenceInfo>,
    /// #855: qualified context/adapter unit name → the combined type table
    /// and service/agent tables the wire-contract peek resolves a handler's
    /// boundary shape and cross-context hash against.
    /// See `bynk_check::analysis::ProjectAnalysis::boundary_info`.
    pub boundary_info: HashMap<String, ContextBoundaryInfo>,
    /// #848: qualified unit name → its doc-comment intra-doc-link search
    /// order — itself first, then its `uses` targets, then its `consumes`
    /// targets. See `bynk_check::analysis::ProjectAnalysis::doc_scope`.
    pub doc_scope: HashMap<String, Vec<String>>,
}

/// Slice A: which trees a project's analysis walks.
///
/// `bynk-ide` owns this rather than re-exporting `bynk_project::Roots`:
/// this crate is the IDE-facing published surface, and `Roots` carries
/// `tests_prefix` semantics an IDE caller has no business knowing. The lowering
/// is a few lines and it is the seam where the LSP's needs and the compiler's
/// can diverge later without a break.
#[derive(Debug, Clone)]
pub enum AnalysisRoots {
    /// One tree, walked as a single root, with no manifest consulted — the
    /// pre-slice-A behaviour and what [`diagnose_project`] still means.
    SingleTree(PathBuf),
    /// A manifest-backed project rooted here: `bynk.toml`'s `[paths]
    /// include`/`exclude` decide the trees, exactly as `bynkc` reads them.
    /// Mirrors `bynk-driver`'s `project_options` — the compiler's own choice.
    Project(PathBuf),
}

impl AnalysisRoots {
    /// Content-ownership track (#1086) slice 2: `overlay` threads into
    /// `try_read_project_paths_with` (already overlay-aware, already `pub`)
    /// instead of the disk-only `read_project_paths`, so an unsaved edit to
    /// `bynk.toml` itself — not just to the `.bynk` sources it names — is
    /// visible to the `Project` variant's manifest read. Mirrors `343b2482`'s
    /// CLI-side fix (`bynk-driver`'s `project_options`) on the LSP side.
    ///
    /// `discover_files` (below) threads `overlay` straight through now (slice
    /// 5 correction, below) — both it and `diagnose_project_with` need the
    /// same real-or-overlaid `bynk.toml` content to resolve the same roots.
    ///
    /// Content-ownership track (#1086) slice 5 correction (found only under
    /// implementation): reading `bynk.toml` through `overlay` alone used to
    /// still reach the real on-disk manifest on a miss, because `bynk-emit`'s
    /// `read_source` fell back to a real disk read. Slice 5 deleted that
    /// fallback — but R2.3 forbids this crate from touching the filesystem
    /// itself to restore it here, so it does **not** grow a fallback of its
    /// own; every caller (`bynk-lsp`'s `sweep_project_content`,
    /// `bynk-testkit::read_project_sources`) now reads `bynk.toml` itself,
    /// above the driver boundary, and includes it in the `overlay`/content
    /// map it hands in — the same "process edge constructs `Sources`" R2.3
    /// already requires of every `.bynk` file. `lower` stays exactly what its
    /// slice-2 doc above describes: caller's overlay, nothing else.
    fn lower(&self, overlay: &HashMap<PathBuf, String>) -> bynk_project::Roots {
        match self {
            AnalysisRoots::SingleTree(root) => bynk_project::Roots::Single(root.clone()),
            AnalysisRoots::Project(root) => bynk_project::Roots::Split {
                project_root: root.clone(),
                paths: bynk_project::try_read_project_paths_with(root, overlay)
                    .unwrap_or_else(|_| bynk_project::ProjectPaths::conventional(root)),
            },
        }
    }

    /// The project root every analysed `source_path` is relative to. For
    /// `SingleTree` that is the tree itself (identity ≡ tree-relative, ADR
    /// 0198).
    pub fn project_root(&self) -> &Path {
        match self {
            AnalysisRoots::SingleTree(r) | AnalysisRoots::Project(r) => r,
        }
    }
}

/// Slice A: the `.bynk` files these roots contain — the same discovery
/// `compile_project` performs, `exclude` and the `out`/`node_modules` caches
/// honoured. For enumerating a project's units without analysing it.
///
/// `overlay` is consulted only for `bynk.toml` itself (via `lower`) — an
/// `AnalysisRoots::Project`'s `[paths] include`/`exclude` decide what gets
/// walked, so the caller must include a real-or-overlaid manifest entry for
/// a non-conventional layout to enumerate correctly (content-ownership
/// track (#1086) slice 5: this crate stays disk-free per R2.3, so it cannot
/// fall back to reading `bynk.toml` itself on a miss).
pub fn discover_files(roots: &AnalysisRoots, overlay: &HashMap<PathBuf, String>) -> Vec<PathBuf> {
    bynk_project::discover_project_files(&roots.lower(overlay))
}

/// #302: the qualified name a file moved from `old_rel` to `new_rel` should
/// now declare, preserving whichever single-file/multi-file arrangement
/// `old_rel` used to satisfy against `old_name` — for the LSP's
/// `workspace/willRenameFiles` handler.
pub fn renamed_unit_name(old_rel: &Path, old_name: &str, new_rel: &Path) -> Option<String> {
    bynk_project::renamed_unit_name(old_rel, old_name, new_rel)
}

/// v0.24 (ADR 0052): non-bailing, overlay-aware, file-attributed project
/// diagnostics — the LSP analysis entry point, distinct from
/// `compile_project` (which bails and emits). `overlay` maps
/// canonicalised absolute paths to buffer text layered over disk reads.
///
/// Slice A: this is the **single-tree convenience** over
/// [`diagnose_project_with`] — it walks `root` as one tree and consults no
/// manifest, which is what every caller handing in a fixture root already
/// means. A manifest-backed project wants
/// `diagnose_project_with(&AnalysisRoots::Project(root), …)`.
///
/// #2081: it passes [`SchemaLock::Off`], so it never reconciles events
/// against `bynk.schema.lock`. Every caller is a test or a test kit; the
/// language server calls [`diagnose_project_with`] with the lock it read.
pub fn diagnose_project(root: &Path, overlay: &HashMap<PathBuf, String>) -> ProjectDiagnostics {
    diagnose_project_with(
        &AnalysisRoots::SingleTree(root.to_path_buf()),
        overlay,
        &SchemaLock::Off,
    )
}

/// Slice A: project diagnostics over manifest-resolved roots — the LSP analyses
/// exactly the files `bynkc` compiles, from the same manifest, through the same
/// discovery.
///
/// Every path in the result is **project-relative** (ADR 0198), so a file is
/// named uniquely across `include` roots.
///
/// `schema_lock` (#2081) is the project's `bynk.schema.lock`, read by the
/// caller (this crate reads no disk): under [`SchemaLock::On`] events are
/// reconciled against it, read-only, so a non-additive change is reported as
/// `bynk check` reports it; [`SchemaLock::Off`] skips that.
pub fn diagnose_project_with(
    roots: &AnalysisRoots,
    overlay: &HashMap<PathBuf, String>,
    schema_lock: &SchemaLock,
) -> ProjectDiagnostics {
    // #43: destructured (not `analysis.field`-by-field) so that a field added
    // to `ProjectAnalysis` without a matching update *here* is a compile
    // error (an unmentioned field in a struct pattern), not a silently
    // stale `ProjectDiagnostics` — the fields below are one-for-one
    // identical between the two types precisely because this site can't
    // forget one.
    let bynk_check::analysis::ProjectAnalysis {
        snapshots,
        errors,
        index,
        hints,
        expr_types,
        ty_intern,
        locals,
        requirements,
        unit_sources,
        sequence_info,
        boundary_info,
        doc_scope,
    } = bynk_check::analysis::analyse_project(&roots.lower(overlay), overlay, schema_lock);
    let mut by_file: HashMap<PathBuf, Vec<Diagnostic>> = HashMap::new();
    let mut unattributed = Vec::new();
    for ae in errors {
        let d = Diagnostic {
            severity: Severity::for_error(&ae.error),
            error: ae.error,
            label_files: ae.label_files,
        };
        match ae.source_path {
            Some(p) => by_file.entry(p).or_default().push(d),
            None => unattributed.push(d),
        }
    }
    let files = snapshots
        .into_iter()
        .map(|(source_path, text)| FileDiagnostics {
            diagnostics: by_file.remove(&source_path).unwrap_or_default(),
            source_path,
            text,
        })
        .collect();
    // Anything attributed to a path without a snapshot (defensive — should
    // not happen) still surfaces rather than vanishing.
    for (_, ds) in by_file {
        unattributed.extend(ds);
    }
    ProjectDiagnostics {
        files,
        unattributed,
        index,
        hints,
        requirements,
        expr_types,
        ty_intern,
        locals,
        unit_sources,
        sequence_info,
        boundary_info,
        doc_scope,
    }
}

#[cfg(test)]
mod testkit {
    //! In-process test helpers for this crate's own inline `#[cfg(test)]`
    //! modules (content-ownership track, #1086, slice 4).
    //!
    //! This crate cannot depend on the cross-crate `bynk-testkit` (that crate
    //! depends on `bynk-ide`, so a dev-dependency back onto it would cycle —
    //! Cargo would instantiate two separate copies of this very crate, making
    //! `crate::AnalysisRoots` and `bynk_ide::AnalysisRoots` distinct,
    //! non-interchangeable types; found the hard way in slice 3, not
    //! foreseen). So this mirrors `bynk-testkit::read_project_sources`
    //! directly against `crate::discover_files`, the same production
    //! discovery both use.
    //!
    //! An inline module, not `bynk-ide/src/testkit.rs` as a separate
    //! `#[cfg(test)]`-gated file: `fs_below_driver`'s probe
    //! (`xtask/src/greenfield_status.rs`) only recognises an inline
    //! `#[cfg(test)] mod name { … }` block as test-scope — a whole file gated
    //! by its *declaration* (`#[cfg(test)] mod testkit;` in a different file)
    //! isn't a pattern it looks for, so a separate file's `std::fs` calls read
    //! as production usage and moved `fs_below_driver`'s `bynk-ide` count
    //! from 0 back to 1 (found by running `cargo xtask greenfield-status`
    //! after the first version of this module, not anticipated).

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    /// A complete `(path, content)` map for `roots` — the direct replacement
    /// for `diagnose_project(&root, &HashMap::new())`'s reliance on
    /// `bynk-emit`'s disk fallback. Keyed by the literal discovered path, not
    /// canonicalised — matching `bynk-testkit`'s own convention, itself
    /// matching `bynk-driver`'s production `sources_for_roots` (canonicalising
    /// broke a project-consistency check the hard way in slice 3 — see
    /// `bynk-testkit/src/lib.rs`'s doc).
    pub(crate) fn read_project_sources(roots: &crate::AnalysisRoots) -> HashMap<PathBuf, String> {
        // Only ever called with `SingleTree` (below), which consults no
        // manifest — an empty overlay is correct, not a stand-in fallback.
        crate::discover_files(roots, &HashMap::new())
            .into_iter()
            .filter_map(|p| {
                let content = std::fs::read_to_string(&p).ok()?;
                Some((p, content))
            })
            .collect()
    }

    /// `diagnose_project(root, &HashMap::new())`, with a complete sources map
    /// instead of relying on `bynk-emit`'s disk fallback to fill it in.
    pub(crate) fn diagnose_project(root: &Path) -> crate::ProjectDiagnostics {
        let sources = read_project_sources(&crate::AnalysisRoots::SingleTree(root.to_path_buf()));
        crate::diagnose_project(root, &sources)
    }
}
