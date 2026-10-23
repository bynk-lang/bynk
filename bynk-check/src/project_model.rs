//! Project-wide orchestration: discovery → parse → group → resolve, shared
//! between `bynk-emit`'s `run_checks` (both `Mode::Build` and `Mode::Analyse`)
//! and this crate's own [`crate::analysis::analyse_project`].
//!
//! P4.1 (#1115), second scope finding on the tracking issue: this pipeline —
//! `phase_discovery` through `assemble_unit_info`, plus the per-unit symbol
//! composition (`compose_unit_symbols`/`merge_consumed_exports`/
//! `collect_unit_methods`) — used to live only in `bynk-emit/src/project.rs`,
//! inline in `run_checks`. A literal no-indirection `bynk-check`-side analysis
//! entry point needs the identical sequence, so rather than write a second,
//! independently-maintained copy (the mistake this whole design track's
//! `extract, don't duplicate` principle exists to prevent — see
//! `lower_field_default_wire`, `build_capability_op_info` for the same move
//! made earlier in this track), it moved here. `bynk-emit`'s `run_checks`
//! becomes a caller of these functions instead of owning the logic, the same
//! way P4.0 turned `project.rs` into a caller of `bynk-project`.
//!
//! What stayed in `bynk-emit` (not shared, because only the `Mode::Build` path
//! needs it, or because it's genuinely emission-shaped): the `Mode::Build`
//! bail gate and everything from emission onward (`EmitUnitCtx`, `emit_unit`,
//! `collect_history_target_agents`). The whole-project `messages`/locale-
//! ambiguity/event-subscription checks (P5.0/P5.1), the function-type-
//! boundary check (P5.2, [`phase_function_type_boundaries`]), and
//! schema-registry reconciliation/platform-lock enforcement (P5.3,
//! [`crate::schema_registry::reconcile`]/[`phase_platform_lock`]) have since
//! moved here too — the P5.2 move closed `phase_group`'s optional
//! boundary-check hook, which used to be the only way `run_checks` and the
//! new entry point could reach it without duplicating the diagnostic-ordering
//! logic (see `analysis.rs` for the residual-gap accounting that remains).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use crate::checker::{self, Ty, TyId, Types};
use crate::context_checks::{
    build_capability_op_info, reject_fn_types, ts_type_ref_display, type_ref_is_held,
    type_ref_to_display, validate_store_field_value_types,
};
use crate::firstparty::{self, Platform};
use crate::icu;
use crate::index::{RefSink, SymbolKind};
use crate::resolver::MethodTable as ResolverMethodTable;
use crate::symbols::{
    ConsumedType, ContextMessageBundle, FileDeclIndex, UnitTable, build_file_decl_index,
    build_unit_table, consumes_span_of, detect_context_message_bundle, parsed_alias_span,
    uses_span_of,
};
use bynk_project::{
    AttributedError, LabelFile, ParsedFile, UnitKind, check_directory_kind_consistency,
    check_directory_name_consistency, check_file_directory_conflicts, check_group_kind_consistency,
    check_group_module_docs, check_path_name_alignment, detect_consumes_cycles,
    discover_bynk_files, is_unpinned_range, normalize_rel, parse_sources, parse_sources_recovering,
    read_adapter_binding, read_adapter_include, read_source,
};
use bynk_syntax::ast::*;
/// P6.49 (design/tracks/the-ir.md §6b), following P6.27's `ExprId` precedent
/// (`checker.rs:39`): re-exported because this module's own public API —
/// [`compose_unit_symbols`]'s `combined_types`/`combined_fns`,
/// [`collect_unit_methods`]'s return, and [`UnitInfo::exports`]'s value type
/// — is already parameterised by these three types. `bynk-emit` only ever
/// plumbs the resulting tables through to other `bynk-check` calls; it never
/// matches a variant of any of the three.
pub use bynk_syntax::ast::{FnDecl, TypeDecl, Visibility};
use bynk_syntax::error::CompileError;
use bynk_syntax::lexer;
use bynk_syntax::parser;
use bynk_syntax::span::{FileId, Span};

/// Collection-point error sink (ADR 0052). Helpers keep their plain
/// `&mut Vec<CompileError>` signatures; call sites attribute via
/// `extend_for` with the file in scope at that point.
///
/// P4.1 (#1115): relocated from `bynk-emit/src/project/diagnostics.rs`
/// alongside the `phase_*` functions above, which all take `&mut ErrorSink` —
/// the same "shared logic pulls its own types down with it" pattern already
/// applied to `UnitTable`/`ConsumedType` in the `symbols.rs` move. `Mode` and
/// `ProjectFailure` (the other two `diagnostics.rs` pipeline-driving types)
/// stayed in `bynk-emit`, unaffected — neither is a dependency of anything
/// this module needs.
pub struct ErrorSink {
    entries: Vec<AttributedError>,
    /// v0.89 (ADR 0117): non-failing warnings, classified on push by
    /// `Severity::for_error`. Kept apart so `is_empty`/`len` — the build-failure
    /// gates — stay errors-only, while every warning source (commons-fn checks,
    /// service/agent handler validation, parser) is captured uniformly.
    warnings: Vec<AttributedError>,
    /// The identity path of every parsed project file, by the `FileId` its
    /// spans carry ([`Self::register_file`]). Read when the sink is drained,
    /// to say which file each secondary label lies in (`label_files`).
    files: HashMap<FileId, PathBuf>,
}

impl Default for ErrorSink {
    fn default() -> Self {
        Self::new()
    }
}

impl ErrorSink {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            warnings: Vec::new(),
            files: HashMap::new(),
        }
    }
    /// Record that spans carrying `file` lie in the project file `identity`.
    pub fn register_file(&mut self, file: FileId, identity: PathBuf) {
        self.files.insert(file, identity);
    }
    /// Register every file of `parsed` that was read from disk, by the durable
    /// `FileId` its absolute path interns to (`parse_cache::file_id_for`) — the
    /// one map both an error's file ([`Self::extend_attributed_by_span`]) and
    /// each label's (when the sink is drained) are read from, so the two never
    /// disagree. `phase_parse` registers every file it parses; a
    /// toolchain-injected unit, with no absolute path, is never registered.
    pub fn register_parsed(&mut self, parsed: &[ParsedFile]) {
        for pf in parsed {
            if let Some(abs) = pf.abs_path() {
                self.register_file(
                    bynk_project::parse_cache::file_id_for(&abs),
                    pf.identity_path(),
                );
            }
        }
    }
    pub fn push_for(&mut self, file: Option<&Path>, mut error: CompileError) {
        // #2223: a dependency unit's canonical name reads as
        // `org/package:unit` wherever a diagnostic names it.
        display_canonical_names(&mut error);
        let attributed = AttributedError::new(file.map(Path::to_path_buf), error);
        match bynk_syntax::Severity::for_error(&attributed.error) {
            bynk_syntax::Severity::Warning => self.warnings.push(attributed),
            bynk_syntax::Severity::Error => self.entries.push(attributed),
        }
    }
    pub fn extend_for(
        &mut self,
        file: Option<&Path>,
        errs: impl IntoIterator<Item = CompileError>,
    ) {
        for e in errs {
            self.push_for(file, e);
        }
    }
    /// #1659 (closes #696's gap): attribute each error to the parsed file its
    /// span is in, by `FileId`. Every parsed file's spans carry the durable
    /// `FileId` its absolute path interns to (`parse_cache::file_id_for`), so
    /// the owning file is recoverable from the span alone. Suite and
    /// integration diagnostics were pushed unattributed, because threading a
    /// file through their many internal push sites was deferred, and rendered
    /// with no file, line or span. An error whose span matches no parsed file
    /// stays unattributed, as before. `parsed` is registered first
    /// ([`Self::register_parsed`]), so a file `phase_parse` never saw is still
    /// found.
    pub fn extend_attributed_by_span(
        &mut self,
        parsed: &[ParsedFile],
        errs: impl IntoIterator<Item = CompileError>,
    ) {
        self.register_parsed(parsed);
        for e in errs {
            let file = self.files.get(&e.span.file).cloned();
            self.push_for(file.as_deref(), e);
        }
    }
    /// True when no **error-severity** diagnostic has been collected — the
    /// build-failure gate. Warnings do not count (ADR 0117).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Consume the sink, yielding the non-failing **warnings** (ADR 0117).
    pub fn into_warnings(self) -> Vec<AttributedError> {
        let mut warnings = self.warnings;
        attribute_labels(&self.files, &mut warnings);
        warnings
    }
    /// Consume the sink, yielding errors then warnings — the full diagnostic
    /// list the LSP and a failed build render together.
    pub fn into_all(self) -> Vec<AttributedError> {
        let mut all = self.entries;
        all.extend(self.warnings);
        attribute_labels(&self.files, &mut all);
        all
    }
    /// The count of **error-severity** diagnostics.
    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Fill in each error's `label_files` from the `FileId` its labels' spans
/// carry, against the files `phase_parse` registered. A label is in the
/// error's own file when its span has the primary span's `FileId`, carries no
/// file identity (`FileId::UNKNOWN`, a span built without one), or names the
/// error's own `source_path`. One naming another registered file is
/// [`LabelFile::Other`]; one naming a toolchain-injected unit's reserved id is
/// [`LabelFile::FirstParty`] (#1956), carrying that unit's own source; any other
/// is [`LabelFile::Unavailable`]. Neither is ever drawn against the primary
/// file's text.
fn attribute_labels(files: &HashMap<FileId, PathBuf>, errors: &mut [AttributedError]) {
    for ae in errors {
        let primary = ae.error.span.file;
        ae.label_files = ae
            .error
            .labels
            .iter()
            .map(|(span, _)| {
                if span.file == primary || span.file == FileId::UNKNOWN {
                    return LabelFile::Primary;
                }
                match files.get(&span.file) {
                    Some(path) if Some(path) == ae.source_path.as_ref() => LabelFile::Primary,
                    Some(path) => LabelFile::Other(path.clone()),
                    None => match firstparty_unit_for(span.file) {
                        Some(unit) => LabelFile::FirstParty {
                            identity_path: unit.identity_path,
                            source: unit.source,
                        },
                        None => LabelFile::Unavailable,
                    },
                }
            })
            .collect();
    }
}

/// v0.17: a resolved adapter binding — the user-authored `.binding.ts` module
/// that supplies an adapter's external provider symbols. Copied verbatim into
/// the output beside the adapter's emitted interface module so that `tsc`
/// checks the `implements` contract and compose can import the symbols.
pub struct AdapterBinding {
    /// Output path, relative to the output root (e.g. `tokens.binding.ts`).
    pub output_path: PathBuf,
    /// Verbatim TypeScript content read from the source tree.
    pub content: String,
    /// #2321: the files the `binding … includes [ … ]` clause ships beside it.
    pub includes: Vec<IncludedFile>,
}

/// #2321: one file an adapter binding includes, copied byte for byte.
#[derive(Clone)]
pub struct IncludedFile {
    /// Output path, relative to the output root (beside the binding).
    pub output_path: PathBuf,
    pub bytes: Vec<u8>,
}

/// The build target. Determines how cross-context calls and per-context
/// modules are emitted (v0.8). Bundle mode is the default — all contexts
/// emit into one TypeScript bundle and cross-context calls are direct
/// function invocations. Workers mode produces per-context Cloudflare
/// Worker bundles that communicate via Service Bindings.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum BuildTarget {
    /// Existing behaviour: one TS bundle, direct function calls between
    /// contexts.
    #[default]
    Bundle,
    /// One Worker per context. Cross-context calls become Service Binding
    /// invocations using a JSON wire format with refinement validation on
    /// the receiving side.
    Workers,
}

pub fn normalize_service_defaults(parsed: &mut [ParsedFile]) {
    for pf in parsed.iter_mut() {
        let items = match pf.unit_mut() {
            SourceUnit::Commons(c) => &mut c.items,
            SourceUnit::Context(c) => &mut c.items,
            SourceUnit::Adapter(a) => &mut a.items,
            SourceUnit::Suite(_) => continue,
        };
        for item in items.iter_mut() {
            if let CommonsItem::Service(svc) = item {
                inject_service_defaults(svc);
            }
        }
    }
}

/// Inject a single service's `by`/`given` defaults into its handlers. A handler
/// that names its own `by` (or `given`) overrides the default outright — the
/// default fills only an *absent* clause, never merges. A service with no default
/// is left untouched (byte-for-byte the pre-v0.155 behaviour).
pub fn inject_service_defaults(svc: &mut ServiceDecl) {
    let default_by = svc.default_by.clone();
    let default_given = svc.default_given.clone();
    if default_by.is_none() && default_given.is_empty() {
        return;
    }
    for handler in svc.handlers.iter_mut() {
        // #2341 (decision C): `on rejected` runs for a request no handler took,
        // before any actor is known, and uses no capability.
        if handler.kind == HandlerKind::Rejected {
            continue;
        }
        if handler.by_clause.is_none()
            && let Some(def) = &default_by
        {
            handler.by_clause = Some(def.clone());
        }
        if handler.given.is_empty() && !default_given.is_empty() {
            handler.given = default_given.clone();
        }
    }
}

/// Phase 1: discover the `.bynk` files under the source (and, in split mode,
/// the tests) root by walking the filesystem. Pushes any discovery error into
/// `errors` and signals a pipeline bail via `Err(())` (the caller terminates
/// with `finish`); otherwise returns the discovered `(src_files, tests_files)`.
///
/// #1077/#1081 review: this is the on-disk half only — `no_sources`/
/// `check_file_directory_conflicts` moved to [`check_discovered_files`], which
/// `run_checks` calls on the result *either* this walk *or* a caller-supplied
/// `discovered` list produces, so a `CompileOptions.sources`-driven compile
/// (the CLI's own path as of #1081) still gets both checks — they are
/// properties of "what files does this build have," not of having just
/// walked the disk to find them.
/// R3.9 (#1113): walks every `(root, prefix)` tree `Roots::trees` resolves
/// to, not a hardcoded primary/secondary pair — `trees[0]` is the mandatory
/// tree (a missing directory is a real error, via `discover_bynk_files`
/// itself); every later tree is optional, same as the old secondary tree
/// always was (a project may simply have no such subtree).
#[allow(clippy::result_unit_err)]
pub fn phase_discovery(
    trees: &[(PathBuf, PathBuf)],
    excludes: &[PathBuf],
    errors: &mut ErrorSink,
) -> Result<Vec<Vec<PathBuf>>, ()> {
    let mut out = Vec::with_capacity(trees.len());
    for (i, (root, _prefix)) in trees.iter().enumerate() {
        match discover_bynk_files(root, excludes) {
            Ok(f) => out.push(f),
            // Every tree past the first is optional — a missing directory is
            // not an error, same as the old secondary tree always was. Tried
            // via `discover_bynk_files` itself (a `fs::read_dir`) rather than
            // a `root.exists()` pre-check, which would cost a redundant
            // `stat()` per optional tree for the same answer.
            Err(e) if i > 0 && e.category == "bynk.project.no_root" => {
                out.push(Vec::new());
            }
            Err(e) => {
                errors.push_for(None, e);
                return Err(());
            }
        }
    }
    Ok(out)
}

/// The checks every tree's file list must pass regardless of where it came
/// from — a real disk walk ([`phase_discovery`]) or a caller-supplied
/// `discovered`/`CompileOptions.sources` list (#1077/#1081 review). An empty
/// project (`bynk.project.no_sources`) signals a bail via `Err(())`; a
/// file/directory name conflict is a non-fatal diagnostic.
#[allow(clippy::result_unit_err)]
pub fn check_discovered_files(
    trees: &[(PathBuf, PathBuf)],
    file_lists: &[Vec<PathBuf>],
    errors: &mut ErrorSink,
) -> Result<(), ()> {
    if file_lists.iter().all(|f| f.is_empty()) {
        errors.push_for(
            None,
            CompileError::new(
                "bynk.project.no_sources",
                Span::default(),
                format!(
                    "no `.bynk` source files found under {}",
                    trees[0].0.display()
                ),
            ),
        );
        return Err(());
    }
    for ((root, _prefix), files) in trees.iter().zip(file_lists.iter()) {
        if let Err(e) = check_file_directory_conflicts(root, files) {
            errors.extend_for(None, e);
        }
    }
    Ok(())
}

/// A memoized parse of one first-party synthetic source, keyed by the
/// caller's `cache` — `inject_firstparty` passes one per registry entry.
/// Finding #55/#65: the source text is a fixed `include_str!` constant, so its
/// parse is a pure function of that constant and only needs computing once per
/// process, not once per compile/analyse round. Which units a project injects
/// is still decided fresh for every project from its own parsed
/// `uses`/`consumes`; only the parse *result* is cached.
/// T3.4 (R2.4): each first-party synthetic unit reserves its own 1M-wide
/// `ExprId` block, spaced far above anything a real project's own file count
/// could ever reach — see [`firstparty_parsed`]'s doc comment for why a fixed
/// reservation, not a threaded counter, is the right shape here.
pub const FIRSTPARTY_ID_BLOCK: u32 = 1_000_000;
pub const FIRSTPARTY_ID_BASE: u32 = 1_000_000_000;

/// #1956: the first `FileId` reserved for first-party synthetic units. Each
/// unit's spans carry `FIRSTPARTY_FILE_ID_BASE + id_block` ([`firstparty_file_id`]),
/// fixed for the same reason as its `ExprId` block: the parse is cached per
/// process, so the id can't come from any one project's interner. The range
/// sits just below `FileId::UNKNOWN` (`u32::MAX`), far above any id
/// `parse_cache::file_id_for` hands a real file. Without an identity of their
/// own, a first-party span counted as the error's own file, and a "declared
/// here" label into `bynk.bynk` was drawn at the same offsets in the user's.
pub const FIRSTPARTY_FILE_ID_BASE: u32 = 0xFFFF_0000;

/// The `FileId` every span of first-party `unit` carries.
pub fn firstparty_file_id(unit: &firstparty::Unit) -> FileId {
    FileId(FIRSTPARTY_FILE_ID_BASE + unit.id_block)
}

/// The first-party unit whose spans carry `file`, if any.
fn firstparty_unit_for(file: FileId) -> Option<&'static firstparty::Unit> {
    firstparty::UNITS
        .iter()
        .find(|unit| firstparty_file_id(unit) == file)
}

pub fn firstparty_parsed(
    cache: &'static OnceLock<Result<ParsedFile, Vec<CompileError>>>,
    identity_path: &'static str,
    src: &'static str,
    kind: UnitKind,
    // T3.4 (R2.4): a fixed base, not a live project counter — `cache` is a
    // `OnceLock`, parsed once per *process*, and reused as-is across every
    // later compile in that process regardless of how many real files that
    // *particular* compile happens to have. A threaded counter can't work
    // here (this parse doesn't know, and must never depend on, which compile
    // triggers it first); a fixed, permanently-reserved range that no real
    // project could ever grow into does. Call sites space their bases
    // `FIRSTPARTY_ID_BLOCK` apart, one block per registry entry
    // (`firstparty::Unit::id_block`), so first-party units can never collide
    // with each other either, however many of them one project injects.
    id_base: u32,
    // #1956: the unit's reserved `FileId` ([`firstparty_file_id`]), stamped on
    // every span so a label into it is never taken for the user's file.
    file: FileId,
) -> Result<ParsedFile, Vec<CompileError>> {
    cache
        .get_or_init(|| {
            lexer::tokenize_in(src, file)
                .map_err(|e| vec![e])
                .and_then(|toks| {
                    parser::parse_unit_with_warnings_from(&toks, src, &mut { id_base })
                        .map(|(unit, _warnings)| unit)
                })
                .map(|unit| {
                    ParsedFile::synthetic(
                        PathBuf::from(identity_path),
                        PathBuf::from(identity_path),
                        src.to_string(),
                        unit,
                        kind,
                    )
                })
        })
        .clone()
}

/// #1663: every syntax error in `source`, from a recovering parse, when the
/// strict parse failed with `strict` (its first), merged with it
/// ([`bynk_syntax::parser::merge_syntax_errors`]). A lex error has no recovery.
fn all_syntax_errors(source: &str, strict: Vec<CompileError>) -> Vec<CompileError> {
    let Ok(tokens) = bynk_syntax::lexer::tokenize(source) else {
        return strict;
    };
    let recovered = bynk_syntax::parser::parse_units_recovering(&tokens, source).errors;
    bynk_syntax::parser::merge_syntax_errors(strict, recovered)
}

/// Phase 2: parse every discovered file into a `ParsedFile`, recording each
/// file's source text into `snapshots` and any parse errors into `errors`.
/// Then inject the first-party units the project consumes or uses
/// (`inject_firstparty`). Returns the parsed units, the registry entries
/// injected, and (#1710) each unit's [`BrokenDeclNames`]; signals a pipeline
/// bail via `Err(())` when parsing produced errors and yielded no units at all.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::result_unit_err)]
pub fn phase_parse(
    // R3.9 (#1113): one `(root, prefix)` pair per `Roots::trees` entry, not a
    // hardcoded primary/secondary pair — every `include` tree is walked.
    trees: &[(PathBuf, PathBuf)],
    file_lists: &[Vec<PathBuf>],
    overlay: &HashMap<PathBuf, String>,
    errors: &mut ErrorSink,
    snapshots: &mut Vec<(PathBuf, String)>,
) -> Result<(Vec<ParsedFile>, InjectedUnits, BrokenDeclNames), ()> {
    let mut parsed: Vec<ParsedFile> = Vec::new();
    let mut broken: BrokenDeclNames = HashMap::new();
    // P8.4 (#1515): `FileId`/`ExprId` allocation is no longer threaded through
    // this function — `parse_sources` now resolves both through
    // `bynk_project::parse_cache`'s own durable, process-lifetime counters
    // (see that module's own doc comment, [DECISION D]), which trivially
    // keeps the old within-one-call uniqueness guarantee T3.4/T3.5 named
    // (global uniqueness implies uniqueness within any one call).
    let parse_tree = |root: &Path,
                      prefix: &Path,
                      files: &[PathBuf],
                      parsed: &mut Vec<ParsedFile>,
                      broken: &mut BrokenDeclNames,
                      errors: &mut ErrorSink,
                      snapshots: &mut Vec<(PathBuf, String)>| {
        for path in files {
            // Tree-relative: what unit validation reads.
            let rel = path.strip_prefix(root).unwrap_or(path).to_path_buf();
            // Slice 0 — project-relative: what *names* the file. Equal to `rel`
            // for a single root (empty prefix).
            let id = prefix.join(&rel);
            let source = match read_source(path, overlay) {
                Ok(s) => s,
                Err(e) => {
                    errors.push_for(
                        Some(&id),
                        CompileError::new(
                            "bynk.project.read_failed",
                            Span::default(),
                            format!("could not read `{}`: {e}", path.display()),
                        ),
                    );
                    continue;
                }
            };
            snapshots.push((id.clone(), source.clone()));
            match parse_sources(root, prefix, path, source) {
                Ok((pfs, warnings)) => {
                    parsed.extend(pfs);
                    // ADR 0117: the sink classifies these as warnings — they
                    // surface with the build but never gate it.
                    errors.extend_for(Some(&id), warnings);
                }
                // #1663: the strict parse stops at a file's first syntax
                // error. Report every one, from a recovering parse. #1710: the
                // declarations that recovery kept are checked too, with the
                // ones it skipped as known names (Decision B), so the file's
                // other faults aren't hidden behind its syntax error. Nothing
                // is emitted: the syntax errors already fail the build.
                Err(errs) => {
                    let source = snapshots.last().expect("pushed just above").1.clone();
                    match parse_sources_recovering(root, prefix, path, source.clone()) {
                        Some(recovered) => {
                            errors.extend_for(
                                Some(&id),
                                bynk_syntax::parser::merge_syntax_errors(errs, recovered.errors),
                            );
                            // The parser records skipped names per file, so
                            // each unit the file declares (a `commons` beside
                            // its `suite`, say) gets them all: wider, but only
                            // within the one file.
                            for pf in &recovered.files {
                                broken
                                    .entry(pf.unit().name().joined())
                                    .or_default()
                                    .extend(recovered.broken_decl_names.iter().cloned());
                            }
                            parsed.extend(recovered.files);
                        }
                        None => errors.extend_for(Some(&id), all_syntax_errors(&source, errs)),
                    }
                }
            }
        }
    };
    for ((root, prefix), files) in trees.iter().zip(file_lists.iter()) {
        parse_tree(
            root,
            prefix,
            files,
            &mut parsed,
            &mut broken,
            errors,
            snapshots,
        );
    }
    errors.register_parsed(&parsed);
    if !errors.is_empty() && parsed.is_empty() {
        return Err(());
    }

    let injected = inject_firstparty(&mut parsed, errors);
    Ok((parsed, injected, broken))
}

/// The first-party units a project injected, as registry entries in ascending
/// [`firstparty::Unit::id_block`] order. A unit whose source failed to parse is
/// still listed, so its binding is still supplied. Its own `uses`/`consumes`
/// cannot be read, though, so the units only it reaches are not injected: a
/// malformed first-party source (which `every_first_party_source_parses` rejects
/// in CI) reports its parse error and may add unresolved-name errors for those.
pub type InjectedUnits = Vec<&'static firstparty::Unit>;

/// Inject the first-party units the project reaches (standard-library S1,
/// #1945). A unit in the [`firstparty::UNITS`] registry is reached when a parsed
/// unit `consumes` it (an adapter) or `uses` it (a commons). The units it reaches
/// in turn are its own source's `uses`/`consumes` lines, so injection repeats
/// until nothing new is reached: the dependencies between first-party units
/// (`bynk.map` uses `bynk.list`; the `bynk` adapter uses `bynk.locale.types`)
/// come from their sources, never a hand-kept list. Each injected unit flows
/// through the normal pipeline (tables, exports, emission, compose), and an
/// adapter's binding is supplied by the toolchain (§4.2). Units are pushed in
/// [`firstparty::Unit::id_block`] order, whatever order they were reached in.
fn inject_firstparty(parsed: &mut Vec<ParsedFile>, errors: &mut ErrorSink) -> InjectedUnits {
    const N: usize = firstparty::UNITS.len();
    static CACHES: [OnceLock<Result<ParsedFile, Vec<CompileError>>>; N] =
        [const { OnceLock::new() }; N];
    let reaches = |pf: &ParsedFile, unit: &firstparty::Unit| match unit.kind {
        firstparty::UnitKind::Adapter => {
            pf.consumes().iter().any(|c| c.target.joined() == unit.name)
        }
        firstparty::UnitKind::Commons => pf.uses().iter().any(|u| u.target.joined() == unit.name),
    };
    let mut reached: Vec<Option<Option<ParsedFile>>> = vec![None; N];
    loop {
        let mut grew = false;
        for (i, unit) in firstparty::UNITS.iter().enumerate() {
            if reached[i].is_some() {
                continue;
            }
            let injected = reached.iter().flatten().flatten();
            if !parsed.iter().chain(injected).any(|pf| reaches(pf, unit)) {
                continue;
            }
            let kind = match unit.kind {
                firstparty::UnitKind::Adapter => UnitKind::Adapter,
                firstparty::UnitKind::Commons => UnitKind::Commons,
            };
            let parse = firstparty_parsed(
                &CACHES[i],
                unit.identity_path,
                unit.source,
                kind,
                FIRSTPARTY_ID_BASE + unit.id_block * FIRSTPARTY_ID_BLOCK,
                firstparty_file_id(unit),
            );
            reached[i] = Some(match parse {
                Ok(pf) => Some(pf),
                Err(errs) => {
                    errors.extend_for(None, errs);
                    None
                }
            });
            grew = true;
        }
        if !grew {
            break;
        }
    }
    let mut order: Vec<usize> = (0..N).filter(|&i| reached[i].is_some()).collect();
    order.sort_by_key(|&i| firstparty::UNITS[i].id_block);
    for &i in &order {
        if let Some(Some(pf)) = reached[i].take() {
            parsed.push(pf);
        }
    }
    order.into_iter().map(|i| &firstparty::UNITS[i]).collect()
}

/// S4a (#1966): resolve each derived capability (`capability Sessions = Kv`)
/// against the capabilities its context flattens in with `consumes U { Kv }`,
/// and record it. A valid one enters the unit's capability table with its
/// base's operations, so every later check treats it as an ordinary
/// capability; its provision is implied (the base's platform provider, bound to
/// the instance's own Worker binding). Runs after
/// [`crate::symbols::record_flattened_caps`].
pub fn phase_resolve_derived(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &[ParsedFile],
    unit_tables: &mut HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
) {
    for (name, indices) in groups {
        for &i in indices {
            if parsed[i].is_synthetic() {
                continue;
            }
            let path = parsed[i].identity_path();
            for item in parsed[i].items() {
                let CommonsItem::Capability(c) = item else {
                    continue;
                };
                let Some(d) = &c.derived else { continue };
                if kinds.get(name) != Some(&UnitKind::Context) {
                    errors.push_for(
                        Some(&path),
                        CompileError::new(
                            "bynk.capability.derived_outside_context",
                            c.span,
                            format!(
                                "derived capability `{}` is declared outside a context",
                                c.name.name
                            ),
                        )
                        .with_note("a named instance of a platform resource belongs to the context that owns its data"),
                    );
                    continue;
                }
                let Some(table) = unit_tables.get(name) else {
                    continue;
                };
                let Some(owner) = table.flattened_caps.get(&d.base.name).cloned() else {
                    errors.push_for(
                        Some(&path),
                        CompileError::new(
                            "bynk.capability.unknown_base",
                            d.base.span,
                            format!(
                                "`{}` is not a capability this context selects with `consumes U {{ … }}`",
                                d.base.name
                            ),
                        )
                        .with_note(format!(
                            "a derived capability's base is a platform capability, selected by name: `consumes bynk.cloudflare {{ {} }}`",
                            d.base.name
                        )),
                    );
                    continue;
                };
                let Some(kind) = crate::firstparty::instantiable(&owner, &d.base.name) else {
                    errors.push_for(
                        Some(&path),
                        CompileError::new(
                            "bynk.capability.base_not_instantiable",
                            d.base.span,
                            format!(
                                "`{owner}.{}` has no named instances: a derived capability's base must be a platform resource kind",
                                d.base.name
                            ),
                        ),
                    );
                    continue;
                };
                // Interop I5 (#1972): each option's value, once valid.
                let mut options: Vec<(String, crate::symbols::OptionValue)> = Vec::new();
                for ann in &d.annotations {
                    if let Some(option_kind) = kind.option_kind(&ann.name.name) {
                        if options.iter().any(|(o, _)| *o == ann.name.name) {
                            errors.push_for(
                                Some(&path),
                                CompileError::new(
                                    "bynk.capability.invalid_option",
                                    ann.span,
                                    format!("`@{}` is given twice", ann.name.name),
                                )
                                .with_note("an instance takes each option once"),
                            );
                        } else if let Some(value) =
                            instance_option_value(ann, option_kind, &d.base.name, &path, errors)
                        {
                            // A Worker the build itself makes is a Bynk
                            // context: deploy and dev read any build Worker
                            // as a project edge, never a foreign one, so it
                            // is reached with `consumes` (review of #2008).
                            let own = match &value {
                                crate::symbols::OptionValue::Str(value)
                                    if ann.name.name == firstparty::SERVICE_WORKER_OPTION =>
                                {
                                    kinds.iter().find(|(unit, kind)| {
                                        **kind == UnitKind::Context
                                            && bynk_project::worker_dir_name(unit) == *value
                                    })
                                }
                                _ => None,
                            };
                            if let Some((context, _)) = own {
                                errors.push_for(
                                    Some(&path),
                                    CompileError::new(
                                        "bynk.capability.invalid_option",
                                        ann.span,
                                        format!(
                                            "`{}` is this project's context `{context}`, not a Worker outside it",
                                            bynk_project::worker_dir_name(context)
                                        ),
                                    )
                                    .with_note(format!(
                                        "reach a Bynk context with `consumes {context}`, which checks its contract"
                                    )),
                                );
                            } else {
                                options.push((ann.name.name.clone(), value));
                            }
                        }
                    }
                    if kind.option_kind(&ann.name.name).is_none() {
                        let allowed = if kind.options.is_empty() {
                            format!("`{}` instances take no options", d.base.name)
                        } else {
                            format!(
                                "`{}` instances take {}",
                                d.base.name,
                                kind.option_names()
                                    .map(|o| format!("`@{o}`"))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            )
                        };
                        errors.push_for(
                            Some(&path),
                            CompileError::new(
                                "bynk.capability.unknown_option",
                                ann.span,
                                format!(
                                    "`@{}` is not an option of a `{}` instance",
                                    ann.name.name, d.base.name
                                ),
                            )
                            .with_note(allowed),
                        );
                    }
                }
                for required in kind.required {
                    if !d.annotations.iter().any(|a| a.name.name == *required) {
                        errors.push_for(
                            Some(&path),
                            CompileError::new(
                                "bynk.capability.missing_option",
                                c.span,
                                format!("a `{}` instance needs `@{required}(…)`", d.base.name),
                            )
                            .with_note(format!("write `{}`", instance_example(kind))),
                        );
                    }
                }
                if table.exported_capabilities.contains(&c.name.name) {
                    errors.push_for(
                        Some(&path),
                        CompileError::new(
                            "bynk.exports.derived_capability",
                            c.span,
                            format!("derived capability `{}` cannot be exported", c.name.name),
                        )
                        .with_note("an instance's data belongs to this context; another context reaches it through this context's services"),
                    );
                }
                let ops = unit_tables
                    .get(&owner)
                    .and_then(|t| t.capabilities.get(&d.base.name))
                    .map(|base| base.ops.clone())
                    .unwrap_or_default();
                let binding = crate::worker_bindings::instance_binding_name(
                    kind.binding_prefix,
                    &c.name.name,
                );
                let table = unit_tables.get_mut(name).expect("checked above");
                if let Some(decl) = table.capabilities.get_mut(&c.name.name) {
                    decl.ops = ops;
                }
                table.derived.insert(
                    c.name.name.clone(),
                    crate::symbols::DerivedInstance {
                        base_unit: owner,
                        base_cap: d.base.name.clone(),
                        binding,
                        options,
                        span: c.span,
                    },
                );
            }
        }
    }
}

/// The example instance declaration a note about `kind` shows: every
/// required option set to a sample value (P3, #2342, so a `RateLimit` note
/// shows `@limit(100) @period(60.seconds)`, not a Worker name).
pub(crate) fn instance_example(kind: &firstparty::Instantiable) -> String {
    let name = match kind.capability {
        "Service" => "Billing",
        "RateLimit" => "LoginLimit",
        _ => "Archives",
    };
    let mut out = format!("capability {name} = {}", kind.capability);
    for required in kind.required {
        out.push_str(&format!(" @{required}({})", option_sample(required)));
    }
    out
}

/// A sample value of option `option`, as a note writes it.
fn option_sample(option: &str) -> &'static str {
    match option {
        firstparty::R2_JURISDICTION_OPTION => "\"eu\"",
        firstparty::RATELIMIT_LIMIT_OPTION => "100",
        firstparty::RATELIMIT_PERIOD_OPTION => "60.seconds",
        _ => "\"legacy-billing\"",
    }
}

/// Interop I5 (#1972): the value of one instance option: one positional
/// literal of the option's kind (P3, #2342). `@worker("name")` names a Worker,
/// so it must be a Worker name as Cloudflare takes one: lowercase letters,
/// digits, `-` and `_`, at most 63 characters, not starting or ending with
/// `-`. `@jurisdiction` is one of R2's; `@limit` a positive `Int`; `@period`
/// one of the rate limiting windows. `None`, with the error pushed, when the
/// value is not one of these.
fn instance_option_value(
    ann: &bynk_syntax::ast::Annotation,
    kind: firstparty::OptionKind,
    base: &str,
    path: &std::path::Path,
    errors: &mut ErrorSink,
) -> Option<crate::symbols::OptionValue> {
    use crate::symbols::OptionValue;
    let option = ann.name.name.as_str();
    let invalid = |span, message: String| {
        CompileError::new("bynk.capability.invalid_option", span, message).with_note(format!(
            "write `@{option}({})`, for example",
            option_sample(option)
        ))
    };
    let [arg] = ann.args.as_slice() else {
        errors.push_for(
            Some(path),
            invalid(
                ann.span,
                format!("`@{option}` takes one positional argument"),
            ),
        );
        return None;
    };
    if arg.label.is_some() {
        errors.push_for(
            Some(path),
            invalid(
                ann.span,
                format!("`@{option}` takes one positional argument"),
            ),
        );
        return None;
    }
    let value = match (kind, &arg.value.kind) {
        (firstparty::OptionKind::Str, ExprKind::StrLit(value)) => OptionValue::Str(value.clone()),
        (firstparty::OptionKind::Str, _) => {
            errors.push_for(
                Some(path),
                invalid(arg.span, format!("`@{option}` takes a string literal")),
            );
            return None;
        }
        (firstparty::OptionKind::PositiveInt, ExprKind::IntLit { value, .. }) if *value > 0 => {
            OptionValue::Int(*value)
        }
        (firstparty::OptionKind::PositiveInt, _) => {
            errors.push_for(
                Some(path),
                invalid(
                    arg.span,
                    format!("`@{option}` takes a positive `Int` literal"),
                ),
            );
            return None;
        }
        (firstparty::OptionKind::Duration, ExprKind::DurationLit { millis, .. })
            if *millis % 1000 == 0 && firstparty::RATELIMIT_PERIODS.contains(&(*millis / 1000)) =>
        {
            OptionValue::Seconds(*millis / 1000)
        }
        (firstparty::OptionKind::Duration, _) => {
            errors.push_for(
                Some(path),
                CompileError::new(
                    "bynk.capability.invalid_option",
                    arg.span,
                    format!("`@{option}` must be a window a `{base}` offers"),
                )
                .with_note(format!(
                    "the windows are {}",
                    firstparty::RATELIMIT_PERIODS
                        .iter()
                        .map(|s| format!("`{s}.seconds`"))
                        .collect::<Vec<_>>()
                        .join(" and ")
                )),
            );
            return None;
        }
    };
    let OptionValue::Str(text) = &value else {
        return Some(value);
    };
    if option == crate::firstparty::R2_JURISDICTION_OPTION
        && !crate::firstparty::R2_JURISDICTIONS.contains(&text.as_str())
    {
        errors.push_for(
            Some(path),
            CompileError::new(
                "bynk.capability.invalid_option",
                ann.span,
                format!("`{text}` is not a jurisdiction a `{base}` bucket can be pinned to"),
            )
            .with_note(format!(
                "the jurisdictions are {}",
                crate::firstparty::R2_JURISDICTIONS
                    .iter()
                    .map(|j| format!("`\"{j}\"`"))
                    .collect::<Vec<_>>()
                    .join(" and ")
            )),
        );
        return None;
    }
    if option == crate::firstparty::SERVICE_WORKER_OPTION && !is_worker_name(text) {
        errors.push_for(
            Some(path),
            CompileError::new(
                "bynk.capability.invalid_option",
                ann.span,
                format!("`{text}` is not a Worker name a `{base}` can reach"),
            )
            .with_note(
                "a Worker name is 1 to 63 lowercase letters, digits, `-` or `_`, and does not start or end with `-`",
            ),
        );
        return None;
    }
    Some(value)
}

/// Whether `name` is a Cloudflare Worker name ([`instance_option_value`]).
fn is_worker_name(name: &str) -> bool {
    (1..=63).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// #1710: for each unit with a file the strict parse rejected, the names of
/// the declarations recovery skipped there. References to them, from any file
/// that can see the unit, are known names, not unknown ones (#1663's Decision
/// B); see [`crate::check_pipeline::prepare_unit_check_ctx`].
pub type BrokenDeclNames = HashMap<String, Vec<String>>;

/// The `include` tree that discovered `pf`, found by matching its absolute
/// path against each tree's root — not `trees[0]` unconditionally, since
/// R3.9 (#1113) lets a file live under any `include` tree, not just the
/// first. Falls back to `trees[0]` for a `pf` with no `abs_path` (unreachable
/// for a real adapter: only synthetic units, which never declare `binding`,
/// go without one) or if it somehow matches none. The longest matching root
/// wins, in case one `include` tree is nested inside another.
///
/// A `trees` root is only absolute when `Roots`'s own `project_root` is — a
/// relative project root (`bynkc build .`, the ordinary CLI shape) leaves
/// every tree root relative, while `pf.abs_path()` (`bynk-project`'s
/// `parse_sources`, via `std::path::absolute`) is always absolute. Comparing
/// them directly with `starts_with` would never match, silently collapsing
/// this back to the `trees[0]` bug it exists to fix. Each root is resolved
/// through the same `std::path::absolute` before comparing, matching
/// `abs_path`'s own normalisation exactly rather than requiring the caller
/// to have already absolutised `Roots::project_root`.
pub fn tree_root_for<'a>(trees: &'a [(PathBuf, PathBuf)], pf: &ParsedFile) -> &'a Path {
    let Some(abs) = pf.abs_path() else {
        return trees[0].0.as_path();
    };
    trees
        .iter()
        .filter_map(|(root, _)| {
            std::path::absolute(root)
                .ok()
                .map(|abs_root| (root, abs_root))
        })
        .filter(|(_, abs_root)| abs.starts_with(abs_root))
        .max_by_key(|(root, _)| root.as_os_str().len())
        .map(|(root, _)| root.as_path())
        .unwrap_or_else(|| trees[0].0.as_path())
}

/// #2321: resolve an adapter binding's `includes [ … ]` list. Each path is
/// relative to the adapter's own directory, must stay inside it (no absolute
/// path, no `..`), must be a `.wasm`, and must be unique once normalised; the
/// file is read beside the binding (a dependency's from beside the file it was
/// read from, like the binding itself, #2223). Reports the four
/// `bynk.adapter.include_*` diagnostics and skips the offending entry.
fn resolve_binding_includes(
    pf: &ParsedFile,
    a: &AdapterDecl,
    b: &BindingDecl,
    adapter_dir: &Path,
    adapter_src_dir: &Path,
    errors: &mut ErrorSink,
) -> Vec<IncludedFile> {
    let adapter = a.name.joined();
    let mut seen: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    let mut out = Vec::new();
    for inc in &b.includes {
        let raw = Path::new(&inc.path);
        let escapes = raw.is_absolute()
            || raw.components().any(|c| {
                matches!(
                    c,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            });
        if escapes {
            errors.push_for(
                Some(&pf.identity_path()),
                CompileError::new(
                    "bynk.adapter.include_outside",
                    inc.span,
                    format!(
                        "adapter `{adapter}` includes `{}`, which is outside the adapter's directory",
                        inc.path
                    ),
                )
                .with_note("an included path is relative to the adapter's file and may not be absolute or contain `..`"),
            );
            continue;
        }
        let rel = normalize_rel(raw);
        if rel.extension().and_then(|e| e.to_str()) != Some("wasm") {
            errors.push_for(
                Some(&pf.identity_path()),
                CompileError::new(
                    "bynk.adapter.include_kind",
                    inc.span,
                    format!(
                        "adapter `{adapter}` includes `{}`, which is not a `.wasm` file",
                        inc.path
                    ),
                )
                .with_note("only WebAssembly modules (`.wasm`) can be included beside a binding"),
            );
            continue;
        }
        if !seen.insert(rel.clone()) {
            errors.push_for(
                Some(&pf.identity_path()),
                CompileError::new(
                    "bynk.adapter.include_duplicate",
                    inc.span,
                    format!("adapter `{adapter}` includes `{}` more than once", inc.path),
                )
                .with_note("remove the repeated entry"),
            );
            continue;
        }
        match read_adapter_include(&adapter_src_dir.join(&rel)) {
            Ok(bytes) => out.push(IncludedFile {
                output_path: normalize_rel(&adapter_dir.join(&rel)),
                bytes,
            }),
            Err(e) => errors.push_for(
                Some(&pf.identity_path()),
                CompileError::new(
                    "bynk.adapter.include_missing",
                    inc.span,
                    format!(
                        "adapter `{adapter}` includes `{}`, which could not be read ({e})",
                        inc.path
                    ),
                )
                .with_note("the path is resolved relative to the adapter's source file"),
            ),
        }
    }
    out
}

/// Phase 3: group the parsed units by qualified name (production units, unit
/// tests, and integration suites tracked separately), run the per-directory
/// and path/name consistency checks, enforce the reserved `bynk` namespace and
/// the adapter `binding` rules, resolve each adapter's binding module, and fold
/// the adapters' pinned npm dependencies. Pushes diagnostics into `errors` and
/// returns the production `groups`/`kinds`, the `test`/`integration` groups, the
/// resolved `adapter_bindings`, and the collected `npm_deps`.
#[allow(clippy::type_complexity)]
#[allow(clippy::too_many_arguments)]
pub fn phase_group(
    parsed: &[ParsedFile],
    trees: &[(PathBuf, PathBuf)],
    platform: Platform,
    injected: &[&'static firstparty::Unit],
    overlay: &HashMap<PathBuf, String>,
    errors: &mut ErrorSink,
) -> (
    BTreeMap<String, Vec<usize>>,
    BTreeMap<String, UnitKind>,
    BTreeMap<String, Vec<usize>>,
    BTreeMap<String, Vec<usize>>,
    HashMap<String, AdapterBinding>,
    std::collections::BTreeMap<String, String>,
) {
    // Tests (v0.7) are tracked separately from production units. Their
    // `target` joined-name can intentionally coincide with a commons or
    // context name; they don't enter the production groups/kinds maps.
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut kinds: BTreeMap<String, UnitKind> = BTreeMap::new();
    let mut test_groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    // v0.16: integration tests are tracked by suite name, separately again from
    // unit tests — their `name()` is the synthetic `integration <suite>`.
    let mut integration_groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, pf) in parsed.iter().enumerate() {
        let name = pf.unit().name().joined();
        if pf.kind() == UnitKind::Integration {
            integration_groups.entry(name).or_default().push(i);
        } else if pf.kind() == UnitKind::Test {
            test_groups.entry(name).or_default().push(i);
        } else {
            groups.entry(name.clone()).or_default().push(i);
            kinds.entry(name).or_insert(pf.kind());
        }
    }
    // #696: the consistency checks pair each error with the project-relative
    // path of the file its primary span belongs to, so the CLI renders them
    // with ariadne source context rather than the plain fallback.
    if let Err(e) = check_directory_name_consistency(parsed) {
        for (path, err) in e {
            errors.push_for(Some(&path), err);
        }
    }
    if let Err(e) = check_directory_kind_consistency(parsed) {
        errors.extend_for(None, e);
    }
    // A group must agree on kind across all its files (different name but
    // same kind is fine; same name but different kind is an error).
    if let Err(e) = check_group_kind_consistency(parsed, &groups) {
        for (path, err) in e {
            errors.push_for(Some(&path), err);
        }
    }
    // #1885 (DECISION D): a unit split across files carries at most one
    // module doc.
    if let Err(e) = check_group_module_docs(parsed, &groups) {
        for (path, err) in e {
            errors.push_for(Some(&path), err);
        }
    }
    // Each *source* unit's file path must match its declared qualified name.
    // v0.113 (DECISION S): a `suite` has no path-identity requirement — it names
    // its target and is legal in any file — so test-ness carries no path check.
    if let Err(e) = check_path_name_alignment(parsed) {
        for (path, err) in e {
            errors.push_for(Some(&path), err);
        }
    }

    // v0.20a: function types are confined to non-boundary positions. P5.2:
    // this used to be an injected hook (`function_type_boundary_check`) so
    // `run_checks` and the new analysis entry point could reach it without
    // `bynk-check` reaching back into `bynk-emit`; now that the check lives
    // here too, it's a direct call at the exact point the hook used to fire,
    // preserving diagnostic order for both callers with no hook needed.
    phase_function_type_boundaries(parsed, errors);

    // v0.17: the `bynk` root namespace is reserved for the toolchain. No user
    // unit of any kind may be named `bynk` or `bynk.*` (§3.4).
    for pf in parsed {
        if pf.is_synthetic() {
            continue;
        }
        let qn = pf.unit().name();
        if qn.parts.first().is_some_and(|p| p.name == "bynk") {
            errors.push_for(Some(&pf.identity_path()),
                CompileError::new(
                    "bynk.namespace.reserved",
                    qn.span,
                    format!(
                        "`{}` uses the reserved `bynk` namespace — the `bynk` root is reserved for the toolchain's conformance surface",
                        qn.joined()
                    ),
                )
                .with_note("rename the unit so its first segment is not `bynk`"),
            );
        }
    }

    // v0.17: an adapter that declares any external provider must name a
    // `binding` module to supply the implementation symbols (§3.5). First-party
    // (synthetic) adapters omit the clause — the toolchain supplies the binding.
    for pf in parsed {
        if pf.is_synthetic() {
            continue;
        }
        if let Some(a) = pf.adapter() {
            let has_external = a
                .items
                .iter()
                .any(|it| matches!(it, CommonsItem::Provider(p) if p.external));
            if has_external && a.binding.is_none() {
                errors.push_for(Some(&pf.identity_path()),
                    CompileError::new(
                        "bynk.adapter.no_binding",
                        a.span,
                        format!(
                            "adapter `{}` declares an external provider but has no `binding` clause to supply its implementation",
                            a.name.joined()
                        ),
                    )
                    .with_note(
                        "add a `binding \"<module>\"` clause naming the TypeScript module that exports the provider symbols",
                    ),
                );
            }
        }
    }

    // v0.17: resolve each adapter's binding module (relative to the adapter's
    // source file) and read it, so compose can import the external provider
    // symbols and the binding is copied into the output for the `tsc` gate.
    let mut adapter_bindings: HashMap<String, AdapterBinding> = HashMap::new();
    // v0.17/v0.19: the toolchain supplies each injected first-party adapter's
    // binding — the `bynk` surface's per platform, a platform adapter's single
    // one (the lock check rejects other `--platform` selections).
    for unit in injected {
        if let Some((output_path, content)) = unit.binding_for(platform) {
            adapter_bindings.insert(
                unit.name.to_string(),
                AdapterBinding {
                    output_path: PathBuf::from(output_path),
                    content: content.to_string(),
                    includes: Vec::new(),
                },
            );
        }
    }
    for pf in parsed {
        let Some(a) = pf.adapter() else { continue };
        let Some(b) = &a.binding else { continue };
        let pf_source_path = pf.source_path();
        let adapter_dir = pf_source_path.parent().unwrap_or(Path::new(""));
        let out_rel = normalize_rel(&adapter_dir.join(&b.module));
        // #2223: a dependency file's `source_path` is its output layout
        // (`deps/<org>/<package>/…`), so its binding is found beside the file
        // it was read from.
        let src_abs = match (pf.package(), pf.abs_path()) {
            (Some(_), Some(abs)) => {
                let dir = abs.parent().unwrap_or(Path::new("")).to_path_buf();
                dir.join(normalize_rel(Path::new(&b.module)))
            }
            _ => tree_root_for(trees, pf).join(&out_rel),
        };
        match read_adapter_binding(&src_abs, overlay) {
            Ok(content) => {
                // The adapter file's own directory on disk — what includes are
                // relative to (the binding may sit in a subdirectory of it).
                let adapter_src_dir = match (pf.package(), pf.abs_path()) {
                    (Some(_), Some(abs)) => abs.parent().unwrap_or(Path::new("")).to_path_buf(),
                    _ => tree_root_for(trees, pf).join(adapter_dir),
                };
                let includes =
                    resolve_binding_includes(pf, a, b, adapter_dir, &adapter_src_dir, errors);
                adapter_bindings.insert(
                    a.name.joined(),
                    AdapterBinding {
                        output_path: out_rel,
                        content,
                        includes,
                    },
                );
            }
            Err(e) => {
                errors.push_for(Some(&pf.identity_path()),
                    CompileError::new(
                        "bynk.adapter.no_binding",
                        b.module_span,
                        format!(
                            "adapter `{}` names binding module `{}`, which could not be read ({e})",
                            a.name.joined(),
                            b.module
                        ),
                    )
                    .with_note(
                        "the binding path is resolved relative to the adapter's source file; author the `.binding.ts` there",
                    ),
                );
            }
        }
    }

    // v0.17: collect adapter npm dependencies for `package.json`, rejecting
    // unpinned ranges ([DECISION L] stub — fold + pin-check only, no allow-list).
    // #2201: two entries for one package must agree on the range; a different
    // range is refused rather than intersected or overwritten.
    let mut npm_deps: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    // package → (adapter, its file's identity path, entry span) of the first entry.
    let mut first_dep: HashMap<String, (String, PathBuf, Span)> = HashMap::new();
    for pf in parsed {
        let Some(a) = pf.adapter() else { continue };
        let Some(b) = &a.binding else { continue };
        for dep in &b.requires {
            if is_unpinned_range(&dep.range) {
                errors.push_for(Some(&pf.identity_path()),
                    CompileError::new(
                        "bynk.requires.unpinned_dependency",
                        dep.span,
                        format!(
                            "dependency `{}` has an unpinned version range `{}` — pin a concrete range (e.g. `^1.2.0`)",
                            dep.package, dep.range
                        ),
                    )
                    .with_note(
                        "unpinned ranges (`*`, `latest`, …) make builds irreproducible and are rejected",
                    ),
                );
                continue;
            }
            let adapter = a.name.joined();
            let identity = pf.identity_path();
            if let Some(range) = npm_deps.get(&dep.package) {
                if *range != dep.range {
                    let (first_adapter, first_path, first_span) = &first_dep[&dep.package];
                    let message = if *first_adapter == adapter {
                        format!(
                            "adapter `{adapter}` requires `{}` at both `{range}` and `{}`",
                            dep.package, dep.range
                        )
                    } else {
                        format!(
                            "adapter `{adapter}` requires `{}` at `{}`, but adapter `{first_adapter}` requires it at `{range}`",
                            dep.package, dep.range
                        )
                    };
                    let mut err =
                        CompileError::new("bynk.requires.conflicting_range", dep.span, message);
                    if *first_path == identity {
                        err = err.with_label(*first_span, "first required here");
                    }
                    errors.push_for(
                        Some(&identity),
                        err.with_note(format!(
                            "`{}` is first required at `{range}` by adapter `{first_adapter}` (in `{}`); the project installs one version of each package, so every adapter must state the same range",
                            dep.package,
                            first_path
                                .components()
                                .map(|c| c.as_os_str().to_string_lossy())
                                .collect::<Vec<_>>()
                                .join("/")
                        )),
                    );
                }
                continue;
            }
            first_dep.insert(dep.package.clone(), (adapter, identity, dep.span));
            npm_deps.insert(dep.package.clone(), dep.range.clone());
        }
    }

    (
        groups,
        kinds,
        test_groups,
        integration_groups,
        adapter_bindings,
        npm_deps,
    )
}

/// v0.20a: apply the function-type boundary confinement to every serialisable
/// or boundary-crossing position in a file's items: record fields and sum
/// payloads (types can cross contexts and persist), service/agent handler
/// signatures (the Workers wire), capability operation signatures (kept out
/// in v0.20a — see ADR 0030), agent state fields, and agent keys. Free `fn`
/// signatures are deliberately NOT walked — they are the non-boundary home
/// of function types.
///
/// #696: each diagnostic is paired with the project-relative `identity_path` of
/// the file whose items produced it, so the CLI renders it against that file's
/// source.
///
/// P5.2 (`design/tracks/semantics-in-the-checker.md` §6): relocated verbatim
/// from `bynk-emit/src/project/validate.rs`'s `check_function_type_boundaries`
/// — category 6 of `analysis.rs`'s own seven-category accounting. Previously
/// reached only through `phase_group`'s optional `function_type_boundary_check`
/// hook (`Some` from `run_checks`, `None` from the new entry point); that hook
/// is gone — [`phase_group`] itself now calls this function directly, at the
/// exact point the hook used to fire, so both callers see it in the same
/// diagnostic-ordering position as before and can no longer drift on whether
/// the check runs at all.
pub fn phase_function_type_boundaries(parsed: &[ParsedFile], errors: &mut ErrorSink) {
    // v0.174 (#592): the boundary check now also rejects a *recursive* generic
    // record (`reject_fn_types`' `App` arm), which needs the type declarations to
    // walk the containment graph. Build the project-wide table once — a generic
    // referenced from one file may be declared in another.
    let types = collect_type_decls(parsed.iter().flat_map(|pf| pf.items()));
    for pf in parsed {
        let mut file_errors: Vec<CompileError> = Vec::new();
        check_function_type_boundary_items(pf.items(), &types, &mut file_errors);
        for err in file_errors {
            errors.push_for(Some(&pf.identity_path()), err);
        }
    }
}

/// v0.174 (#592): a `name -> TypeDecl` table over a set of items, for the
/// recursive-generic boundary walk. Relocated alongside
/// `phase_function_type_boundaries` (P5.2) — public since `bynk-emit`'s
/// single-file compile path (`lib.rs`) also needs it, across the crate
/// boundary this relocation now draws.
pub fn collect_type_decls<'a>(
    items: impl Iterator<Item = &'a CommonsItem>,
) -> HashMap<String, Arc<TypeDecl>> {
    let mut out = HashMap::new();
    for item in items {
        match item {
            CommonsItem::Type(t) => {
                out.entry(t.name.name.clone())
                    .or_insert_with(|| Arc::new(t.clone()));
            }
            // Events track, slice 0 (spine #936): an event's synthetic
            // `TypeDecl` joins the same table, so a field referencing an
            // event type recurses into it exactly like any other type.
            CommonsItem::Event(e) => {
                out.entry(e.name.name.clone())
                    .or_insert_with(|| Arc::new(e.as_type_decl()));
            }
            _ => {}
        }
    }
    out
}

/// Item-level body of the boundary confinement, shared with the single-file
/// (legacy) compile path in `bynk-emit`'s `lib.rs`. Relocated alongside
/// `phase_function_type_boundaries` (P5.2).
pub fn check_function_type_boundary_items(
    items: &[CommonsItem],
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    for item in items {
        match item {
            CommonsItem::Type(t) => match &t.body {
                TypeBody::Record(r) => {
                    for f in &r.fields {
                        reject_fn_types(&f.type_ref, "a record field", types, errors);
                    }
                }
                TypeBody::Sum(s) => {
                    for v in &s.variants {
                        for p in &v.payload {
                            reject_fn_types(&p.type_ref, "a sum-variant payload", types, errors);
                        }
                    }
                }
                TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
            },
            // Events track, slice 0 (spine #936): an event's fields are
            // boundary values (an emission crosses a context boundary),
            // so the same record-field rule applies as for a `type`.
            CommonsItem::Event(e) => {
                for f in &e.body.fields {
                    reject_fn_types(&f.type_ref, "an event field", types, errors);
                }
            }
            CommonsItem::Capability(c) => {
                for op in &c.ops {
                    for p in &op.params {
                        reject_fn_types(
                            &p.type_ref,
                            "a capability operation signature",
                            types,
                            errors,
                        );
                    }
                    // v0.102 (§2.9.1): a capability operation may *produce* a
                    // held value — it is the canonical held source — so an
                    // `Effect[Connection[F]]` return is admitted.
                    if !type_ref_is_held(&op.return_type) {
                        reject_fn_types(
                            &op.return_type,
                            "a capability operation signature",
                            types,
                            errors,
                        );
                    }
                }
            }
            CommonsItem::Service(s) => {
                for h in &s.handlers {
                    for p in &h.params {
                        // #2341 (decision A): an HTTP handler's `headers` is
                        // the one place `RequestHeaders` may appear.
                        let http_headers = matches!(h.kind, HandlerKind::Http { .. })
                            && p.name.name == "headers"
                            && matches!(p.type_ref, TypeRef::RequestHeaders(_));
                        // #2341 (decision C): and `on rejected`'s parameter is
                        // the one place `BoundaryRejection` may appear.
                        let http_headers = http_headers
                            || (h.kind == HandlerKind::Rejected
                                && matches!(p.type_ref, TypeRef::BoundaryRejection(_)));
                        // v0.102 (§2.9.4): the framework may supply a held
                        // value as a handler parameter (the `on open`
                        // connection), so a `Connection[F]` parameter is
                        // admitted.
                        if !type_ref_is_held(&p.type_ref) && !http_headers {
                            reject_fn_types(
                                &p.type_ref,
                                "a service handler signature",
                                types,
                                errors,
                            );
                        }
                    }
                    reject_fn_types(&h.return_type, "a service handler signature", types, errors);
                }
            }
            CommonsItem::Agent(a) => {
                reject_fn_types(&a.key_type, "an agent key", types, errors);
                for f in &a.store_fields {
                    validate_store_field_value_types(f, types, errors);
                }
                for h in &a.handlers {
                    for p in &h.params {
                        // v0.102 (§2.9.4): a held value may be transferred to
                        // an agent handler as a parameter.
                        if !type_ref_is_held(&p.type_ref) {
                            reject_fn_types(
                                &p.type_ref,
                                "an agent handler signature",
                                types,
                                errors,
                            );
                        }
                    }
                    reject_fn_types(&h.return_type, "an agent handler signature", types, errors);
                }
            }
            CommonsItem::Actor(a) => {
                if let Some(id) = &a.identity {
                    reject_fn_types(id, "an actor identity type", types, errors);
                }
            }
            // slice 1: `MessageEntry.code`/`.template` are plain string
            // literals, no fn-type-bearing fields to reject here.
            CommonsItem::Fn(_)
            | CommonsItem::Provider(_)
            | CommonsItem::Messages(_)
            | CommonsItem::Asset(_)
            | CommonsItem::Embed(_) => {}
        }
    }
}

/// Phase 4: build each production unit's combined symbol table from its files,
/// pushing any table-construction errors into `errors`.
pub fn phase_symbol_tables(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &[ParsedFile],
    errors: &mut ErrorSink,
) -> HashMap<String, UnitTable> {
    let mut unit_tables: HashMap<String, UnitTable> = HashMap::new();
    for (name, indices) in groups {
        let kind = *kinds.get(name).expect("every group has a kind");
        // #696: build_unit_table pairs each diagnostic with its declaring file.
        let mut table_errors: Vec<(PathBuf, CompileError)> = Vec::new();
        let table = build_unit_table(name, kind, indices, parsed, &mut table_errors);
        for (path, err) in table_errors {
            errors.push_for(Some(&path), err);
        }
        unit_tables.insert(name.clone(), table);
    }
    unit_tables
}

/// Phase 5: resolve each unit's `uses` clauses, checking the target exists, is
/// a commons, and is not self-referential. Returns unit → deduplicated list of
/// used commons; diagnostics go into `errors`.
/// #1702 review: the order to check units in — every `uses` target before the
/// units that use it, ties broken by name, so the order stays a function of
/// the source alone (`deterministic_diagnostic_order_behaviour`). A generic
/// function's compared type parameters (#1688) are computed while its own unit
/// is checked, in that unit's environment, so an importer must come after.
/// Units on a `uses` cycle (already an error) are appended in name order.
pub fn uses_first_order<'a, I>(
    names: I,
    unit_uses: &HashMap<String, Vec<String>>,
) -> Vec<&'a String>
where
    I: IntoIterator<Item = &'a String>,
{
    let names: BTreeSet<&String> = names.into_iter().collect();
    let mut placed: HashSet<&String> = HashSet::new();
    let mut order = Vec::with_capacity(names.len());
    loop {
        let ready: Vec<&String> = names
            .iter()
            .copied()
            .filter(|n| !placed.contains(n))
            .filter(|n| {
                unit_uses.get(*n).is_none_or(|deps| {
                    deps.iter()
                        .all(|d| !names.contains(d) || placed.iter().any(|p| *p == d))
                })
            })
            .collect();
        if ready.is_empty() {
            break;
        }
        for n in ready {
            placed.insert(n);
            order.push(n);
        }
    }
    order.extend(names.iter().copied().filter(|n| !placed.contains(n)));
    order
}

pub fn phase_resolve_uses(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &[ParsedFile],
    unit_tables: &HashMap<String, UnitTable>,
    // S3 (#1963): `uses` of an experimental commons needs an `[unstable]` opt-in.
    policy: &crate::stability::StabilityPolicy,
    errors: &mut ErrorSink,
) -> HashMap<String, Vec<String>> {
    let mut unit_uses: HashMap<String, Vec<String>> = HashMap::new();
    for (name, indices) in groups {
        let mut uses_targets: Vec<String> = Vec::new();
        for &i in indices {
            for u in parsed[i].uses() {
                let target = u.target.joined();
                if !unit_tables.contains_key(&target) {
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.uses.unknown_commons",
                            u.span,
                            format!("unknown commons `{target}`"),
                        )
                        .with_note(
                            "the target of a `uses` clause must be a commons in the project",
                        ),
                    );
                    continue;
                }
                // A first-party unit's own dependencies are the library's
                // business, never the project's opt-in.
                if !parsed[i].is_synthetic()
                    && let Some(err) = policy.check(&target, u.target.span)
                {
                    errors.push_for(Some(&parsed[i].identity_path()), err);
                }
                let target_kind = *kinds.get(&target).unwrap();
                if target_kind != UnitKind::Commons {
                    errors.push_for(Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.uses.target_is_context",
                            u.span,
                            format!(
                                "`uses {target}` targets a context — `uses` may only target a commons"
                            ),
                        )
                        .with_note(
                            "to declare a dependency on a context, use `consumes` instead",
                        ),
                    );
                    continue;
                }
                if target == *name {
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.uses.self_reference",
                            u.span,
                            format!("`{name}` cannot `uses` itself"),
                        ),
                    );
                    continue;
                }
                if !uses_targets.contains(&target) {
                    uses_targets.push(target);
                }
            }
        }
        unit_uses.insert(name.clone(), uses_targets);
    }
    unit_uses
}

/// Phase 5b: resolve each unit's `consumes` clauses (target exists, is a context
/// or adapter, not self-referential, obeys the adapter selection rules), and for
/// the braced `consumes U { Cap, … }` form validate and record the flattened
/// capabilities. Returns unit → consumed targets and unit → flattened-cap → owning
/// unit; diagnostics go into `errors` and clause-position references into `refs`.
#[allow(clippy::type_complexity)]
pub fn phase_resolve_consumes(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &[ParsedFile],
    unit_tables: &HashMap<String, UnitTable>,
    // S3 (#1963): consuming an experimental adapter, or selecting an
    // experimental capability, needs an `[unstable]` opt-in.
    policy: &crate::stability::StabilityPolicy,
    errors: &mut ErrorSink,
    refs: &mut RefSink,
) -> (
    HashMap<String, Vec<String>>,
    HashMap<String, HashMap<String, String>>,
) {
    let mut unit_consumes: HashMap<String, Vec<String>> = HashMap::new();
    // v0.17: `consumes U { Cap, … }` flattens selected caps into the consumer's
    // local namespace. unit → bare-cap → consumed unit providing it.
    let mut unit_flattened: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (name, indices) in groups {
        let kind = *kinds.get(name).unwrap();
        let mut consumes_targets: Vec<String> = Vec::new();
        let mut flattened: HashMap<String, String> = HashMap::new();
        // #1857: the file that first selected each flattened capability, so a
        // file of a split context may repeat another file's selection.
        let mut flattened_in: HashMap<String, usize> = HashMap::new();
        let local_caps: HashSet<String> = unit_tables
            .get(name)
            .map(|t| t.capabilities.keys().cloned().collect())
            .unwrap_or_default();
        for &i in indices {
            refs.enter_file(&parsed[i].identity_path(), name, parsed[i].is_synthetic());
            for c in parsed[i].consumes() {
                let target = c.target.joined();
                // v0.18: an adapter's `consumes` is the braced capability-selection
                // form only — an adapter has no services to RPC-call, so the
                // whole-unit and `as Alias` forms are meaningless inside one.
                if kind == UnitKind::Adapter && c.selected.is_none() {
                    errors.push_for(Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.adapter.consumes_requires_selection",
                            c.span,
                            format!(
                                "an adapter's `consumes` must select capabilities — write `consumes {target} {{ Cap, … }}`",
                            ),
                        )
                        .with_note(
                            "adapters depend on capabilities, never on services; the whole-unit and aliased forms are context-only",
                        ),
                    );
                    continue;
                }
                if !unit_tables.contains_key(&target) {
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.consumes.unknown_context",
                            c.span,
                            format!("unknown context `{target}`"),
                        )
                        .with_note(
                            "the target of a `consumes` clause must be a context in the project",
                        ),
                    );
                    continue;
                }
                let target_kind = *kinds.get(&target).unwrap();
                // v0.17: `consumes` may target a context or an adapter (the host
                // boundary). It may not target a commons (use `uses` for that).
                if target_kind != UnitKind::Context && target_kind != UnitKind::Adapter {
                    errors.push_for(Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.consumes.target_is_commons",
                            c.span,
                            format!(
                                "`consumes {target}` targets a commons — `consumes` may only target a context or adapter"
                            ),
                        )
                        .with_note(
                            "to mix in declarations from a commons, use `uses` instead",
                        ),
                    );
                    continue;
                }
                // v0.18: adapter dependencies are adapter-to-adapter (spec §4.5) —
                // an adapter consuming a *context* would pull service logic into
                // the host boundary.
                if kind == UnitKind::Adapter && target_kind == UnitKind::Context {
                    errors.push_for(Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.adapter.consumes_context",
                            c.span,
                            format!(
                                "adapter `{name}` cannot `consumes` the context `{target}` — adapter dependencies are adapter-to-adapter"
                            ),
                        )
                        .with_note(
                            "an adapter may only depend on capabilities exported by other adapters (e.g. the `bynk` surface)",
                        ),
                    );
                    continue;
                }
                if target == *name {
                    let kind_word = if kind == UnitKind::Adapter {
                        "adapter"
                    } else {
                        "context"
                    };
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.consumes.self_reference",
                            c.span,
                            format!("{kind_word} `{name}` cannot `consumes` itself"),
                        ),
                    );
                    continue;
                }
                if !parsed[i].is_synthetic()
                    && let Some(err) = policy.check(&target, c.target.span)
                {
                    errors.push_for(Some(&parsed[i].identity_path()), err);
                }
                // v0.17: `consumes U { Cap, … }` — validate each selected name is
                // a capability `U` exports, detect clashes, and record the
                // flattening so bare `given Cap` resolves through the local path.
                if let Some(names) = &c.selected {
                    let exported = unit_tables
                        .get(&target)
                        .map(|t| &t.exported_capabilities)
                        .cloned()
                        .unwrap_or_default();
                    for cap in names {
                        if !parsed[i].is_synthetic()
                            && let Some(err) =
                                policy.check(&format!("{target}.{}", cap.name), cap.span)
                        {
                            errors.push_for(Some(&parsed[i].identity_path()), err);
                        }
                        if !exported.contains(&cap.name) {
                            errors.push_for(
                                Some(&parsed[i].identity_path()),
                                CompileError::new(
                                    "bynk.given.cross_context_unknown_capability",
                                    cap.span,
                                    format!(
                                        "`{target}` does not export a capability named `{}`",
                                        cap.name
                                    ),
                                ),
                            );
                            continue;
                        }
                        if local_caps.contains(&cap.name) {
                            errors.push_for(Some(&parsed[i].identity_path()), CompileError::new(
                                "bynk.consumes.capability_name_clash",
                                cap.span,
                                format!(
                                    "flattened capability `{}` clashes with a capability declared locally — use qualified `given {target}.{}` instead",
                                    cap.name, cap.name
                                ),
                            ));
                            continue;
                        }
                        if let Some(prev) = flattened.get(&cap.name) {
                            // #1857: the same selection, repeated in another
                            // file of the context, is one selection.
                            if *prev == target && flattened_in.get(&cap.name) != Some(&i) {
                                refs.record_in_unit(
                                    cap.span,
                                    SymbolKind::Capability,
                                    &cap.name,
                                    &target,
                                );
                                continue;
                            }
                            errors.push_for(Some(&parsed[i].identity_path()), CompileError::new(
                                "bynk.consumes.capability_name_clash",
                                cap.span,
                                format!(
                                    "capability `{}` is flattened from both `{prev}` and `{target}` — qualify one with `given U.{}`",
                                    cap.name, cap.name
                                ),
                            ));
                            continue;
                        }
                        // v0.25: the selection list names the capability in
                        // the consumed unit (clause-position reference).
                        refs.record_in_unit(cap.span, SymbolKind::Capability, &cap.name, &target);
                        flattened.insert(cap.name.clone(), target.clone());
                        flattened_in.insert(cap.name.clone(), i);
                    }
                }
                if !consumes_targets.contains(&target) {
                    consumes_targets.push(target);
                }
            }
        }
        unit_consumes.insert(name.clone(), consumes_targets);
        unit_flattened.insert(name.clone(), flattened);
    }
    (unit_consumes, unit_flattened)
}

/// Phases 5b'/5b'': collect each context's `consumes` aliases (alias →
/// consumed-context name), reporting alias-vs-alias conflicts (5b'), then report
/// any alias that clashes with a locally-declared type/fn/capability/service/agent
/// (5b''). Returns the per-context alias maps; diagnostics go into `errors`.
pub fn phase_consumes_aliases(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &[ParsedFile],
    unit_tables: &HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
) -> HashMap<String, HashMap<String, String>> {
    let mut unit_consumes_aliases: HashMap<String, HashMap<String, String>> = HashMap::new();
    for (name, indices) in groups {
        let kind = *kinds.get(name).unwrap();
        if kind != UnitKind::Context {
            continue;
        }
        let mut aliases: HashMap<String, String> = HashMap::new();
        // #1857: where each alias, and each aliased target, was first seen:
        // `(the other half of the pair, file index, alias span)`. A context's
        // `consumes` clauses apply to all its files, so a file may repeat one
        // another file states, as it repeats `context`; only a pair that
        // disagrees is a conflict.
        let mut by_alias: HashMap<String, (String, usize, Span)> = HashMap::new();
        let mut by_target: HashMap<String, (String, usize, Span)> = HashMap::new();
        for &i in indices {
            for c in parsed[i].consumes() {
                let Some(alias) = &c.alias else { continue };
                let target = c.target.joined();
                if !unit_tables.contains_key(&target) {
                    // Already reported as unknown context above.
                    continue;
                }
                // #1857: a repeat is fine in *another* file; twice in one file
                // is still a conflict.
                let first = by_alias
                    .get(&alias.name)
                    .filter(|(t, f, _)| *t != target || *f == i)
                    .map(|(t, f, sp)| {
                        let what = if *t == target {
                            "the same clause appears earlier in this file".to_string()
                        } else {
                            format!("alias `{}` already names `{t}`", alias.name)
                        };
                        (what, *f, *sp)
                    })
                    .or_else(|| {
                        by_target
                            .get(&target)
                            .filter(|(a, _, _)| *a != alias.name)
                            .map(|(a, f, sp)| {
                                (format!("`{target}` is already consumed as `{a}`"), *f, *sp)
                            })
                    });
                if let Some((what, prev_file, prev_span)) = first {
                    // A label can point only into this file; another file's
                    // clause is named in the message.
                    let where_ = if prev_file == i {
                        String::new()
                    } else {
                        // `/`-separated on every platform, as the project
                        // layout is written (a Windows `display()` uses `\`).
                        format!(
                            " (first stated in `{}`)",
                            parsed[prev_file]
                                .identity_path()
                                .components()
                                .map(|c| c.as_os_str().to_string_lossy())
                                .collect::<Vec<_>>()
                                .join("/")
                        )
                    };
                    let mut err = CompileError::new(
                        "bynk.consumes.alias_conflict",
                        alias.span,
                        format!(
                            "`consumes {target} as {}` conflicts with another `consumes` clause in context `{name}`: {what}{where_}",
                            alias.name
                        ),
                    );
                    if prev_file == i {
                        err = err.with_label(prev_span, "previously defined here");
                    }
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        err.with_note(
                            "a context may consume a unit under one alias, and an alias may name one unit; repeating the same clause in another file is fine",
                        ),
                    );
                    continue;
                }
                if by_alias.contains_key(&alias.name) {
                    // #1857: the same clause, repeated in another file.
                    continue;
                }
                aliases.insert(alias.name.clone(), target.clone());
                by_alias.insert(alias.name.clone(), (target.clone(), i, alias.span));
                by_target.insert(target, (alias.name.clone(), i, alias.span));
            }
        }
        unit_consumes_aliases.insert(name.clone(), aliases);
    }

    // -- 5b''. Detect alias-vs-local-decl conflicts. An alias must not clash
    //          with any locally declared type/fn/capability/service/agent.
    for (name, aliases) in &unit_consumes_aliases {
        let Some(local) = unit_tables.get(name) else {
            continue;
        };
        for alias in aliases.keys() {
            let alias_site = parsed_alias_span(parsed, &groups[name], alias);
            let alias_span = alias_site.map(|(_, s)| s).unwrap_or_default();
            let alias_file = alias_site.map(|(i, _)| parsed[i].identity_path());
            let conflict_kind = if local.types.contains_key(alias) {
                Some("type")
            } else if local.fns.contains_key(alias) {
                Some("function")
            } else if local.capabilities.contains_key(alias) {
                Some("capability")
            } else if local.services.contains_key(alias) {
                Some("service")
            } else if local.agents.contains_key(alias) {
                Some("agent")
            } else {
                None
            };
            if let Some(kind) = conflict_kind {
                errors.push_for(alias_file.as_deref(),
                    CompileError::new(
                        "bynk.consumes.alias_conflict",
                        alias_span,
                        format!(
                            "alias `{alias}` conflicts with a local {kind} of the same name in context `{name}`",
                        ),
                    )
                    .with_note(
                        "pick a different alias for the `consumes` clause, or rename the local declaration",
                    ),
                );
            }
        }
    }
    unit_consumes_aliases
}

/// Phase 6: for each unit, detect when two `uses`-imported commons declare the
/// same (non-shadowed) type or function name — an unrenamable conflict at the use
/// site. Diagnostics go into `errors`.
pub fn phase_uses_name_conflicts(
    unit_uses: &HashMap<String, Vec<String>>,
    unit_tables: &HashMap<String, UnitTable>,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    errors: &mut ErrorSink,
) {
    for (name, targets) in unit_uses {
        let local = unit_tables.get(name).expect("unit table present");
        let mut imported: HashMap<String, String> = HashMap::new();
        for t in targets {
            let used = unit_tables.get(t).expect("used unit table present");
            for type_name in used.types.keys() {
                // Unlike the fn and asset loops below, a local service, agent,
                // provider or asset does not skip this (#1927, review of
                // #1933). It hides an imported type from naming, but the type
                // stays composed so its values resolve, and with two used
                // commons declaring it, which one the unit's values carry is
                // still ambiguous.
                if local.types.contains_key(type_name) || local.fns.contains_key(type_name) {
                    continue;
                }
                if let Some(prev) = imported.get(type_name) {
                    let site = uses_span_of(parsed, &groups[name], t);
                    let span = site.map(|(_, s)| s).unwrap_or_default();
                    let file = site.map(|(i, _)| parsed[i].identity_path());
                    errors.push_for(file.as_deref(),
                        CompileError::new(
                            "bynk.uses.name_conflict",
                            span,
                            format!(
                                "`{name}` uses two commons that both declare `{type_name}`: `{prev}` and `{t}`",
                            ),
                        )
                        .with_note(
                            "name conflicts at the use site are not yet renamable; remove or restructure one of the imports",
                        ),
                    );
                } else {
                    imported.insert(type_name.clone(), t.clone());
                }
            }
            for fn_name in used.fns.keys() {
                // #1925: any local value hides the import (`shadowed_imports`),
                // so two used commons declaring it leave nothing ambiguous.
                if crate::symbols::local_value_kind(local, fn_name).is_some() {
                    continue;
                }
                if let Some(prev) = imported.get(fn_name) {
                    let site = uses_span_of(parsed, &groups[name], t);
                    let span = site.map(|(_, s)| s).unwrap_or_default();
                    let file = site.map(|(i, _)| parsed[i].identity_path());
                    errors.push_for(file.as_deref(),
                        CompileError::new(
                            "bynk.uses.name_conflict",
                            span,
                            format!(
                                "`{name}` uses two commons that both declare `{fn_name}`: `{prev}` and `{t}`",
                            ),
                        )
                        .with_note(
                            "name conflicts at the use site are not yet renamable; remove or restructure one of the imports",
                        ),
                    );
                } else {
                    imported.insert(fn_name.clone(), t.clone());
                }
            }
            // #1883, slice 1a ([DECISION G]): a commons' assets come in
            // through `uses` like its functions, and share their namespace.
            for asset_name in used.assets.keys() {
                // #1925: any local value hides the import, as above.
                if crate::symbols::local_value_kind(local, asset_name).is_some() {
                    continue;
                }
                if let Some(prev) = imported.get(asset_name) {
                    let site = uses_span_of(parsed, &groups[name], t);
                    let span = site.map(|(_, s)| s).unwrap_or_default();
                    let file = site.map(|(i, _)| parsed[i].identity_path());
                    errors.push_for(file.as_deref(),
                        CompileError::new(
                            "bynk.uses.name_conflict",
                            span,
                            format!(
                                "`{name}` uses two commons that both declare `{asset_name}`: `{prev}` and `{t}`",
                            ),
                        )
                        .with_note(
                            "name conflicts at the use site are not yet renamable; remove or restructure one of the imports",
                        ),
                    );
                } else {
                    imported.insert(asset_name.clone(), t.clone());
                }
            }
            // #2387, slice 1: a commons' embeds come in through `uses` the
            // same way, in the same namespace.
            for embed_name in used.embeds.keys() {
                if crate::symbols::local_value_kind(local, embed_name).is_some() {
                    continue;
                }
                if let Some(prev) = imported.get(embed_name) {
                    let site = uses_span_of(parsed, &groups[name], t);
                    let span = site.map(|(_, s)| s).unwrap_or_default();
                    let file = site.map(|(i, _)| parsed[i].identity_path());
                    errors.push_for(file.as_deref(),
                        CompileError::new(
                            "bynk.uses.name_conflict",
                            span,
                            format!(
                                "`{name}` uses two commons that both declare `{embed_name}`: `{prev}` and `{t}`",
                            ),
                        )
                        .with_note(
                            "name conflicts at the use site are not yet renamable; remove or restructure one of the imports",
                        ),
                    );
                } else {
                    imported.insert(embed_name.clone(), t.clone());
                }
            }
        }
    }
}

/// message-bundles slice 1 (#859): messages-block legality, `@reference`
/// cardinality, within-block duplicate codes, and the `uses bynk.locale`
/// dependency. Runs here (not in `phase_group`) because it needs `unit_uses`,
/// resolved just above.
///
/// P5.0 (`design/tracks/semantics-in-the-checker.md` §6): relocated verbatim
/// from `bynk-emit/src/project/validate.rs`'s `check_messages_bundles` — one
/// of the two live editor-diagnostics regressions this slice closes (category
/// 2 of `analysis.rs`'s own seven-category accounting). Cross-locale
/// completeness (`bynk.messages.incomplete`, only for codes present in the
/// reference locale but not this one — a locale-specific-only code is not an
/// error, per the "reference is a floor, not a ceiling" convention) and
/// cross-locale placeholder-*set* agreement (`bynk.messages.placeholder_mismatch`,
/// only for codes present in both — a missing code is `incomplete`'s job, not
/// this one's). Two blocks declaring the same locale tag are rejected outright
/// (`bynk.resolve.duplicate_message_locale`, PR #875 review) — the emitter
/// has no dedup of its own, so a silent last-wins here would let a hard
/// `tsc` redeclare error (two colliding `const __messages_<tag>`
/// declarations) through instead.
pub fn phase_messages_bundles(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_uses: &HashMap<String, Vec<String>>,
    errors: &mut ErrorSink,
) {
    for (name, indices) in groups {
        let mut first_messages: Option<(usize, Span)> = None;
        let mut reference_sites: Vec<(usize, Span)> = Vec::new();
        let mut reference_block: Option<(usize, &MessagesDecl)> = None;
        let mut by_tag: HashMap<&str, (usize, &MessagesDecl)> = HashMap::new();
        for &i in indices {
            for item in parsed[i].items() {
                let CommonsItem::Messages(m) = item else {
                    continue;
                };
                if first_messages.is_none() {
                    first_messages = Some((i, m.span));
                }
                if kinds.get(name) != Some(&UnitKind::Commons) {
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.messages.outside_commons",
                            m.span,
                            "`messages` declarations are only allowed inside a commons, not a context or adapter",
                        ),
                    );
                    continue;
                }
                // #899: the tag is a `LocaleTag` string literal, checked here
                // against `LocaleTag`'s own refinement (read from the
                // firstparty `bynk.locale.types` source, so the pattern has one
                // definition). An invalid tag would otherwise reach `Intl` at
                // runtime as `new Intl.PluralRules("xx")`, which throws — the
                // opposite of `render`'s totality contract.
                if !checker::locale_tag_accepts(&m.tag) {
                    let pattern = checker::locale_tag_pattern().unwrap_or("");
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.messages.invalid_locale_tag",
                            m.tag_span,
                            format!(
                                "\"{}\" is not a valid `LocaleTag` — it must match the pattern `{}`",
                                m.tag, pattern
                            ),
                        ),
                    );
                }
                // message-bundles slice 2 (#874, PR #875 review): two blocks
                // declaring the same locale tag are rejected, not
                // last-write-wins — the emitter (`emit_messages_bundle`) has no
                // dedup of its own and would emit two colliding table entries
                // under one object key, a hard `tsc` error. Mirrors
                // `bynk.resolve.duplicate_fn`'s own shape: only the *first*
                // occurrence seeds `by_tag`, so a third duplicate still reports
                // against the original, not the second.
                if let Some(&(_, prev)) = by_tag.get(m.tag.as_str()) {
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.resolve.duplicate_message_locale",
                            m.tag_span,
                            format!("locale \"{}\" is already declared in this bundle", m.tag),
                        )
                        .with_label(prev.tag_span, "previously declared here"),
                    );
                } else {
                    by_tag.insert(m.tag.as_str(), (i, m));
                }
                for ann in &m.annotations {
                    if ann.name.name == "reference" {
                        reference_sites.push((i, ann.span));
                        reference_block = Some((i, m));
                    }
                }
                let mut seen: HashMap<&str, Span> = HashMap::new();
                for entry in &m.entries {
                    if let Some(prev) = seen.get(entry.code.as_str()) {
                        errors.push_for(
                            Some(&parsed[i].identity_path()),
                            CompileError::new(
                                "bynk.resolve.duplicate_message_code",
                                entry.code_span,
                                format!(
                                    "message code \"{}\" is already declared in this block",
                                    entry.code
                                ),
                            )
                            .with_label(*prev, "previously declared here"),
                        );
                    } else {
                        seen.insert(entry.code.as_str(), entry.code_span);
                    }
                    // message-bundles slice 3 (#878): runs unconditionally,
                    // once per entry, regardless of `@reference` cardinality
                    // — malformed ICU syntax shouldn't wait on cardinality
                    // being resolved first.
                    check_entry_icu_syntax(entry, Some(&parsed[i].identity_path()), errors);
                }
            }
        }
        let Some((first_i, first_span)) = first_messages else {
            continue;
        };
        if kinds.get(name) != Some(&UnitKind::Commons) {
            // Already reported above (outside_commons) for every block;
            // cardinality/uses checks don't apply to a non-commons unit.
            continue;
        }
        match reference_sites.len() {
            0 => {
                errors.push_for(
                    Some(&parsed[first_i].identity_path()),
                    CompileError::new(
                        "bynk.messages.missing_reference",
                        first_span,
                        "a message bundle must have exactly one `@reference` block; none found",
                    ),
                );
            }
            1 => {
                // message-bundles slice 2 (#874): "the reference" is only
                // well-defined here — 0 or 2+ already reported their own
                // diagnostic above, and completeness/placeholder-agreement
                // against an ambiguous or absent reference would be noise.
                let (_, reference) = reference_block
                    .expect("reference_sites.len() == 1 implies reference_block is Some");
                // Sorted for deterministic diagnostic order — `by_tag`'s
                // HashMap iteration is not otherwise stable across runs.
                let mut sorted_tags: Vec<&&str> = by_tag.keys().collect();
                sorted_tags.sort();
                for &&tag in &sorted_tags {
                    let &(locale_i, locale_m) = &by_tag[tag];
                    if tag == reference.tag.as_str() {
                        continue;
                    }
                    for ref_entry in &reference.entries {
                        let Some(locale_entry) =
                            locale_m.entries.iter().find(|e| e.code == ref_entry.code)
                        else {
                            errors.push_for(
                                Some(&parsed[locale_i].identity_path()),
                                CompileError::new(
                                    "bynk.messages.incomplete",
                                    locale_m.span,
                                    format!(
                                        "locale \"{tag}\" is missing code \"{}\", declared by the reference locale \"{}\"",
                                        ref_entry.code, reference.tag
                                    ),
                                ),
                            );
                            continue;
                        };
                        let ref_names = icu::placeholder_names(&ref_entry.template);
                        let locale_names = icu::placeholder_names(&locale_entry.template);
                        if ref_names != locale_names {
                            errors.push_for(
                                Some(&parsed[locale_i].identity_path()),
                                CompileError::new(
                                    "bynk.messages.placeholder_mismatch",
                                    locale_entry.template_span,
                                    format!(
                                        "locale \"{tag}\"'s template for code \"{}\" uses placeholders {locale_names:?}, but the reference locale \"{}\"'s uses {ref_names:?}",
                                        ref_entry.code, reference.tag
                                    ),
                                ),
                            );
                        }
                        // message-bundles slice 3 (#878, Decision D): a name
                        // present in both templates must also agree on ICU
                        // format *kind* (plain/plural/select/number/date) —
                        // a UI can't sanely alternate that per locale. A
                        // missing name is `placeholder_mismatch`'s job, not
                        // this one's; a malformed template's kinds are
                        // silently absent from `template_format_kinds`
                        // (already reported once by `check_entry_icu_syntax`
                        // above, never double-reported here).
                        let ref_kinds = icu::template_format_kinds(&ref_entry.template);
                        let locale_kinds = icu::template_format_kinds(&locale_entry.template);
                        for (pname, ref_kind) in &ref_kinds {
                            let Some(locale_kind) = locale_kinds.get(pname) else {
                                continue;
                            };
                            if locale_kind != ref_kind {
                                errors.push_for(
                                    Some(&parsed[locale_i].identity_path()),
                                    CompileError::new(
                                        "bynk.messages.format_mismatch",
                                        locale_entry.template_span,
                                        format!(
                                            "locale \"{tag}\"'s placeholder \"{pname}\" in code \"{}\" is formatted as {}, but the reference locale \"{}\"'s is {}",
                                            ref_entry.code,
                                            locale_kind.as_str(),
                                            reference.tag,
                                            ref_kind.as_str(),
                                        ),
                                    ),
                                );
                            }
                        }
                    }
                }
            }
            _ => {
                let (_, first_ref_span) = reference_sites[0];
                for &(i, span) in &reference_sites[1..] {
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.messages.multiple_reference",
                            span,
                            "a message bundle must have exactly one `@reference` block; found more than one",
                        )
                        .with_label(first_ref_span, "first `@reference` here"),
                    );
                }
            }
        }
        // Locale-negotiation-slice-2 follow-up (#886): the synthetic `render`
        // this commons gets (`synthetic_render_fn`, symbols.rs) names
        // `LocaleTag`/`Message` by `TypeRef::Named` — real, resolved
        // references, not bypassed — so both `bynk.locale` (for `render`
        // itself) and `bynk.locale.types` (for the types its signature
        // names) must be `uses`d. Kept as one diagnostic, not two: a message
        // bundle always needs both together, so splitting the code would
        // just be two author-facing fixes for one underlying requirement.
        let targets = unit_uses.get(name);
        let has_locale_uses =
            targets.is_some_and(|targets| targets.iter().any(|t| t == firstparty::LOCALE_UNIT));
        let has_locale_types_uses = targets
            .is_some_and(|targets| targets.iter().any(|t| t == firstparty::LOCALE_TYPES_UNIT));
        if !has_locale_uses || !has_locale_types_uses {
            let missing = match (has_locale_uses, has_locale_types_uses) {
                (false, false) => "`bynk.locale` and `bynk.locale.types`",
                (false, true) => "`bynk.locale`",
                (true, false) => "`bynk.locale.types`",
                (true, true) => unreachable!("at least one of the two is missing here"),
            };
            errors.push_for(
                Some(&parsed[first_i].identity_path()),
                CompileError::new(
                    "bynk.messages.missing_locale_dependency",
                    first_span,
                    format!("a commons declaring `messages` must also `uses` {missing}"),
                ),
            );
        }
    }
}

/// Locale capability track, slice 2 (#882): a context whose direct `uses`
/// reaches two or more message-bundle commons has no principled single
/// answer for what `Locale.current()` should negotiate against — but this
/// is only worth diagnosing when the context actually `consumes bynk {
/// Locale }` at all; a context with 2+ bundles that never touches `Locale`
/// has nothing ambiguous to resolve.
///
/// P5.0 (`design/tracks/semantics-in-the-checker.md` §6): relocated verbatim
/// from `bynk-emit/src/project/validate.rs`'s `check_locale_bundle_ambiguity`
/// — category 3 of `analysis.rs`'s own seven-category accounting, the second
/// of this slice's two live editor-diagnostics regressions.
pub fn phase_locale_bundle_ambiguity(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    errors: &mut ErrorSink,
) {
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context) {
            continue;
        }
        let ContextMessageBundle::Many(bundles) =
            detect_context_message_bundle(name, unit_uses, groups, kinds, parsed)
        else {
            continue;
        };
        let consumes_locale = unit_flattened
            .get(name)
            .and_then(|m| m.get("Locale"))
            .is_some_and(|owner| owner == firstparty::BYNK_UNIT);
        if !consumes_locale {
            continue;
        }
        for &i in indices {
            for c in parsed[i].consumes() {
                if c.target.joined() != firstparty::BYNK_UNIT {
                    continue;
                }
                let Some(locale_ident) = c.selected.iter().flatten().find(|id| id.name == "Locale")
                else {
                    continue;
                };
                let mut err = CompileError::new(
                    "bynk.locale.multiple_message_bundles",
                    locale_ident.span,
                    format!(
                        "context `{name}` uses {} message bundles ({}) — `Locale.current()` has no single bundle to negotiate against",
                        bundles.len(),
                        bundles.join(", "),
                    ),
                );
                for &j in indices {
                    for u in parsed[j].uses() {
                        if bundles.contains(&u.target.joined()) {
                            err = err
                                .with_label(u.span, format!("`{}` used here", u.target.joined()));
                        }
                    }
                }
                errors.push_for(Some(&parsed[i].identity_path()), err);
            }
        }
    }
}

/// A message bundle entry's ICU template, syntax-checked at parse time
/// against the ICU MessageFormat grammar `icu.rs` implements. Relocated
/// alongside `phase_messages_bundles` (P5.0) — its only caller.
fn check_entry_icu_syntax(entry: &MessageEntry, file: Option<&Path>, errors: &mut ErrorSink) {
    for (inner_offset, inner) in icu::icu_dispatch_placeholders(&entry.template) {
        if let Err(e) = icu::parse_icu_placeholder(inner) {
            let decoded_start = inner_offset + e.offset;
            let decoded_span = Span::new(decoded_start, decoded_start + e.len);
            let raw_span = decoded_span.offset(entry.template_span.start + 1);
            errors.push_for(
                file,
                CompileError::new(
                    "bynk.messages.malformed_icu_syntax",
                    raw_span,
                    e.kind.message(),
                ),
            );
        }
    }
}

/// Events track, slice 0 (spine #936): a `from Events(E)` subscription must
/// name a real, declared event — owned either by this context or by a
/// context it `consumes` (mirroring `discover_event_subscribers`'s own
/// ownership resolution, `project.rs`, which silently drops an unresolvable
/// subscription rather than diagnosing it). Runs at the project-wide phase
/// (needs `unit_tables` + `unit_consumes` together, unlike the local, per-
/// context `check_service_protocols`), alongside the other cross-unit checks
/// that need the same two maps.
///
/// P5.1 (`design/tracks/semantics-in-the-checker.md` §6): relocated verbatim
/// from `bynk-emit/src/project/validate.rs`'s `check_event_subscriptions` —
/// category 4 of `analysis.rs`'s own seven-category accounting, the third
/// live editor-diagnostics regression this track closes.
pub fn phase_event_subscriptions(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_uses: &HashMap<String, Vec<String>>,
    errors: &mut ErrorSink,
) {
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context) {
            continue;
        }
        let consumed = unit_consumes.get(name).cloned().unwrap_or_default();
        for &i in indices {
            for item in parsed[i].items() {
                let CommonsItem::Service(s) = item else {
                    continue;
                };
                let ServiceProtocol::Events {
                    event_type,
                    pattern,
                    schema_dispatch,
                } = &s.protocol
                else {
                    continue;
                };
                // Events track, slice 4 (spine #936): `via schema(N)`'s
                // legality needs nothing about the subscribed event itself
                // (unlike the payload pattern below), so it's checked
                // independently of whether the subscription even resolves.
                if let Some(dispatch) = schema_dispatch {
                    check_schema_dispatch(dispatch, &parsed[i].identity_path(), errors);
                }
                let TypeRef::Named(id) = event_type else {
                    continue;
                };
                let owner_locally = unit_tables
                    .get(name)
                    .filter(|t| t.events.contains_key(&id.name))
                    .map(|_| name.clone());
                let owner_consumed = consumed.iter().find(|c| {
                    unit_tables
                        .get(*c)
                        .is_some_and(|t| t.events.contains_key(&id.name))
                });
                let owner = owner_locally
                    .as_deref()
                    .or(owner_consumed.map(String::as_str));
                let Some(owner) = owner else {
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.event.unknown_subscription",
                            id.span,
                            format!(
                                "`{}` is not a declared event in this context or any consumed context",
                                id.name
                            ),
                        )
                        .with_note(
                            "check the spelling, or add `consumes <context>` for the context whose `event` this names — an unresolvable subscription never receives anything, silently",
                        ),
                    );
                    continue;
                };
                // Events track, slice 1 (spine #936): once the event itself
                // resolves, check the subscription pattern's fields against
                // its declared record shape. No pattern is the pattern-less
                // form (slice 0) and needs none of this.
                let Some(pattern) = pattern else {
                    continue;
                };
                let Some(event_decl) = unit_tables.get(owner).and_then(|t| t.events.get(&id.name))
                else {
                    continue;
                };
                check_event_pattern(
                    pattern,
                    event_decl,
                    owner,
                    unit_tables,
                    unit_uses,
                    &parsed[i].identity_path(),
                    errors,
                );
            }
        }
    }
}

/// Events track, slice 1 (spine #936): resolve a subscription pattern's
/// fields/values against the owning event's declared record shape. `owner`
/// is the context that declares `event_decl` (may differ from the
/// subscribing context, reached via `consumes`) — a field's own type (e.g. a
/// discriminator sum like `Region`) resolves against the *owner's* types
/// (locally declared, or pulled in via the owner's own `uses <commons>`),
/// mirroring how the field's type is resolved everywhere else the event's
/// record shape is used.
fn check_event_pattern(
    pattern: &EventPattern,
    event_decl: &EventDecl,
    owner: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    identity_path: &std::path::Path,
    errors: &mut ErrorSink,
) {
    let mut seen: HashSet<String> = HashSet::new();
    for field in &pattern.fields {
        if !seen.insert(field.name.name.clone()) {
            errors.push_for(
                Some(identity_path),
                CompileError::new(
                    "bynk.event.pattern_duplicate_field",
                    field.name.span,
                    format!(
                        "field `{}` is matched more than once in this subscription pattern",
                        field.name.name
                    ),
                ),
            );
            continue;
        }
        let Some(record_field) = event_decl
            .body
            .fields
            .iter()
            .find(|f| f.name.name == field.name.name)
        else {
            let known: Vec<&str> = event_decl
                .body
                .fields
                .iter()
                .map(|f| f.name.name.as_str())
                .collect();
            errors.push_for(
                Some(identity_path),
                CompileError::new(
                    "bynk.event.pattern_unknown_field",
                    field.name.span,
                    format!(
                        "`{}` has no field named `{}`",
                        event_decl.name.name, field.name.name
                    ),
                )
                .with_note(format!(
                    "declared fields: {}",
                    if known.is_empty() {
                        "(none)".to_string()
                    } else {
                        known.join(", ")
                    }
                )),
            );
            continue;
        };
        check_event_pattern_value(
            &field.value,
            record_field,
            owner,
            unit_tables,
            unit_uses,
            identity_path,
            errors,
        );
    }
}

/// Events track, slice 4 (spine #936): `via schema(N)`'s `N` must be a
/// positive `Int` literal — the identical rule `@schema(N)` already
/// enforces (`bynk.event.bad_schema_version`), reused under its own code
/// since the two are unrelated syntax positions (an annotation on the
/// event's own declaration vs. a clause on a subscriber's header).
fn check_schema_dispatch(
    dispatch: &SchemaDispatch,
    identity_path: &std::path::Path,
    errors: &mut ErrorSink,
) {
    let SchemaVersionPattern::Literal(n) = &dispatch.pattern;
    if *n <= 0 {
        errors.push_for(
            Some(identity_path),
            CompileError::new(
                "bynk.event.bad_schema_dispatch",
                dispatch.span,
                "`via schema(...)`'s argument must be a positive `Int` literal",
            ),
        );
    }
}

/// Resolve one pattern field's matched value against that field's declared
/// type — a literal must match the field's base type; a variant must name a
/// nullary member of the field's sum type.
fn check_event_pattern_value(
    value: &EventPatternValue,
    record_field: &RecordField,
    owner: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    identity_path: &std::path::Path,
    errors: &mut ErrorSink,
) {
    match value {
        EventPatternValue::Literal { value: lit, span } => {
            // A base type (`Int`/`String`/`Bool`/…) is its own `TypeRef`
            // variant, not `TypeRef::Named` — only a *user*-declared type
            // (including a refined/opaque type built on a base) goes through
            // `resolve_type_decl`. An earlier version of this match only
            // handled the `Named` case, so a plain `orderId: String` field
            // (the common case) fell through to "not a literal-kind type",
            // caught by `events_workers_wiring.rs`'s patterned fixture.
            let base = match &record_field.type_ref {
                TypeRef::Base(b, _) => Some(*b),
                TypeRef::Named(field_type_name) => {
                    resolve_type_decl(unit_tables, unit_uses, owner, &field_type_name.name)
                        .and_then(|d| match &d.body {
                            TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } => {
                                Some(*base)
                            }
                            _ => None,
                        })
                }
                _ => None,
            };
            let Some(base) = base else {
                errors.push_for(
                    Some(identity_path),
                    CompileError::new(
                        "bynk.event.pattern_type_mismatch",
                        *span,
                        format!(
                            "field `{}` is not a literal-kind type — a literal pattern value cannot match it",
                            record_field.name.name
                        ),
                    ),
                );
                return;
            };
            let kind_matches = matches!(
                (lit, base),
                (LiteralValue::Int(_), BaseType::Int)
                    | (LiteralValue::Str(_), BaseType::String)
                    | (LiteralValue::Bool(_), BaseType::Bool)
            );
            if !kind_matches {
                errors.push_for(
                    Some(identity_path),
                    CompileError::new(
                        "bynk.event.pattern_type_mismatch",
                        *span,
                        format!(
                            "this literal does not match the type of field `{}` (`{}`)",
                            record_field.name.name,
                            type_ref_to_display(&record_field.type_ref)
                        ),
                    ),
                );
            }
        }
        EventPatternValue::Variant {
            type_name,
            variant,
            span,
        } => {
            let TypeRef::Named(field_type_name) = &record_field.type_ref else {
                errors.push_for(
                    Some(identity_path),
                    CompileError::new(
                        "bynk.event.pattern_type_mismatch",
                        *span,
                        format!(
                            "field `{}` is not a sum type — a variant pattern value cannot match it",
                            record_field.name.name
                        ),
                    ),
                );
                return;
            };
            if let Some(qualifier) = type_name
                && qualifier.name != field_type_name.name
            {
                errors.push_for(
                    Some(identity_path),
                    CompileError::new(
                        "bynk.event.pattern_type_mismatch",
                        qualifier.span,
                        format!(
                            "field `{}` has type `{}`, not `{}`",
                            record_field.name.name, field_type_name.name, qualifier.name
                        ),
                    ),
                );
                return;
            }
            let Some(decl) =
                resolve_type_decl(unit_tables, unit_uses, owner, &field_type_name.name)
            else {
                // The field's own type failed to resolve — a different,
                // pre-existing check (ordinary type-reference resolution)
                // already reports this; don't double-report it here.
                return;
            };
            let TypeBody::Sum(sum) = &decl.body else {
                errors.push_for(
                    Some(identity_path),
                    CompileError::new(
                        "bynk.event.pattern_type_mismatch",
                        *span,
                        format!(
                            "field `{}` has type `{}`, which is not a sum type",
                            record_field.name.name, field_type_name.name
                        ),
                    ),
                );
                return;
            };
            let Some(member) = sum.variants.iter().find(|v| v.name.name == variant.name) else {
                errors.push_for(
                    Some(identity_path),
                    CompileError::new(
                        "bynk.event.pattern_unknown_variant",
                        variant.span,
                        format!(
                            "`{}` has no variant named `{}`",
                            field_type_name.name, variant.name
                        ),
                    ),
                );
                return;
            };
            if !member.payload.is_empty() {
                errors.push_for(
                    Some(identity_path),
                    CompileError::new(
                        "bynk.event.pattern_variant_payload",
                        variant.span,
                        format!(
                            "`{}.{}` carries a payload — only a nullary variant may be matched here, since testing the tag alone would silently ignore the payload",
                            field_type_name.name, variant.name
                        ),
                    ),
                );
            }
        }
    }
}

/// Resolve a named type as `owner` sees it: the context's own `types` first,
/// then any commons unit it `uses`. Events track slice 1 (spine #936) needs
/// this because a pattern field's type (e.g. a discriminator sum) may be
/// declared in a commons the event's owning context pulls in with `uses`,
/// rather than in the context itself.
fn resolve_type_decl<'a>(
    unit_tables: &'a HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    owner: &str,
    name: &str,
) -> Option<&'a Arc<TypeDecl>> {
    if let Some(t) = unit_tables.get(owner).and_then(|t| t.types.get(name)) {
        return Some(t);
    }
    for used in unit_uses.get(owner).into_iter().flatten() {
        if let Some(t) = unit_tables.get(used).and_then(|t| t.types.get(name)) {
            return Some(t);
        }
    }
    None
}

/// Phase 6b: validate each context/adapter's `exports opaque/transparent { … }`
/// clauses — every name must be a locally-declared type, with no duplicates
/// within a clause or conflicting visibilities across clauses. Returns unit →
/// (type → visibility); diagnostics go into `errors` and export references into
/// `refs`.
pub fn phase_validate_type_exports(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &[ParsedFile],
    unit_tables: &HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
    refs: &mut RefSink,
) -> HashMap<String, HashMap<String, Visibility>> {
    let mut exports_visibility: HashMap<String, HashMap<String, Visibility>> = HashMap::new();
    for (name, indices) in groups {
        let kind = *kinds.get(name).unwrap();
        if kind != UnitKind::Context && kind != UnitKind::Adapter {
            // Commons may not have exports clauses (parsed grammar prevents it
            // at the parser level), but in case any sneak in, skip.
            continue;
        }
        let local = unit_tables.get(name).unwrap();
        let mut seen: HashMap<String, (Visibility, Span)> = HashMap::new();
        for &i in indices {
            refs.enter_file(&parsed[i].identity_path(), name, parsed[i].is_synthetic());
            for clause in parsed[i].exports() {
                // v0.15: `exports capability { ... }` clauses are validated
                // separately (§4.1); 6b handles only type exports.
                let ExportKind::Type(clause_vis) = clause.kind else {
                    continue;
                };
                let mut within: HashMap<String, Span> = HashMap::new();
                for n in clause.names.iter().map(|e| &e.name) {
                    if let Some(prev) = within.get(&n.name) {
                        errors.push_for(
                            Some(&parsed[i].identity_path()),
                            CompileError::new(
                                "bynk.exports.duplicate_in_clause",
                                n.span,
                                format!(
                                    "type `{}` appears more than once in this exports clause",
                                    n.name
                                ),
                            )
                            .with_label(*prev, "previously listed here"),
                        );
                        continue;
                    }
                    within.insert(n.name.clone(), n.span);

                    if !local.types.contains_key(&n.name) {
                        errors.push_for(Some(&parsed[i].identity_path()),
                            CompileError::new(
                                "bynk.exports.undeclared_type",
                                n.span,
                                format!(
                                    "exports clause references `{}`, which is not a type declared in context `{}`",
                                    n.name, name
                                ),
                            )
                            .with_note(
                                "only types declared in the same context can appear in `exports` clauses",
                            ),
                        );
                        continue;
                    }
                    // v0.25: `exports opaque/transparent { T }` names the type.
                    refs.record(n.span, SymbolKind::Type, &n.name);

                    if let Some((prev_vis, prev_span)) = seen.get(&n.name) {
                        if *prev_vis == clause_vis {
                            errors.push_for(
                                Some(&parsed[i].identity_path()),
                                CompileError::new(
                                    "bynk.exports.duplicate_export",
                                    n.span,
                                    format!("type `{}` is exported more than once", n.name),
                                )
                                .with_label(*prev_span, "previously exported here"),
                            );
                        } else {
                            errors.push_for(Some(&parsed[i].identity_path()),
                                CompileError::new(
                                    "bynk.exports.conflicting_visibility",
                                    n.span,
                                    format!(
                                        "type `{}` is exported with conflicting visibilities — pick `opaque` or `transparent`",
                                        n.name,
                                    ),
                                )
                                .with_label(*prev_span, "previously exported here"),
                            );
                        }
                        continue;
                    }
                    seen.insert(n.name.clone(), (clause_vis, n.span));
                }
            }
        }
        let mut visibility_map: HashMap<String, Visibility> = HashMap::new();
        for (n, (v, _)) in seen {
            visibility_map.insert(n, v);
        }
        exports_visibility.insert(name.clone(), visibility_map);
    }
    exports_visibility
}

/// Phase 6b': validate each context/adapter's `exports capability { … }` clauses
/// (v0.15 §4.1) — every name must be a capability the unit declares *and*
/// provides, with no duplicate exports. Diagnostics go into `errors` and export
/// references into `refs`.
pub fn phase_validate_capability_exports(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &[ParsedFile],
    unit_tables: &HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
    refs: &mut RefSink,
) {
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context)
            && kinds.get(name) != Some(&UnitKind::Adapter)
        {
            continue;
        }
        let local = unit_tables.get(name).unwrap();
        let mut seen: HashMap<String, Span> = HashMap::new();
        for &i in indices {
            refs.enter_file(&parsed[i].identity_path(), name, parsed[i].is_synthetic());
            for clause in parsed[i].exports() {
                if !matches!(clause.kind, ExportKind::Capability) {
                    continue;
                }
                for n in clause.names.iter().map(|e| &e.name) {
                    if let Some(prev) = seen.get(&n.name) {
                        errors.push_for(
                            Some(&parsed[i].identity_path()),
                            CompileError::new(
                                "bynk.exports.duplicate_export",
                                n.span,
                                format!("capability `{}` is exported more than once", n.name),
                            )
                            .with_label(*prev, "previously exported here"),
                        );
                        continue;
                    }
                    seen.insert(n.name.clone(), n.span);
                    if local.capabilities.contains_key(&n.name) {
                        // v0.25: `exports capability { Cap }` names the
                        // capability.
                        refs.record(n.span, SymbolKind::Capability, &n.name);
                    }
                    if !local.capabilities.contains_key(&n.name) {
                        errors.push_for(Some(&parsed[i].identity_path()),
                            CompileError::new(
                                "bynk.exports.undeclared_capability",
                                n.span,
                                format!(
                                    "`exports capability` references `{}`, which is not a capability declared in context `{}`",
                                    n.name, name
                                ),
                            )
                            .with_note(
                                "only capabilities declared in the same context can appear in `exports capability` clauses",
                            ),
                        );
                        continue;
                    }
                    // S4a (#1966): a derived capability is refused as an export
                    // in its own right (`bynk.exports.derived_capability`); its
                    // provision is implied, so "no provider" would mislead.
                    let derived = local
                        .capabilities
                        .get(&n.name)
                        .is_some_and(|c| c.derived.is_some());
                    if !derived && !local.providers.contains_key(&n.name) {
                        errors.push_for(Some(&parsed[i].identity_path()),
                            CompileError::new(
                                "bynk.exports.capability_not_provided",
                                n.span,
                                format!(
                                    "exported capability `{}` has no provider in context `{}` — a consumer cannot instantiate it",
                                    n.name, name
                                ),
                            )
                            .with_note(format!(
                                "add a `provides {} = …` declaration so the capability can be wired into consumers",
                                n.name
                            )),
                        );
                    }
                }
            }
        }
    }
}

/// Phase 6c: validate that every (non-external) provider matches its capability
/// exactly — each capability op has a provider op, and every provider op has a
/// matching capability op with the same parameter and return types. Diagnostics
/// go into `errors`.
pub fn phase_validate_providers(
    unit_tables: &HashMap<String, UnitTable>,
    // #696: the merged `UnitTable` has flattened a unit's files away, so provider
    // diagnostics need the group's files to recover which one declares each
    // provider and attribute the diagnostic to it.
    groups: &BTreeMap<String, Vec<usize>>,
    parsed: &[ParsedFile],
    // #1710: each unit's declarations recovery skipped (`phase_parse`).
    broken: &BrokenDeclNames,
    errors: &mut ErrorSink,
    tys: &Arc<Types>,
) {
    for (name, table) in unit_tables {
        // Map each provided capability to the project-relative path of the file
        // that declares its provider — every diagnostic below carries a span into
        // that file.
        let provider_files: HashMap<&str, PathBuf> = groups
            .get(name)
            .map(|indices| {
                indices
                    .iter()
                    .flat_map(|&i| {
                        parsed[i].items().iter().filter_map(move |item| match item {
                            CommonsItem::Provider(p) => {
                                Some((p.capability.name.as_str(), parsed[i].identity_path()))
                            }
                            _ => None,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        for (cap_name, provider) in &table.providers {
            let provider_file = provider_files.get(cap_name.as_str()).map(|p| p.as_path());
            // v0.17: an external provider has no Bynk body to match against the
            // capability — its implementation is the binding, checked by `tsc`.
            if provider.external {
                continue;
            }
            let Some(cap) = table.capabilities.get(cap_name) else {
                // #1710: a capability recovery skipped is a known name; its
                // syntax error is the report, and the provider has nothing to
                // be matched against until it's fixed.
                if broken.get(name).is_some_and(|b| b.contains(cap_name)) {
                    continue;
                }
                errors.push_for(provider_file,
                    CompileError::new(
                        "bynk.provider.unknown_capability",
                        provider.capability.span,
                        format!(
                            "provider targets unknown capability `{}` — declare the capability in the same context",
                            cap_name
                        ),
                    ),
                );
                continue;
            };
            // #926 (Decision E): a capability op with its own type parameter(s)
            // cannot be implemented by a Bynk-bodied provider — the body would
            // need `T` rigid through the handler-body checker for a body that
            // can only ever return `None` or echo a `T`-typed parameter.
            // External providers (checked above) are exempt: TypeScript
            // natively supports a generic interface method, so a hand-authored
            // binding class implements it directly.
            for cap_op in &cap.ops {
                if !cap_op.type_params.is_empty() {
                    errors.push_for(
                        provider_file,
                        CompileError::new(
                            "bynk.provider.generic_op_requires_external",
                            provider.span,
                            format!(
                                "provider `{}` for capability `{}` has a Bynk body, but operation `{}` declares its own type parameter(s) (`[{}]`) — a generic capability operation requires an external (bodiless) provider",
                                provider.provider_name.name,
                                cap_name,
                                cap_op.name.name,
                                cap_op
                                    .type_params
                                    .iter()
                                    .map(|p| p.name.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", "),
                            ),
                        )
                        .with_note(
                            "write `provides Cap = Name` with no `{ … }` block, and supply the implementation as a hand-authored class in the adapter's binding file",
                        ),
                    );
                }
            }
            // 1) Every capability op has a provider op.
            for cap_op in &cap.ops {
                if !provider.ops.iter().any(|o| o.name.name == cap_op.name.name) {
                    errors.push_for(
                        provider_file,
                        CompileError::new(
                            "bynk.provider.missing_operation",
                            provider.span,
                            format!(
                                "provider `{}` for capability `{}` is missing operation `{}`",
                                provider.provider_name.name, cap_name, cap_op.name.name
                            ),
                        ),
                    );
                }
            }
            // 2) Every provider op corresponds to a capability op with the
            //    same signature (param types and return type).
            for prov_op in &provider.ops {
                let Some(cap_op) = cap.ops.iter().find(|o| o.name.name == prov_op.name.name) else {
                    errors.push_for(provider_file, CompileError::new(
                        "bynk.provider.extra_operation",
                        prov_op.span,
                        format!(
                            "provider operation `{}.{}` does not match any operation in capability `{}`",
                            provider.provider_name.name, prov_op.name.name, cap_name
                        ),
                    ));
                    continue;
                };
                if cap_op.params.len() != prov_op.params.len() {
                    errors.push_for(provider_file, CompileError::new(
                        "bynk.provider.signature_mismatch",
                        prov_op.span,
                        format!(
                            "provider operation `{}.{}` has {} parameter(s), but capability operation expects {}",
                            provider.provider_name.name,
                            prov_op.name.name,
                            prov_op.params.len(),
                            cap_op.params.len()
                        ),
                    ));
                    continue;
                }
                // Resolved-`Ty` equality, not surface-syntax comparison: two
                // signatures that spell a type differently (an alias, or a
                // generic application written out) but resolve to the same
                // `Ty` must not be flagged as a mismatch, and — the bug this
                // replaces — a `TypeRef` shape `type_refs_match` didn't cover
                // (List/Map/Query/Stream/Connection/…) must not be silently
                // treated as *matching* just because it fell through to
                // `_ => false` on both sides of an `!`. A Bynk-bodied
                // provider op has no type params of its own (checked above),
                // so its params/return type resolve with no vars in scope.
                let cap_info = build_capability_op_info(cap_op, &table.types, tys);
                let no_vars = HashSet::new();
                let prov_params: Vec<TyId> = prov_op
                    .params
                    .iter()
                    .map(|p| {
                        checker::resolve_type_ref_in(&p.type_ref, &table.types, &no_vars, tys)
                            .unwrap_or(tys.intern(Ty::Unit))
                    })
                    .collect();
                let prov_return_ty =
                    checker::resolve_type_ref_in(&prov_op.return_type, &table.types, &no_vars, tys)
                        .unwrap_or(tys.intern(Ty::Unit));
                for (i, (cap_ty, (prov_p, prov_ty))) in cap_info
                    .params
                    .iter()
                    .zip(prov_op.params.iter().zip(prov_params.iter()))
                    .enumerate()
                {
                    if cap_ty != prov_ty {
                        errors.push_for(provider_file, CompileError::new(
                            "bynk.provider.signature_mismatch",
                            prov_p.span,
                            format!(
                                "provider operation `{}.{}` parameter {} has type `{}`, but capability declares `{}`",
                                provider.provider_name.name,
                                prov_op.name.name,
                                i + 1,
                                ts_type_ref_display(&prov_p.type_ref),
                                ts_type_ref_display(&cap_op.params[i].type_ref)
                            ),
                        ));
                    }
                }
                if cap_info.return_ty != prov_return_ty {
                    errors.push_for(provider_file, CompileError::new(
                        "bynk.provider.signature_mismatch",
                        prov_op.return_type.span(),
                        format!(
                            "provider operation `{}.{}` returns `{}`, but capability declares `{}`",
                            provider.provider_name.name,
                            prov_op.name.name,
                            ts_type_ref_display(&prov_op.return_type),
                            ts_type_ref_display(&cap_op.return_type)
                        ),
                    ));
                }
            }
        }
    }
}

/// v0.19: the lock violation a deployment unit's native-platform set implies
/// under the selected `--platform`, if any. Pure — unit-tested below with
/// synthetic sets (the conflict arm is not yet reachable end-to-end while
/// only one platform ships native capabilities).
///
/// P5.3 (`design/tracks/semantics-in-the-checker.md` §6): relocated
/// verbatim from `bynk-emit/src/project/validate.rs`, alongside
/// [`phase_platform_lock`].
fn lock_violation(
    native: &BTreeMap<Platform, String>,
    selected: Platform,
) -> Option<LockViolation> {
    let mut platforms = native.iter();
    let (first, first_unit) = platforms.next()?;
    if let Some((second, second_unit)) = platforms.next() {
        return Some(LockViolation::Conflict {
            a: (*first, first_unit.clone()),
            b: (*second, second_unit.clone()),
        });
    }
    if *first != selected {
        return Some(LockViolation::Required {
            needed: *first,
            unit: first_unit.clone(),
        });
    }
    None
}

/// A platform-lock violation (v0.19, `bynk.target.*`).
#[derive(Debug, PartialEq, Eq)]
enum LockViolation {
    /// The deployment unit needs `needed` but another platform is selected.
    Required { needed: Platform, unit: String },
    /// The deployment unit's closure spans two mutually-exclusive platforms.
    Conflict {
        a: (Platform, String),
        b: (Platform, String),
    },
}

/// v0.15's cross-context capability resolution, relocated alongside
/// [`phase_platform_lock`] (P5.3): resolve a `given`/handler capability
/// prefix (`ctx.Cap`) against a context's own `consumes`/alias tables. Pure —
/// no codegen, no `bynk-emit` dependency of its own — so unlike
/// `collect_given_closure` this one **is** shared rather than duplicated:
/// `bynk-emit/src/project.rs`'s own copy of this function (and of
/// [`handler_cross_caps`]) was deleted in review (#1133) and every one of its
/// call sites repointed here — `bynk-emit` already depends on `bynk-check`,
/// so there was no dependency direction to route around, and keeping two
/// copies only bought two things that could drift out of sync for no reason.
pub fn resolve_consume_prefix(
    prefix: &str,
    consumed: &[String],
    aliases: &HashMap<String, String>,
) -> Option<String> {
    if let Some(q) = aliases.get(prefix) {
        return Some(q.clone());
    }
    if consumed.iter().any(|c| c == prefix) {
        return Some(prefix.to_string());
    }
    None
}

/// v0.15: the cross-context capabilities a context's **handlers** reference,
/// as `deps_key → consumed_context`. Shared with `bynk-emit`, not duplicated
/// — see [`resolve_consume_prefix`]'s doc.
pub fn handler_cross_caps(
    table: &UnitTable,
    consumed: &[String],
    aliases: &HashMap<String, String>,
    flattened: &HashMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut scan = |given: &[CapRef]| {
        for c in given {
            // Events track, slice 0 (spine #936): `Events.emit` is
            // intercepted entirely at the call site (release-at-commit
            // buffering) and never calls through a constructed provider —
            // there is no `EventsProvider` for compose to build, so the
            // first-party `Events` must never become a compose deps entry.
            if c.key() == "Events" && flattened.get(c.key()).map(String::as_str) == Some("bynk") {
                continue;
            }
            if let Some(p) = c.prefix() {
                if let Some(ctx) = resolve_consume_prefix(&p, consumed, aliases) {
                    out.entry(c.key().to_string()).or_insert(ctx);
                }
            } else if let Some(unit) = flattened.get(c.key()) {
                // v0.17: a bare flattened capability is provided by the unit it
                // was flattened from.
                out.entry(c.key().to_string())
                    .or_insert_with(|| unit.clone());
            }
        }
    };
    for s in table.services.values() {
        for h in &s.handlers {
            scan(&h.given);
        }
    }
    for a in table.agents.values() {
        for h in &a.handlers {
            scan(&h.given);
        }
    }
    out
}

/// The `(unit, capability)` pairs a provider capability's `given` closure
/// transitively reaches, itself included, recorded into `reached`. P5.3: a pure resolution walk over the
/// same graph `bynk-emit`'s `instantiate_provider_ts_expr` walks to build a
/// TypeScript instantiation expression — this one builds no TypeScript at
/// all, since `bynk-check` must never depend on `bynk-emit`'s codegen
/// (`bynk-emit` depends on `bynk-check`, never the reverse). The two walks
/// must keep resolving `given` targets identically (prefix → alias/consumes,
/// bare → flattened) or `phase_platform_lock`'s native-platform accounting
/// could drift from what a real build's compose actually instantiates; a
/// reviewer changing one should check the other.
fn collect_given_closure(
    provider_ctx: &str,
    cap: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    reached: &mut BTreeSet<(String, String)>,
) {
    // #2355 follow-up: records `(unit, capability)` pairs rather than units,
    // so the cross-context agent check can find the provider itself; a pair
    // already seen is not walked again.
    if !reached.insert((provider_ctx.to_string(), cap.to_string())) {
        return;
    }
    let Some(provider) = unit_tables
        .get(provider_ctx)
        .and_then(|t| t.providers.get(cap))
    else {
        return;
    };
    if provider.given.is_empty() {
        return;
    }
    let consumed = unit_consumes.get(provider_ctx).cloned().unwrap_or_default();
    let aliases = unit_consumes_aliases
        .get(provider_ctx)
        .cloned()
        .unwrap_or_default();
    let flattened = unit_flattened
        .get(provider_ctx)
        .cloned()
        .unwrap_or_default();
    for g in &provider.given {
        let target_ctx = match g.prefix() {
            Some(p) => resolve_consume_prefix(&p, &consumed, &aliases)
                .unwrap_or_else(|| provider_ctx.to_string()),
            None => flattened
                .get(g.key())
                .cloned()
                .unwrap_or_else(|| provider_ctx.to_string()),
        };
        collect_given_closure(
            &target_ctx,
            g.key(),
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            reached,
        );
    }
}

/// v0.19 (decision 0017): the native platforms a context's **in-process
/// closure** commits it to: every unit whose provider its compose would
/// instantiate — local providers' `given` recursion plus the capabilities its
/// handlers reference — mapped through [`firstparty::platform_of`]. Each
/// platform carries an exemplar unit for the diagnostic message. Service
/// `consumes` edges (RPC under `workers`) do not contribute — only the
/// provider-instantiation walk, which is in-process by construction.
///
/// P5.3: relocated alongside [`phase_platform_lock`], reimplemented on
/// [`collect_given_closure`] rather than moved verbatim — see that
/// function's doc.
fn native_platforms_of_context(
    ctx: &str,
    table: &UnitTable,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
) -> BTreeMap<Platform, String> {
    let mut reached: BTreeSet<(String, String)> = BTreeSet::new();
    for cap in table.providers.keys() {
        collect_given_closure(
            ctx,
            cap,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            &mut reached,
        );
    }
    let consumed = unit_consumes.get(ctx).cloned().unwrap_or_default();
    let aliases = unit_consumes_aliases.get(ctx).cloned().unwrap_or_default();
    let flattened = unit_flattened.get(ctx).cloned().unwrap_or_default();
    for (key, cctx) in handler_cross_caps(table, &consumed, &aliases, &flattened) {
        collect_given_closure(
            &cctx,
            &key,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            &mut reached,
        );
    }
    let mut referenced: BTreeSet<String> = reached.into_iter().map(|(unit, _)| unit).collect();
    // S4a (#1966): a derived capability reaches its base's platform unit, so
    // a context whose only Cloudflare use is a named instance is still locked.
    referenced.extend(table.derived.values().map(|d| d.base_unit.clone()));
    let mut out = BTreeMap::new();
    for unit in referenced {
        if let Some(p) = firstparty::platform_of(&unit) {
            out.entry(p).or_insert(unit);
        }
    }
    out
}

/// v0.19 (decisions 0017/0024): enforce the platform lock per deployment
/// unit — each context under `--target workers`, the whole program under
/// `bundle` (co-location shares the lock).
///
/// P5.3 (`design/tracks/semantics-in-the-checker.md` §6): relocated from
/// `bynk-emit/src/project/validate.rs`'s `check_platform_lock` — category 5
/// of `analysis.rs`'s own seven-category residual-gap accounting ("gap in
/// name only": `analyse_project` hardcodes `Platform::default()`
/// (Cloudflare) and `BuildTarget::Bundle`, and `bynk.cloudflare` is the only
/// platform-native unit that exists, so `lock_violation` can never fire on
/// that path regardless of where this function lives — see `analysis.rs`'s
/// own doc for why R3.5 still requires the move).
#[allow(clippy::too_many_arguments)]
pub fn phase_platform_lock(
    target: BuildTarget,
    selected: Platform,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    errors: &mut ErrorSink,
) {
    // In-browser track, slice 2: `browser` is a Bundle-only platform — a browser
    // cannot do the Workers wire-call model (Service Bindings, Durable Objects,
    // cross-context wire calls). Reject the combination up front, before the
    // per-unit native-platform lock below, which is moot for an invalid build.
    if selected == Platform::Browser && target == BuildTarget::Workers {
        errors.push_for(
            None,
            CompileError::new(
                "bynk.target.browser_bundle_only",
                Span::default(),
                "`--platform browser` builds only the in-process `Bundle` topology, but `--target workers` was selected; a browser cannot run the Workers wire-call model",
            )
            .with_note("build the browser target with `--target bundle` (the default)"),
        );
        return;
    }
    // v0.104 (real-time track slice 3b): the `from websocket` Workers mapping (the
    // Durable Object hibernatable upgrade) is now emitted, so the 3a platform-lock
    // that gated it off is removed.
    // Per-context native sets, with the context name kept for spans/messages.
    let mut per_context: Vec<(String, BTreeMap<Platform, String>)> = Vec::new();
    let mut names: Vec<&String> = groups.keys().collect();
    names.sort();
    for name in names {
        if kinds.get(name.as_str()) != Some(&UnitKind::Context) {
            continue;
        }
        let Some(table) = unit_tables.get(name.as_str()) else {
            continue;
        };
        let native = native_platforms_of_context(
            name,
            table,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
        );
        if !native.is_empty() {
            per_context.push((name.clone(), native));
        }
    }
    // The deployment units to check: per-context under workers; their union
    // under bundle (the whole program co-locates).
    let units: Vec<(String, BTreeMap<Platform, String>)> = match target {
        BuildTarget::Workers => per_context,
        BuildTarget::Bundle => {
            let mut union = BTreeMap::new();
            let mut owner: Option<String> = None;
            for (ctx, native) in per_context {
                owner.get_or_insert(ctx);
                for (p, unit) in native {
                    union.entry(p).or_insert(unit);
                }
            }
            match owner {
                Some(ctx) if !union.is_empty() => vec![(ctx, union)],
                _ => Vec::new(),
            }
        }
    };
    for (ctx, native) in units {
        let Some(violation) = lock_violation(&native, selected) else {
            continue;
        };
        let span_for = |unit: &str| {
            groups
                .get(&ctx)
                .and_then(|idx| consumes_span_of(parsed, idx, unit))
                .map(|(_, s)| s)
                .unwrap_or_default()
        };
        match violation {
            LockViolation::Required { needed, unit } => {
                errors.push_for(
                    None,
                    CompileError::new(
                        "bynk.target.vendor_required",
                        span_for(&unit),
                        format!(
                            "context `{ctx}` uses the platform-native capabilities of `{unit}`, which run only on the `{}` platform, but the build selects `--platform {}`",
                            needed.as_str(),
                            selected.as_str(),
                        ),
                    )
                    .with_note(
                        "build with the matching `--platform`, or remove the platform-native dependency to stay portable",
                    ),
                );
            }
            LockViolation::Conflict { a, b } => {
                errors.push_for(
                    None,
                    CompileError::new(
                        "bynk.target.vendor_conflict",
                        span_for(&a.1),
                        format!(
                            "one deployment unit (via context `{ctx}`) uses platform-native capabilities from two mutually-exclusive platforms: `{}` (from `{}`) and `{}` (from `{}`)",
                            a.0.as_str(),
                            a.1,
                            b.0.as_str(),
                            b.1,
                        ),
                    )
                    .with_note(
                        "split the consumers into separate deployment units (`--target workers`), or remove one of the platform-native dependencies",
                    ),
                );
            }
        }
    }
}

/// v0.173 (ADR 0196 D1), P5.5 (`design/tracks/semantics-in-the-checker.md`
/// §6, §9): warn where a `bynk.Secrets` read names its secret with a computed
/// expression. Non-failing — the program is correct, `bynk deploy` simply
/// cannot see the name — walked per **file** rather than per unit, since a
/// merged `UnitTable` has thrown away which file a call site lives in and
/// [`ErrorSink::extend_for`] attributes a diagnostic to a path.
///
/// Gated on the Workers target because the whole consequence is about `bynk
/// deploy`'s plan, which no other target produces; warning a bundle project
/// about a deploy plan it will never produce would be noise. Relocated from
/// `bynk-emit::project::run_checks` — that call site's own comment claimed
/// this "reaches the editor" via `bynk check`/the LSP, which was true only
/// while the LSP still called `run_checks`'s `Mode::Analyse` arm; P4.2
/// repointed `bynk-ide` at [`crate::analysis::analyse_project`] instead, and
/// `bynk-check` cannot depend on `bynk-emit` to reach this code — so the
/// claim went stale silently, exactly the "ninth gap" §9 of the design doc
/// flagged as a risk rather than a scoped relocation. Wired into
/// `analyse_project` at the same relative point `run_checks` calls it,
/// mirroring [`phase_platform_lock`]'s own treatment of a build-target-gated
/// check: `analyse_project` hardcodes `BuildTarget::Bundle`, so this closes
/// the category structurally (R3.5 — the diagnostic now originates in
/// `bynk-check`), not observably, the same as categories 1 and 5.
pub fn phase_secrets_computed_name(
    target: BuildTarget,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    errors: &mut ErrorSink,
) {
    if target != BuildTarget::Workers {
        return;
    }
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context) {
            continue;
        }
        let Some(flattened) = unit_flattened.get(name) else {
            continue;
        };
        for &i in indices {
            let SourceUnit::Context(ctx) = &parsed[i].unit() else {
                continue;
            };
            // #1976: agent handlers and provider bodies are this Worker's
            // reads too, so a computed name in one is warned like any other.
            let items = &ctx.items;
            let handlers = items.iter().filter_map(|item| match item {
                CommonsItem::Service(s) => Some(s.handlers.iter()),
                CommonsItem::Agent(a) => Some(a.handlers.iter()),
                _ => None,
            });
            let providers = items.iter().filter_map(|item| match item {
                CommonsItem::Provider(p) => Some(p),
                _ => None,
            });
            let (_, mut warnings) = crate::secrets::secret_reads_of(handlers.flatten(), flattened);
            warnings.extend(crate::secrets::provider_secret_reads_of(providers, flattened).1);
            let rel = parsed[i].identity_path();
            errors.extend_for(Some(&rel), warnings);
        }
    }
}

/// #1822: on the Workers target, a capability a context declares and requires
/// (`given`, on a service or agent handler or a provider) needs a provider in
/// that context. A Worker's composition root builds every capability its
/// handlers are given, so one with no provider left `compose` passing `{}`
/// where the capability was required (TS2345): the bundle built and the
/// Workers build didn't. On the bundle target the host may supply it through
/// `__makeSurface(deps)`, so the check is Workers-only, like
/// [`phase_secrets_computed_name`]. A capability from another unit (flattened
/// or qualified) is that unit's to provide, and `exports capability` already
/// requires its provider (`bynk.exports.capability_not_provided`).
pub fn phase_workers_unprovided_capabilities(
    target: BuildTarget,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
) {
    if target != BuildTarget::Workers {
        return;
    }
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context) {
            continue;
        }
        let Some(table) = unit_tables.get(name) else {
            continue;
        };
        // #1882 review: once per capability, at its first `given`, however
        // many handlers require it.
        let mut reported: HashSet<String> = HashSet::new();
        for &i in indices {
            let SourceUnit::Context(ctx) = &parsed[i].unit() else {
                continue;
            };
            let mut givens: Vec<&CapRef> = Vec::new();
            for item in &ctx.items {
                match item {
                    CommonsItem::Service(s) => {
                        givens.extend(s.handlers.iter().flat_map(|h| &h.given));
                    }
                    CommonsItem::Agent(a) => {
                        givens.extend(a.handlers.iter().flat_map(|h| &h.given));
                    }
                    CommonsItem::Provider(p) => givens.extend(&p.given),
                    _ => {}
                }
            }
            for cap in givens {
                // An exported capability with no provider is already
                // `bynk.exports.capability_not_provided` (#1882 review: no
                // second error for the same gap).
                if cap.context.is_some()
                    || !table.capabilities.contains_key(&cap.name.name)
                    || table.providers.contains_key(&cap.name.name)
                    // S4a (#1966): a derived capability's provision is implied.
                    || table.derived.contains_key(&cap.name.name)
                    || table.exported_capabilities.contains(&cap.name.name)
                    || !reported.insert(cap.name.name.clone())
                {
                    continue;
                }
                errors.push_for(
                    Some(&parsed[i].identity_path()),
                    CompileError::new(
                        "bynk.capability.not_provided",
                        cap.span,
                        format!(
                            "capability `{}` is required here but has no provider in context `{name}`; a Worker builds every capability its handlers are given",
                            cap.name.name
                        ),
                    )
                    .with_note(format!(
                        "add `provides {} = …` to `{name}`, or build for `--target bundle`, where the host supplies it",
                        cap.name.name
                    )),
                );
            }
        }
    }
}

/// #2306: on the Workers target, a `from websocket` service's `on open` is hosted
/// inside the Durable Object of the agent it transfers the connection to, and
/// that method is emitted from the file the *service* and agent share. A context
/// split across several files is emitted one file at a time, so a transfer into
/// an agent declared in a different file than the service was routed to the
/// agent's Durable Object (`compose.ts` forwards the upgrade) but never hosted
/// there: the upgrade returned `404`. Reject it at compile time instead. On the
/// bundle target the open is an ordinary surface method and the layout works, so
/// the check is Workers-only, like [`phase_workers_unprovided_capabilities`].
pub fn phase_workers_ws_open_split(
    target: BuildTarget,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
) {
    if target != BuildTarget::Workers {
        return;
    }
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context) || indices.len() < 2 {
            continue;
        }
        let Some(table) = unit_tables.get(name) else {
            continue;
        };
        let local_agents: HashSet<String> = table.agents.keys().cloned().collect();
        for &i in indices {
            let SourceUnit::Context(ctx) = &parsed[i].unit() else {
                continue;
            };
            for item in &ctx.items {
                let CommonsItem::Service(service) = item else {
                    continue;
                };
                if !matches!(service.protocol, ServiceProtocol::WebSocket { .. }) {
                    continue;
                }
                for open in service
                    .handlers
                    .iter()
                    .filter(|h| matches!(h.kind, HandlerKind::Open))
                {
                    let crate::websocket::WsOpenShape::One(target_agent) =
                        crate::websocket::analyse_open_shape(&open.body, &local_agents)
                    else {
                        continue; // a missing or ambiguous transfer has its own diagnostic
                    };
                    let declared_here = ctx.items.iter().any(|it| {
                        matches!(it, CommonsItem::Agent(a) if a.name.name == target_agent.agent)
                    });
                    if declared_here {
                        continue;
                    }
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.ws.open_transfer_cross_file",
                            open.span,
                            format!(
                                "this `on open` transfers its `connection` to agent `{}`, which is declared in a different file of context `{name}` than the service `{}`",
                                target_agent.agent, service.name.name
                            ),
                        )
                        .with_note(
                            "on Workers the open is hosted inside the agent's Durable Object, which is emitted from the file that declares the agent; move the service into the agent's file, or build for `--target bundle`",
                        ),
                    );
                }
            }
        }
    }
}

/// #2355: whether a bodied provider's ops construct one of its context's own
/// agents (`is_agent`). Such a provider takes a `deps` even with no `given`:
/// its agent calls hand `this.deps` to the agent's methods, and on Workers the
/// agent factory reads the Worker's `env` from it. Syntactic, so the provider
/// class, every composer that builds it, and
/// [`phase_workers_provider_agent_cross_context`] agree on the answer; a local
/// shadowing an agent's name only costs an unused `deps`. It matches the
/// construction shape `Agent(key)`, the one the emitter's `agent_construct`
/// lowers; a new way to reach an agent must be added here too.
pub fn provider_constructs_agent(p: &ProviderDecl, is_agent: impl Fn(&str) -> bool) -> bool {
    fn walk(e: &Expr, is_agent: &dyn Fn(&str) -> bool) -> bool {
        if let ExprKind::Call { name, .. } = &e.kind
            && is_agent(&name.name)
        {
            return true;
        }
        expr_children(e).into_iter().any(|c| walk(c, is_agent))
    }
    p.ops.iter().any(|op| {
        let mut top = Vec::new();
        for s in &op.body.statements {
            statement_exprs(s, &mut top);
        }
        top.push(&op.body.tail);
        top.into_iter().any(|e| walk(e, &is_agent))
    })
}

/// #2355 follow-up: on the Workers target, a context may not require a
/// capability whose provider, in another context, constructs that context's
/// agents. The consumer's composition root builds the provider in-process and
/// hands it the consumer's own `env`, which binds only the consumer's Durable
/// Object namespaces: `tsc` rejects the build, and strip-only `bynk dev`
/// quietly falls back to agent state held in the isolate. The capability can
/// be reached through any `given` the consumer's compose instantiates — a
/// service or agent handler's, or a local provider's, transitively — so the
/// walk is `collect_given_closure`'s, reported once per provider at the
/// first `given` that reaches it. On the bundle target the whole program
/// shares one process and the agent factory takes only the key, so the check
/// is Workers-only, like [`phase_workers_unprovided_capabilities`].
#[allow(clippy::too_many_arguments)]
pub fn phase_workers_provider_agent_cross_context(
    target: BuildTarget,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    errors: &mut ErrorSink,
) {
    if target != BuildTarget::Workers {
        return;
    }
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context) {
            continue;
        }
        let consumed = unit_consumes.get(name).cloned().unwrap_or_default();
        let aliases = unit_consumes_aliases.get(name).cloned().unwrap_or_default();
        let flattened = unit_flattened.get(name).cloned().unwrap_or_default();
        let mut reported: HashSet<(String, String)> = HashSet::new();
        for &i in indices {
            let SourceUnit::Context(ctx) = &parsed[i].unit() else {
                continue;
            };
            let mut givens: Vec<&CapRef> = Vec::new();
            for item in &ctx.items {
                match item {
                    CommonsItem::Service(s) => {
                        givens.extend(s.handlers.iter().flat_map(|h| &h.given));
                    }
                    CommonsItem::Agent(a) => {
                        givens.extend(a.handlers.iter().flat_map(|h| &h.given));
                    }
                    CommonsItem::Provider(p) => givens.extend(&p.given),
                    _ => {}
                }
            }
            for cap in givens {
                // The same resolution as `collect_given_closure`'s.
                let target_ctx = match cap.prefix() {
                    Some(p) => resolve_consume_prefix(&p, &consumed, &aliases)
                        .unwrap_or_else(|| name.clone()),
                    None => flattened
                        .get(cap.key())
                        .cloned()
                        .unwrap_or_else(|| name.clone()),
                };
                let mut reached: BTreeSet<(String, String)> = BTreeSet::new();
                collect_given_closure(
                    &target_ctx,
                    cap.key(),
                    unit_tables,
                    unit_consumes,
                    unit_consumes_aliases,
                    unit_flattened,
                    &mut reached,
                );
                for (unit, key) in reached {
                    if unit == *name {
                        continue;
                    }
                    let Some(table) = unit_tables.get(&unit) else {
                        continue;
                    };
                    let Some(provider) = table.providers.get(&key) else {
                        continue;
                    };
                    if provider.external
                        || !provider_constructs_agent(provider, |n| table.agents.contains_key(n))
                        || !reported.insert((unit.clone(), key.clone()))
                    {
                        continue;
                    }
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.capability.provider_agent_cross_context",
                            cap.span,
                            format!(
                                "capability `{key}` is provided by `{}` in context `{unit}`, which constructs `{unit}`'s agents; on Workers context `{name}` would build that provider with its own `env`, which has no binding for those agents' Durable Objects",
                                provider.provider_name.name
                            ),
                        )
                        .with_note(format!(
                            "call a `{unit}` service that uses `{key}` instead, or build for `--target bundle`, where every context shares one process"
                        )),
                    );
                }
            }
        }
    }
}

/// Phase 7: build each production unit's file-declaration index (which file in
/// the unit declares which name), for cross-file lookups in the back half.
pub fn phase_file_index(
    groups: &BTreeMap<String, Vec<usize>>,
    parsed: &[ParsedFile],
) -> HashMap<String, FileDeclIndex> {
    let mut unit_file_index: HashMap<String, FileDeclIndex> = HashMap::new();
    for (name, indices) in groups {
        unit_file_index.insert(name.clone(), build_file_decl_index(indices, parsed));
    }
    unit_file_index
}

/// v0.29.4: the per-unit facets that the producer phases build as nine parallel
/// `HashMap<String, _>`s, all keyed on unit name. Assembling one record per unit
/// makes the "all these maps share one keyset" invariant structural: a single
/// lookup yields every facet as a field, so the per-column `.unwrap()`s on the
/// shared keyset disappear. Fields are total — `exports`/`aliases`/`flattened`
/// default to an empty map for a unit with no entry, reproducing the old
/// `.unwrap_or(empty)` read semantics without the dance.
pub struct UnitInfo {
    pub kind: UnitKind,
    pub table: UnitTable,
    pub uses: Vec<String>,
    pub consumes: Vec<String>,
    pub flattened: HashMap<String, String>,
    pub aliases: HashMap<String, String>,
    pub exports: HashMap<String, Visibility>,
    pub file_index: FileDeclIndex,
    pub files: Vec<usize>,
}

/// v0.29.4: fold the nine parallel per-unit maps into one `HashMap<String,
/// UnitInfo>`. Assembly is driven by the `groups` keyset (the authority), so
/// every group yields exactly one record. Facets that are genuinely optional in
/// the producer maps (`exports`/`aliases`/`flattened`, and `file_index` for a
/// unit with no declarations) default to empty — reproducing the old
/// `.unwrap_or(empty)` read semantics as a total field.
#[allow(clippy::too_many_arguments)]
pub fn assemble_unit_info(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    exports_visibility: &HashMap<String, HashMap<String, Visibility>>,
    unit_file_index: &HashMap<String, FileDeclIndex>,
) -> BTreeMap<String, UnitInfo> {
    groups
        .iter()
        .map(|(name, indices)| {
            let info = UnitInfo {
                kind: *kinds.get(name).unwrap(),
                table: unit_tables.get(name).unwrap().clone(),
                uses: unit_uses.get(name).cloned().unwrap_or_default(),
                consumes: unit_consumes.get(name).cloned().unwrap_or_default(),
                flattened: unit_flattened.get(name).cloned().unwrap_or_default(),
                aliases: unit_consumes_aliases.get(name).cloned().unwrap_or_default(),
                exports: exports_visibility.get(name).cloned().unwrap_or_default(),
                file_index: unit_file_index
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| FileDeclIndex {
                        types: HashMap::new(),
                        fns: HashMap::new(),
                        methods: HashMap::new(),
                        agents: HashMap::new(),
                        capabilities: HashMap::new(),
                    }),
                files: indices.clone(),
            };
            (name.clone(), info)
        })
        .collect()
}

/// Phase 8c: collect every method authored anywhere in one unit, keyed by its
/// attached type's name — so a type's methods surface in the file that declares
/// the type even when the method lives in a sibling file. The collection loop
/// has no `continue`s, so it lifts out whole.
pub fn collect_unit_methods(
    indices: &[usize],
    parsed: &[ParsedFile],
) -> HashMap<String, Vec<FnDecl>> {
    let mut local_methods_for_type: HashMap<String, Vec<FnDecl>> = HashMap::new();
    for &j in indices {
        for item in parsed[j].items() {
            if let CommonsItem::Fn(f) = item
                && let FnName::Method { type_name, .. } = &f.name
            {
                local_methods_for_type
                    .entry(type_name.name.clone())
                    .or_default()
                    .push(f.clone());
            }
        }
    }
    local_methods_for_type
}

/// Phase 8b: merge one context's `consumes` exports into the composed symbol
/// space, recording visibility metadata in the returned `consumed_types`. The
/// per-export `continue`s (missing decl, name conflict) stay internal to the
/// loop, which lifts out whole; name conflicts are pushed into `errors` and the
/// caller's `group_error_baseline` guard reacts to them after this returns.
#[allow(clippy::too_many_arguments)]
pub fn merge_consumed_exports(
    name: &str,
    parsed: &[ParsedFile],
    unit_info: &BTreeMap<String, UnitInfo>,
    combined_types: &mut HashMap<String, Arc<TypeDecl>>,
    combined_methods: &mut HashMap<String, ResolverMethodTable>,
    imported_from: &mut HashMap<String, String>,
    imported_from_kind: &mut HashMap<String, UnitKind>,
    errors: &mut ErrorSink,
) -> HashMap<String, ConsumedType> {
    // Names visible from `consumes` (read-only types from consumed contexts).
    // For each name we track:
    // - the type decl, with the consumed context's identity
    // - the visibility (opaque/transparent)
    // - the owning context's qualified name (for external-construction errors)
    let mut consumed_types: HashMap<String, ConsumedType> = HashMap::new();

    // Now process `consumes` for contexts: add exported types into the
    // symbol table with visibility metadata so the checker can enforce
    // construction / inspection rules.
    for t in unit_info.get(name).into_iter().flat_map(|i| &i.consumes) {
        let used = &unit_info.get(t).expect("consumed unit present").table;
        let used_exports = &unit_info[t].exports;
        for (type_name, vis) in used_exports {
            let Some(decl) = used.types.get(type_name) else {
                continue;
            };
            // #1927: a local value of the export's name (a function, asset,
            // service, agent or provider) takes the emitted module's binding
            // as a type of that name would, so it conflicts the same way.
            let local_value = crate::symbols::local_value_kind(&unit_info[name].table, type_name)
                .map(|(kind, _)| kind)
                .filter(|kind| !matches!(*kind, "type" | "event"));
            if combined_types.contains_key(type_name) || local_value.is_some() {
                // Name conflict between local/uses and consumed export.
                let consumes_site = consumes_span_of(parsed, &unit_info[name].files, t);
                let consumes_span = consumes_site.map(|(_, s)| s).unwrap_or_default();
                let consumes_file = consumes_site.map(|(i, _)| parsed[i].identity_path());
                let in_scope = match local_value {
                    Some(kind) => format!("this context declares a {kind} of the same name"),
                    None => "a type of the same name is already in scope".to_string(),
                };
                errors.push_for(consumes_file.as_deref(),
                    CompileError::new(
                        "bynk.consumes.name_conflict",
                        consumes_span,
                        format!(
                            "context `{name}` consumes `{t}` which exports type `{type_name}`, but {in_scope}",
                        ),
                    )
                    .with_note(
                        "rename one of the conflicting declarations or restructure the import",
                    ),
                );
                continue;
            }
            combined_types.insert(type_name.clone(), decl.clone());
            imported_from.insert(type_name.clone(), t.clone());
            imported_from_kind.insert(type_name.clone(), UnitKind::Context);
            consumed_types.insert(
                type_name.clone(),
                ConsumedType {
                    owning_context: t.clone(),
                    visibility: *vis,
                },
            );
            // Methods on transparently-exported types: they're emitted in
            // the owning context's output, but reading-side methods (like
            // user-declared instance methods) are callable from consumers.
            // For v0.4, we expose all instance methods on consumed types
            // so the checker can resolve method calls; the checker
            // separately enforces that constructors (.of/unsafe) aren't
            // callable externally.
            if let Some(mt) = used.methods.get(type_name) {
                let entry = combined_methods.entry(type_name.clone()).or_default();
                for (m, decl) in &mt.instance {
                    entry
                        .instance
                        .entry(m.clone())
                        .or_insert_with(|| decl.clone());
                }
                // We deliberately *don't* import static methods from
                // consumed contexts. Static methods can construct new
                // values, which is forbidden externally.
            }
        }
    }

    consumed_types
}

/// #1807: close a unit's composed type table over the types its *imported*
/// declarations reach.
///
/// `uses` is one level for naming (ADR 0278): `t.web uses t.model` may not
/// write `t.core`'s `Repo`. But `t.model`'s `type Run = { repo: Repo }` still
/// carries `Repo` into `t.web`, and the checker resolves that field against
/// `t.web`'s table. With `Repo` absent the field's type resolved to nothing,
/// and every position it typed went unchecked: `Run { repo: 42 }` compiled,
/// and so did `r.repo` read as an `Int`.
///
/// Each type an imported declaration names is resolved in that declaration's
/// own scope: its unit's local types, then its unit's `uses`, the same
/// precedence [`compose_unit_symbols`] gives a unit's own names. The walk
/// covers type bodies, fn signatures and method signatures, and repeats for
/// each type it adds. A type it reaches that this unit's table lacks is added
/// with its owning commons as provenance and returned as *hidden*: resolvable
/// here, never nameable. The naming gate (`crate::resolver::check_hidden_type_names`)
/// rejects a hidden name written in this unit's source.
///
/// Two cases stop short:
/// - A reached type that a context owns is not added. It can only arrive
///   through a consumed context's export, so whether it may cross is an
///   `exports` question.
/// - A reached name that this unit already binds to a *different* unit's
///   declaration (its own `type Repo`, say) is `bynk.uses.name_conflict`,
///   when the reached type's commons is not one this unit `uses` directly.
///   The imported field would otherwise be typed by a declaration the unit
///   never chose to shadow. Shadowing a directly used type keeps its existing
///   meaning. Provenance is compared by owning unit.
///
/// #1824: that exemption leaves the same hole open for a directly used
/// commons. Such a shadow is recorded rather than reported (see
/// [`ShadowedReaches`]), and `crate::shadow_uses` rejects it where the unit's
/// own code meets it.
///
/// Returns each hidden name with its owning commons, which emission needs to
/// import it, and the shadows the imported declarations reach.
#[allow(clippy::too_many_arguments)]
pub fn close_reachable_types(
    name: &str,
    parsed: &[ParsedFile],
    unit_info: &BTreeMap<String, UnitInfo>,
    combined_types: &mut HashMap<String, Arc<TypeDecl>>,
    combined_fns: &HashMap<String, Arc<FnDecl>>,
    combined_methods: &mut HashMap<String, ResolverMethodTable>,
    imported_from: &mut HashMap<String, String>,
    imported_from_kind: &mut HashMap<String, UnitKind>,
    errors: &mut ErrorSink,
) -> ReachedTypes {
    let mut conflicted: HashSet<String> = HashSet::new();
    let mut shadowed = ShadowedReaches::default();
    let consumed_signatures = consumed_service_seeds(name, unit_info);
    let hidden = close_imported(
        unit_info,
        combined_types,
        combined_fns,
        combined_methods,
        imported_from,
        imported_from_kind,
        consumed_signatures,
        |r, owner, bound| {
            // #1932: a name a consumed signature reaches but this unit already
            // binds is the unit's own type receiving the value structurally
            // (§6.5), not a conflict.
            if r.signature {
                return;
            }
            let bound = bound.unwrap_or(name);
            // Shadowing a type of a commons this unit `uses` directly is the
            // existing, visible choice (`1697_derived_names_collide` declares
            // its own `Message` beside `uses bynk.locale.types`), so only a
            // shadow of a type the unit cannot name is a conflict.
            let directly_used = unit_info[name].uses.iter().any(|u| u == owner);
            // #1824: the directly used case is a conflict only where this
            // unit's code meets the imported position, which `shadow_uses`
            // decides after checking.
            if bound != owner && directly_used {
                shadowed.0.insert(
                    (r.scope.clone(), r.name.clone()),
                    ShadowedType {
                        name: r.name.clone(),
                        owner: owner.to_string(),
                        bound: bound.to_string(),
                    },
                );
            }
            if bound != owner && !directly_used && conflicted.insert(r.name.clone()) {
                let site = uses_span_of(parsed, &unit_info[name].files, &r.via)
                    .or_else(|| consumes_span_of(parsed, &unit_info[name].files, &r.via));
                let span = site.map(|(_, s)| s).unwrap_or_default();
                let file = site.map(|(i, _)| parsed[i].identity_path());
                errors.push_for(
                    file.as_deref(),
                    CompileError::new(
                        "bynk.uses.name_conflict",
                        span,
                        format!(
                            "`{via}` brings in declarations that use type `{t}` from `{owner}`, \
                             but `{t}` in `{name}` names `{bound}`'s type",
                            via = r.via,
                            t = r.name,
                        ),
                    )
                    .with_note(
                        "rename one of the two types; the imported declarations cannot be \
                         checked against a different type of the same name",
                    ),
                );
            }
        },
    );
    ReachedTypes { hidden, shadowed }
}

/// #1814: close a test suite's composed view over the types its imported
/// declarations reach, as [`close_reachable_types`] closes a unit's table.
///
/// A suite's view is not a unit's: a unit suite's privileged view and a
/// `system` case's harness table each merge their own `imported` units (the
/// target's `uses` and consumed contexts; the suite's `uses` and the
/// participants) one level deep. A declaration of `imported` that won its
/// name in the view (the same `Arc`, first in merge order) seeds the walk, in
/// its declaring unit's scope. What the walk adds is returned as hidden, for
/// the naming gate and for the test scaffold to import from its owning
/// commons.
///
/// A reached name the view already binds to a different declaration is left
/// bound as it is and returned as a [`ViewConflict`]; the caller decides
/// which it reports (review of #1814). A `system` harness reports every one:
/// its view pairs the suite's own `uses` with the participants, which no
/// unit's closure ever sees together. A unit suite reports only those its
/// target's closure cannot: a binding to a consumed context's unexported
/// type, which only the privileged view merges.
pub fn close_view_types(
    imported: &[String],
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    types: &mut HashMap<String, Arc<TypeDecl>>,
    fns: &HashMap<String, Arc<FnDecl>>,
    methods: &mut HashMap<String, ResolverMethodTable>,
) -> ViewReach {
    let mut imported_from: HashMap<String, String> = HashMap::new();
    for u in imported {
        let Some(t) = unit_tables.get(u) else {
            continue;
        };
        for (n, d) in &t.types {
            if types.get(n).is_some_and(|v| Arc::ptr_eq(v, d)) {
                imported_from.entry(n.clone()).or_insert_with(|| u.clone());
            }
        }
        for (n, d) in &t.fns {
            if fns.get(n).is_some_and(|v| Arc::ptr_eq(v, d)) {
                imported_from.entry(n.clone()).or_insert_with(|| u.clone());
            }
        }
    }
    let mut conflicts: Vec<ViewConflict> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let hidden = close_imported(
        &TableScopes {
            tables: unit_tables,
            uses: unit_uses,
        },
        types,
        fns,
        methods,
        &mut imported_from,
        &mut HashMap::new(),
        Vec::new(),
        |r, owner, bound| {
            if bound != Some(owner) && seen.insert(r.name.clone()) {
                conflicts.push(ViewConflict {
                    name: r.name.clone(),
                    owner: owner.to_string(),
                    bound: bound.map(str::to_string),
                    via: r.via.clone(),
                });
            }
        },
    );
    ViewReach { hidden, conflicts }
}

/// #1814: what [`close_view_types`] found.
#[derive(Debug, Clone, Default)]
pub struct ViewReach {
    /// Each type the closure added, with its owning commons.
    pub hidden: BTreeMap<String, String>,
    /// Each reached name the view already bound to another declaration, in
    /// the walk's order, once per name.
    pub conflicts: Vec<ViewConflict>,
}

/// #1814 review: a type an imported declaration of a suite view names, which
/// the view binds to a different declaration of the same name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewConflict {
    /// The name both declarations share.
    pub name: String,
    /// The commons whose declaration the imported one means.
    pub owner: String,
    /// The unit whose declaration the view binds the name to; `None` for the
    /// view's own (a unit suite's target's local type).
    pub bound: Option<String>,
    /// The imported unit the reaching declaration came from.
    pub via: String,
}

/// #1814: what the reachable-type walk reads of a unit: its declared table,
/// its `uses`, and its kind (`None` for a table built without one, which is
/// then a scope but never a commons, so nothing it declares is added).
/// A unit's own closure reads the assembled [`UnitInfo`]s; a suite view's
/// reads the per-unit maps the suite phases carry.
trait ScopeTables {
    fn scope(&self, unit: &str) -> Option<(&UnitTable, &[String], Option<UnitKind>)>;
}

impl ScopeTables for BTreeMap<String, UnitInfo> {
    fn scope(&self, unit: &str) -> Option<(&UnitTable, &[String], Option<UnitKind>)> {
        self.get(unit)
            .map(|i| (&i.table, i.uses.as_slice(), Some(i.kind)))
    }
}

/// #1814: [`ScopeTables`] over the suite phases' per-unit maps.
struct TableScopes<'a> {
    tables: &'a HashMap<String, UnitTable>,
    uses: &'a HashMap<String, Vec<String>>,
}

impl ScopeTables for TableScopes<'_> {
    fn scope(&self, unit: &str) -> Option<(&UnitTable, &[String], Option<UnitKind>)> {
        let table = self.tables.get(unit)?;
        let uses = self.uses.get(unit).map(Vec::as_slice).unwrap_or_default();
        Some((table, uses, table.kind))
    }
}

/// #1807: a type name an imported declaration references: resolved in
/// `scope` (the declaring unit), and reached through `via` (the `uses` or
/// `consumes` target the declaration was imported from, which a conflict
/// points at).
struct Reached {
    name: String,
    scope: String,
    via: String,
    /// #1932: reached from a consumed service's signature (or from a type such
    /// a reach added). The value crosses structurally (§6.5), so a name the
    /// consumer already binds is its own type receiving it, not a conflict.
    signature: bool,
}

fn push_refs(names: Vec<String>, scope: &str, via: &str, queue: &mut Vec<Reached>) {
    push_refs_with(names, scope, via, false, queue);
}

fn push_refs_with(
    names: Vec<String>,
    scope: &str,
    via: &str,
    signature: bool,
    queue: &mut Vec<Reached>,
) {
    queue.extend(names.into_iter().map(|n| Reached {
        name: n,
        scope: scope.to_string(),
        via: via.to_string(),
        signature,
    }));
}

/// #1932: the type names the `on call` handlers of every context `name`
/// consumes reference, resolved in that context's scope and reached through
/// its `consumes`, in a stable order. A consumed service's signature crosses
/// the boundary (§6.5), so a commons type in it is reached like one in an
/// imported declaration: the consumer resolves it, and emission imports its
/// codecs from the declaring commons, whether or not the consumer `uses` it.
fn consumed_service_seeds(name: &str, unit_info: &BTreeMap<String, UnitInfo>) -> Vec<Reached> {
    let mut out = Vec::new();
    let Some(info) = unit_info.get(name) else {
        return out;
    };
    for t in &info.consumes {
        let Some(consumed) = unit_info.get(t) else {
            continue;
        };
        let mut services: Vec<&ServiceDecl> = consumed.table.services.values().collect();
        services.sort_by(|a, b| a.name.name.cmp(&b.name.name));
        for s in services {
            // The one `on call` handler a cross-context call reaches
            // (`resolver::cross_context_service_for`).
            if let Some(h) = s.handlers.iter().find(|h| h.kind == HandlerKind::Call) {
                let mut names = Vec::new();
                for p in &h.params {
                    type_ref_names(&p.type_ref, &HashSet::new(), &mut names);
                }
                type_ref_names(&h.return_type, &HashSet::new(), &mut names);
                push_refs_with(names, t, t, true, &mut out);
            }
        }
    }
    out
}

/// #1807, #1814: the walk [`close_reachable_types`] and [`close_view_types`]
/// share. Seeds from every entry `imported_from` attributes to another unit,
/// adds each reached commons type the table lacks (with its methods), and
/// calls `on_bound(reached, owner, bound)` for a reached commons type whose
/// name the table already binds, `bound` being that binding's provenance
/// (`None` for the table's own declaration).
#[allow(clippy::too_many_arguments)]
fn close_imported<S: ScopeTables + ?Sized>(
    scopes: &S,
    combined_types: &mut HashMap<String, Arc<TypeDecl>>,
    combined_fns: &HashMap<String, Arc<FnDecl>>,
    combined_methods: &mut HashMap<String, ResolverMethodTable>,
    imported_from: &mut HashMap<String, String>,
    imported_from_kind: &mut HashMap<String, UnitKind>,
    // #1932: further seeds, each resolved in its own scope (a consumed
    // service's signature).
    extra_seeds: Vec<Reached>,
    mut on_bound: impl FnMut(&Reached, &str, Option<&str>),
) -> BTreeMap<String, String> {
    let mut queue: Vec<Reached> = Vec::new();
    let mut type_names: Vec<&String> = combined_types.keys().collect();
    type_names.sort();
    for n in type_names {
        if let Some(unit) = imported_from.get(n) {
            push_refs(
                type_decl_ref_names(&combined_types[n]),
                unit,
                unit,
                &mut queue,
            );
        }
    }
    let mut fn_names: Vec<&String> = combined_fns.keys().collect();
    fn_names.sort();
    for n in fn_names {
        if let Some(unit) = imported_from.get(n) {
            push_refs(
                fn_sig_ref_names(&combined_fns[n], &[]),
                unit,
                unit,
                &mut queue,
            );
        }
    }
    let mut method_types: Vec<&String> = combined_methods.keys().collect();
    method_types.sort();
    for t in method_types {
        if let Some(unit) = imported_from.get(t) {
            let mt = &combined_methods[t];
            let attached = combined_types.get(t).map(|d| d.type_params.as_slice());
            let mut decls: Vec<&Arc<FnDecl>> =
                mt.instance.values().chain(mt.statics.values()).collect();
            decls.sort_by_key(|d| d.span.start);
            for d in decls {
                push_refs(
                    fn_sig_ref_names(d, attached.unwrap_or_default()),
                    unit,
                    unit,
                    &mut queue,
                );
            }
        }
    }
    queue.extend(extra_seeds);
    // The walk pops from the back; reverse so names are visited in the
    // sorted seeding order and diagnostics come out deterministically.
    queue.reverse();

    let mut hidden: BTreeMap<String, String> = BTreeMap::new();
    let mut visited: HashSet<(String, String)> = HashSet::new();
    while let Some(r) = queue.pop() {
        if !visited.insert((r.scope.clone(), r.name.clone())) {
            continue;
        }
        let Some(owner) = type_owner_in_scope(&r.scope, &r.name, scopes) else {
            continue;
        };
        // Review of #1813: before the conflict check, so a context-owned
        // reach is skipped whether or not this unit binds the name (a
        // consumer's own `Money` beside a consumed export's field of the
        // exporting context's `Money` is not a conflict).
        let Some((owner_table, _, owner_kind)) = scopes.scope(owner) else {
            continue;
        };
        if owner_kind != Some(UnitKind::Commons) {
            continue;
        }
        if combined_types.contains_key(&r.name) {
            on_bound(&r, owner, imported_from.get(&r.name).map(String::as_str));
            continue;
        }
        let decl = owner_table.types[&r.name].clone();
        push_refs_with(
            type_decl_ref_names(&decl),
            owner,
            &r.via,
            r.signature,
            &mut queue,
        );
        combined_types.insert(r.name.clone(), decl.clone());
        imported_from.insert(r.name.clone(), owner.to_string());
        imported_from_kind.insert(r.name.clone(), UnitKind::Commons);
        hidden.insert(r.name.clone(), owner.to_string());
        // The reached type's methods come along, as a direct `uses` brings
        // them: `r.repo.isValid()` resolves in the declaring scope too.
        if let Some(mt) = owner_table.methods.get(&r.name) {
            let entry = combined_methods.entry(r.name.clone()).or_default();
            let mut decls: Vec<(&String, &Arc<FnDecl>, bool)> = mt
                .instance
                .iter()
                .map(|(m, d)| (m, d, true))
                .chain(mt.statics.iter().map(|(m, d)| (m, d, false)))
                .collect();
            decls.sort_by_key(|(_, d, _)| d.span.start);
            for (m, d, is_instance) in decls {
                push_refs_with(
                    fn_sig_ref_names(d, &decl.type_params),
                    owner,
                    &r.via,
                    r.signature,
                    &mut queue,
                );
                let table = if is_instance {
                    &mut entry.instance
                } else {
                    &mut entry.statics
                };
                table.entry(m.clone()).or_insert_with(|| d.clone());
            }
        }
    }
    hidden
}

/// What [`close_reachable_types`] finds beyond the unit's composed table.
#[derive(Debug, Clone, Default)]
pub struct ReachedTypes {
    /// Each hidden type name with its owning commons, which emission needs to
    /// import it.
    pub hidden: BTreeMap<String, String>,
    /// #1824: the reached names this unit's own declaration shadows.
    pub shadowed: ShadowedReaches,
}

/// #1824: a type an imported declaration names, which this unit resolves to a
/// different declaration of the same name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ShadowedType {
    /// The name both declarations share.
    pub name: String,
    /// The commons whose declaration the imported one means.
    pub owner: String,
    /// The unit whose declaration the name resolves to here (the unit itself,
    /// for its own `type`).
    pub bound: String,
}

/// #1824: the reached type names a unit's own declaration shadows, keyed by
/// the declaring unit whose scope resolves the name and the name.
///
/// Type names are bare strings to the checker, so an imported declaration's
/// `Repo` is checked as whatever `Repo` the importing unit binds. When that is
/// the unit's own `type Repo`, beside a `uses` of the commons the imported
/// declaration means, the checker accepts what the emitted TypeScript rejects.
/// `close_reachable_types` records each such reach here; `crate::shadow_uses`
/// reports the positions the unit's code fills or reads.
#[derive(Debug, Clone, Default)]
pub struct ShadowedReaches(HashMap<(String, String), ShadowedType>);

impl ShadowedReaches {
    /// No imported declaration reaches a shadowed type.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The shadowed types `r` names, resolved in `scope` (the unit that
    /// declares the position), its type variables `vars` excluded. Sorted, so
    /// two positions compare equal when they name the same shadowed types.
    pub fn in_type_ref(&self, scope: &str, r: &TypeRef, vars: &[TypeParam]) -> Vec<&ShadowedType> {
        let vars: HashSet<&str> = vars.iter().map(|p| p.name.name.as_str()).collect();
        let mut names = Vec::new();
        type_ref_names(r, &vars, &mut names);
        let mut out: Vec<&ShadowedType> = names
            .iter()
            .filter_map(|n| self.0.get(&(scope.to_string(), n.clone())))
            .collect();
        out.sort();
        out.dedup();
        out
    }
}

/// #1807: the unit that owns type `name` as seen from `scope`: `scope` itself
/// if it declares `name`, else the first of `scope`'s `uses` that does. This
/// is [`compose_unit_symbols`]'s precedence, applied to another unit.
fn type_owner_in_scope<'a, S: ScopeTables + ?Sized>(
    scope: &'a str,
    name: &str,
    scopes: &'a S,
) -> Option<&'a str> {
    let (table, uses, _) = scopes.scope(scope)?;
    if table.types.contains_key(name) {
        return Some(scope);
    }
    uses.iter()
        .find(|u| {
            scopes
                .scope(u)
                .is_some_and(|(t, _, _)| t.types.contains_key(name))
        })
        .map(String::as_str)
}

/// #1807: the type names a type declaration's body references, its own type
/// parameters excluded.
pub(crate) fn type_decl_ref_names(d: &TypeDecl) -> Vec<String> {
    let vars: HashSet<&str> = d.type_params.iter().map(|p| p.name.name.as_str()).collect();
    let mut out = Vec::new();
    match &d.body {
        TypeBody::Record(r) => {
            for f in &r.fields {
                type_ref_names(&f.type_ref, &vars, &mut out);
            }
        }
        TypeBody::Sum(s) => {
            for v in &s.variants {
                for p in &v.payload {
                    type_ref_names(&p.type_ref, &vars, &mut out);
                }
            }
        }
        TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
    }
    out
}

/// #1807: the type names a fn's signature references, its own type
/// parameters excluded, and for a method its type's (`attached`):
/// `fn Box.get(self) -> A` on `type Box[A]` declares no `[A]` of its own.
fn fn_sig_ref_names(f: &FnDecl, attached: &[TypeParam]) -> Vec<String> {
    let vars: HashSet<&str> = f
        .type_params
        .iter()
        .chain(attached)
        .map(|p| p.name.name.as_str())
        .collect();
    let mut out = Vec::new();
    for p in &f.params {
        type_ref_names(&p.type_ref, &vars, &mut out);
    }
    type_ref_names(&f.return_type, &vars, &mut out);
    out
}

/// #1807: every user type name in `r`, outside `vars`. Exhaustive over
/// `TypeRef`, so a new compound constructor is a build failure here rather
/// than a silently skipped name.
pub(crate) fn type_ref_names(r: &TypeRef, vars: &HashSet<&str>, out: &mut Vec<String>) {
    match r {
        TypeRef::Named(id) => {
            if !vars.contains(id.name.as_str()) {
                out.push(id.name.clone());
            }
        }
        TypeRef::App { name, args, .. } => {
            if !vars.contains(name.name.as_str()) {
                out.push(name.name.clone());
            }
            for a in args {
                type_ref_names(a, vars, out);
            }
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            type_ref_names(a, vars, out);
            type_ref_names(b, vars, out);
        }
        TypeRef::Option(t, _)
        | TypeRef::Effect(t, _)
        | TypeRef::HttpResult(t, _)
        | TypeRef::List(t, _)
        | TypeRef::Query(t, _)
        | TypeRef::Stream(t, _)
        | TypeRef::Connection(t, _)
        | TypeRef::History(t, _) => type_ref_names(t, vars, out),
        TypeRef::Fn(params, ret, _) => {
            for p in params {
                type_ref_names(p, vars, out);
            }
            type_ref_names(ret, vars, out);
        }
        TypeRef::Base(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
        | TypeRef::Unit(_) => {}
    }
}

/// Phase 8a: compose one unit's symbol space — its local table plus a
/// one-level `uses` mixin (commons identity preserved). Returns the combined
/// type/fn/method tables and the `imported_from` provenance maps; the mixin
/// loop has no `continue`s, so it lifts out whole.
#[allow(clippy::type_complexity)]
pub fn compose_unit_symbols(
    name: &str,
    local_table: &UnitTable,
    unit_info: &BTreeMap<String, UnitInfo>,
) -> (
    HashMap<String, Arc<TypeDecl>>,
    HashMap<String, Arc<FnDecl>>,
    HashMap<String, ResolverMethodTable>,
    HashMap<String, String>,
    HashMap<String, UnitKind>,
) {
    // Compose: local + transitive (one level) uses. For commons, mixin
    // preserves type identity; for contexts, mixin produces per-context
    // nominal types. The resolver doesn't distinguish (the rebranding is
    // observable in emission); the symbol table union is the same.
    let mut combined_types = local_table.types.clone();
    let mut combined_fns = local_table.fns.clone();
    let mut combined_methods = local_table.methods.clone();
    let mut imported_from: HashMap<String, String> = HashMap::new();
    let mut imported_from_kind: HashMap<String, UnitKind> = HashMap::new();

    for t in unit_info.get(name).into_iter().flat_map(|i| &i.uses) {
        let used = &unit_info.get(t).expect("used unit present").table;
        for (type_name, decl) in &used.types {
            if !combined_types.contains_key(type_name) {
                combined_types.insert(type_name.clone(), decl.clone());
                imported_from.insert(type_name.clone(), t.clone());
                imported_from_kind.insert(type_name.clone(), UnitKind::Commons);
            }
        }
        for (fn_name, decl) in &used.fns {
            // #1925: a local declaration of any kind shadows the import, as
            // it does in the emitted module ([`shadowed_imports`]).
            if !combined_fns.contains_key(fn_name)
                && crate::symbols::local_value_kind(local_table, fn_name).is_none()
            {
                combined_fns.insert(fn_name.clone(), decl.clone());
                imported_from.insert(fn_name.clone(), t.clone());
                imported_from_kind.insert(fn_name.clone(), UnitKind::Commons);
            }
        }
        // #1883, slice 1b: a used commons' assets are visible here
        // (`assets::compose_unit_assets`); record where each came from, so
        // the emitter imports its constant from the declaring module. A
        // local declaration of the same name, of any kind, wins (#1925).
        for asset_name in used.assets.keys() {
            if crate::symbols::local_value_kind(local_table, asset_name).is_none()
                && !combined_fns.contains_key(asset_name)
                && !imported_from.contains_key(asset_name)
            {
                imported_from.insert(asset_name.clone(), t.clone());
                imported_from_kind.insert(asset_name.clone(), UnitKind::Commons);
            }
        }
        for (type_name, mt) in &used.methods {
            let entry = combined_methods.entry(type_name.clone()).or_default();
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

    (
        combined_types,
        combined_fns,
        combined_methods,
        imported_from,
        imported_from_kind,
    )
}

/// #1925: the `uses`-imported functions and assets that unit `name`'s own
/// declarations of another kind shadow, by name. A local declaration wins, as
/// in the emitted module, so both compositions leave these out
/// ([`compose_unit_symbols`], [`crate::assets::compose_unit_assets`]); the
/// resolver and checker report a reference the local declaration cannot answer
/// as `bynk.uses.name_conflict` ([`ResolvedCommons::shadowed_import_error`]).
/// A same-kind shadow (a local function over an imported one) is ordinary
/// shadowing and is not listed. The first `uses` target to declare a name
/// decides, as in the compositions.
///
/// #1927: an imported type a local value hides is listed too, but stays in
/// the composed table: an imported declaration that uses it must still check,
/// and a value of it may flow through the unit's code. Writing its name is the
/// conflict instead ([`crate::resolver::check_shadowed_type_names`]). A local
/// type or event over an imported type is #1824's type shadowing, not listed.
///
/// [`ResolvedCommons::shadowed_import_error`]: crate::resolver::ResolvedCommons::shadowed_import_error
pub fn shadowed_imports(
    name: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
) -> HashMap<String, crate::resolver::ShadowedImport> {
    let Some(info) = unit_info.get(name) else {
        return HashMap::new();
    };
    shadowed_imports_in(
        &info.table,
        info.uses
            .iter()
            .filter_map(|t| unit_info.get(t).map(|u| (t, &u.table))),
    )
}

/// [`shadowed_imports`] over a unit's own table and its `uses` targets'
/// tables, in `uses` order — for a caller holding tables rather than
/// [`UnitInfo`]s (a suite's view of its target, `crate::test_suites`).
pub fn shadowed_imports_in<'a>(
    local: &UnitTable,
    uses: impl Iterator<Item = (&'a String, &'a UnitTable)>,
) -> HashMap<String, crate::resolver::ShadowedImport> {
    let mut out = HashMap::new();
    for (t, used) in uses {
        // #1927: an imported type too. A local type or event over it is
        // #1824's type shadowing, not this.
        let imports = used
            .fns
            .keys()
            .map(|n| (n, "function"))
            .chain(used.assets.keys().map(|n| (n, "asset")))
            .chain(used.types.keys().map(|n| (n, "type")));
        for (n, imported) in imports {
            if let Some((kind, local_span)) = crate::symbols::local_value_kind(local, n)
                && kind != imported
                && !(imported == "type" && kind == "event")
            {
                let hidden = crate::resolver::ShadowedImport {
                    unit: t.clone(),
                    imported,
                    local: kind,
                    local_span,
                };
                // The first `uses` target to declare a name decides, except
                // that a type replaces a function or asset (review of #1933).
                // The local value hides both imports, but only the type stays
                // composed, so only its entry gates the name: kept second, it
                // would let `uses` order decide whether the unit may write it.
                match out.entry(n.clone()) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(hidden);
                    }
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        if imported == "type" && e.get().imported != "type" {
                            e.insert(hidden);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Phase 5c: detect `consumes` cycles. #696: record each `consumes`-clause
/// site (file + span) keyed by `(consumer, target)` so a detected cycle
/// anchors on the exact clause that forms the closing edge — a real span in
/// a real file — and renders with source context. Synthetic units are left
/// out so their (snapshot-less) files never claim a diagnostic.
pub fn phase_detect_consumes_cycles(
    groups: &BTreeMap<String, Vec<usize>>,
    parsed: &[ParsedFile],
    unit_consumes: &HashMap<String, Vec<String>>,
    errors: &mut ErrorSink,
) {
    let mut consumes_sites: HashMap<(String, String), (PathBuf, Span)> = HashMap::new();
    for (name, indices) in groups {
        for &i in indices {
            if parsed[i].is_synthetic() {
                continue;
            }
            for c in parsed[i].consumes() {
                consumes_sites
                    .entry((name.clone(), c.target.joined()))
                    .or_insert_with(|| (parsed[i].identity_path(), c.span));
            }
        }
    }
    let mut cycle_errors: Vec<(Option<PathBuf>, CompileError)> = Vec::new();
    detect_consumes_cycles(unit_consumes, &consumes_sites, &mut cycle_errors);
    for (path, err) in cycle_errors {
        errors.push_for(path.as_deref(), err);
    }
}

#[cfg(test)]
mod platform_lock_tests {
    use super::{LockViolation, Platform, lock_violation};
    use std::collections::BTreeMap;

    fn native(entries: &[(Platform, &str)]) -> BTreeMap<Platform, String> {
        entries
            .iter()
            .map(|(p, u)| (*p, (*u).to_string()))
            .collect()
    }

    #[test]
    fn empty_closure_imposes_no_lock() {
        assert_eq!(lock_violation(&native(&[]), Platform::Node), None);
    }

    #[test]
    fn matching_platform_is_fine() {
        let n = native(&[(Platform::Cloudflare, "bynk.cloudflare")]);
        assert_eq!(lock_violation(&n, Platform::Cloudflare), None);
    }

    #[test]
    fn mismatched_platform_is_required() {
        let n = native(&[(Platform::Cloudflare, "bynk.cloudflare")]);
        assert_eq!(
            lock_violation(&n, Platform::Node),
            Some(LockViolation::Required {
                needed: Platform::Cloudflare,
                unit: "bynk.cloudflare".to_string(),
            })
        );
    }

    // The conflict arm is not yet reachable end-to-end (only one platform
    // ships native capabilities until `bynk.aws`); the rule is exercised here
    // with a synthetic two-platform set so it does not ship untested
    // (proposal v0.19, review call).
    #[test]
    fn two_platforms_conflict_regardless_of_selection() {
        let n = native(&[
            (Platform::Cloudflare, "bynk.cloudflare"),
            (Platform::Node, "bynk.synthetic"),
        ]);
        let v = lock_violation(&n, Platform::Cloudflare);
        assert_eq!(
            v,
            Some(LockViolation::Conflict {
                a: (Platform::Cloudflare, "bynk.cloudflare".to_string()),
                b: (Platform::Node, "bynk.synthetic".to_string()),
            })
        );
    }
}

#[cfg(test)]
mod native_platform_closure_tests {
    use super::{HashMap, Platform, UnitTable, native_platforms_of_context};
    use bynk_syntax::ast::{CapRef, Ident, ProviderDecl, QualifiedName};
    use bynk_syntax::span::Span;
    use std::collections::HashMap as StdHashMap;

    fn ident(name: &str) -> Ident {
        Ident {
            name: name.to_string(),
            span: Span::default(),
        }
    }

    fn qualified(parts: &[&str]) -> QualifiedName {
        QualifiedName {
            parts: parts.iter().map(|p| ident(p)).collect(),
            span: Span::default(),
        }
    }

    fn given_cap(prefix: Option<&[&str]>, name: &str) -> CapRef {
        CapRef {
            context: prefix.map(qualified),
            name: ident(name),
            span: Span::default(),
        }
    }

    fn provider(capability: &str, given: Vec<CapRef>) -> ProviderDecl {
        ProviderDecl {
            capability: ident(capability),
            provider_name: ident(&format!("{capability}Impl")),
            given,
            ops: Vec::new(),
            external: false,
            documentation: None,
            span: Span::default(),
            trivia: Default::default(),
        }
    }

    fn empty_table() -> UnitTable {
        UnitTable {
            kind: None,
            removed_agents: Vec::new(),
            types: StdHashMap::new(),
            fns: StdHashMap::new(),
            methods: StdHashMap::new(),
            capabilities: StdHashMap::new(),
            providers: StdHashMap::new(),
            services: StdHashMap::new(),
            agents: StdHashMap::new(),
            actors: StdHashMap::new(),
            exported_capabilities: Default::default(),
            events: StdHashMap::new(),
            flattened_caps: StdHashMap::new(),
            assets: StdHashMap::new(),
            embeds: StdHashMap::new(),
            derived: StdHashMap::new(),
        }
    }

    /// P5.3 review finding (#1133): nothing in the tree exercised
    /// `collect_given_closure`'s recursive arm — every existing fixture that
    /// reaches `bynk.cloudflare` does so through a handler's bare `given Kv`
    /// (`handler_cross_caps`, depth 0: `provider.given.is_empty()` short-
    /// circuits immediately), never through a local provider's own `given`
    /// chain. This pins the contract `collect_given_closure`'s own doc
    /// states: a context whose *only* path to a platform-native unit is a
    /// provider's `given` — `provides Cache = LocalCache given
    /// bynk.cloudflare.Kv { … }`, with no handler ever naming `Kv` directly —
    /// must still be recognised as native. `bynkc/tests/fixtures/negative/
    /// 1030_kv_provider_given_wrong_platform` pins the same contract
    /// end-to-end through `run_checks`.
    #[test]
    fn a_providers_given_chain_into_a_platform_native_unit_is_recognised() {
        let mut table = empty_table();
        table.providers.insert(
            "Cache".to_string(),
            provider(
                "Cache",
                vec![given_cap(Some(&["bynk", "cloudflare"]), "Kv")],
            ),
        );
        let mut unit_tables = HashMap::new();
        unit_tables.insert("app.web".to_string(), table);
        let mut unit_consumes = HashMap::new();
        unit_consumes.insert("app.web".to_string(), vec!["bynk.cloudflare".to_string()]);

        let native = native_platforms_of_context(
            "app.web",
            unit_tables.get("app.web").unwrap(),
            &unit_tables,
            &unit_consumes,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            native.get(&Platform::Cloudflare).map(String::as_str),
            Some("bynk.cloudflare"),
            "a provider's own `given` closure into a platform-native unit must be \
             walked recursively, not just a handler's direct `given` — got {native:?}"
        );
    }

    /// A provider whose `given` closure never leaves ordinary (non-native)
    /// units contributes nothing — the recursive walk must not manufacture a
    /// platform out of thin air.
    #[test]
    fn a_providers_given_chain_into_an_ordinary_unit_is_not_native() {
        let mut table = empty_table();
        table.providers.insert(
            "Cache".to_string(),
            provider("Cache", vec![given_cap(None, "Clock")]),
        );
        let mut unit_tables = HashMap::new();
        unit_tables.insert("app.web".to_string(), table);

        let native = native_platforms_of_context(
            "app.web",
            unit_tables.get("app.web").unwrap(),
            &unit_tables,
            &HashMap::new(),
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            native.is_empty(),
            "a `given` closure that never reaches a platform-native unit must not \
             report one — got {native:?}"
        );
    }
}

#[cfg(test)]
mod view_closure_tests {
    use super::{HashMap, UnitTable, build_unit_table, close_view_types};
    use bynk_project::{ParsedFile, UnitKind};
    use bynk_syntax::{lexer, parser};
    use std::path::PathBuf;

    fn table(name: &str, kind: UnitKind, src: &str) -> UnitTable {
        let tokens = lexer::tokenize(src).expect("lex");
        let unit = parser::parse_unit(&tokens, src).expect("parse");
        let path = PathBuf::from(format!("{}.bynk", name.replace('.', "/")));
        let pf = ParsedFile::new(path.clone(), path, None, src.to_string(), unit, kind, false);
        let mut errors = Vec::new();
        build_unit_table(name, kind, &[0], &[pf], &mut errors)
    }

    /// Review of #1814: a table built without a kind (`UnitTable::default()`,
    /// the shape several harnesses build) is still a scope. Its types and its
    /// `uses` are searched; only its own declarations, never a commons', are
    /// not added. It used to make the scope invisible, so a type reached
    /// through it resolved to nothing, silently.
    #[test]
    fn a_kindless_table_is_still_searched_as_a_scope() {
        let core = table(
            "t.core",
            UnitKind::Commons,
            "commons t.core\n\ntype Repo = String where NonEmpty\n",
        );
        let mut model = table(
            "t.model",
            UnitKind::Commons,
            "commons t.model\n\nuses t.core\n\ntype Run = { repo: Repo }\n",
        );
        model.kind = None;
        let mut types = model.types.clone();
        let tables: HashMap<String, UnitTable> =
            [("t.core".to_string(), core), ("t.model".to_string(), model)].into();
        let uses: HashMap<String, Vec<String>> =
            [("t.model".to_string(), vec!["t.core".to_string()])].into();
        let reach = close_view_types(
            &["t.model".to_string()],
            &tables,
            &uses,
            &mut types,
            &HashMap::new(),
            &mut HashMap::new(),
        );
        assert_eq!(
            reach.hidden.get("Repo").map(String::as_str),
            Some("t.core"),
            "{:?}",
            reach.hidden
        );
        assert!(types.contains_key("Repo"));
    }
}

#[cfg(test)]
mod label_attribution_tests {
    use super::*;

    /// Each label is attributed by the `FileId` its span carries: the error's
    /// own file when it shares the primary span's id, has none, or names the
    /// error's own path; another registered file by its path; and an
    /// unregistered file (a toolchain-injected unit) as unavailable.
    #[test]
    fn labels_are_attributed_to_the_file_their_span_is_in() {
        let (own, sibling, injected) = (FileId(10), FileId(11), FileId(12));
        let mut sink = ErrorSink::new();
        sink.register_file(own, PathBuf::from("src/app/z_more.bynk"));
        sink.register_file(sibling, PathBuf::from("src/app/a_web.bynk"));
        let error = CompileError::new(
            "bynk.resolve.duplicate_service",
            Span::new_in(own, 24, 27),
            "service `api` is already declared",
        )
        .with_label(Span::new_in(own, 0, 1), "same id")
        .with_label(Span::new(0, 1), "no id")
        .with_label(Span::new_in(sibling, 21, 24), "sibling")
        .with_label(Span::new_in(injected, 0, 1), "unregistered");
        sink.push_for(Some(Path::new("src/app/z_more.bynk")), error);
        let all = sink.into_all();
        assert_eq!(
            all[0].label_files,
            vec![
                LabelFile::Primary,
                LabelFile::Primary,
                LabelFile::Other(PathBuf::from("src/app/a_web.bynk")),
                LabelFile::Unavailable,
            ]
        );
    }

    /// #1956: a label into a first-party unit carries that unit's reserved
    /// `FileId`, so it is attributed to the unit — with its own source — never
    /// taken for the error's file.
    #[test]
    fn a_label_in_a_firstparty_unit_is_attributed_to_that_unit() {
        let unit = firstparty::unit("bynk").expect("the `bynk` unit is registered");
        let own = FileId(30);
        let mut sink = ErrorSink::new();
        sink.register_file(own, PathBuf::from("src/app/web.bynk"));
        let error = CompileError::new(
            "bynk.resolve.missing_field",
            Span::new_in(own, 0, 7),
            "missing required field `headers` for record `Request`",
        )
        .with_label(
            Span::new_in(firstparty_file_id(unit), 0, 7),
            "field declared here",
        );
        sink.push_for(Some(Path::new("src/app/web.bynk")), error);
        assert_eq!(
            sink.into_all()[0].label_files,
            vec![LabelFile::FirstParty {
                identity_path: unit.identity_path,
                source: unit.source,
            }]
        );
    }

    /// With no file identity on the primary span, a label is still the
    /// error's own file when its id names the error's own path.
    #[test]
    fn a_label_naming_the_errors_own_path_is_primary() {
        let own = FileId(20);
        let mut sink = ErrorSink::new();
        sink.register_file(own, PathBuf::from("src/a.bynk"));
        let error = CompileError::new("bynk.test", Span::default(), "msg")
            .with_label(Span::new_in(own, 0, 1), "here");
        sink.push_for(Some(Path::new("src/a.bynk")), error);
        assert_eq!(sink.into_all()[0].label_files, vec![LabelFile::Primary]);
    }
}

/// #1925: a unit's own declaration of another kind shadowing a `uses`-imported
/// function or asset, and (#1927) a unit's own value shadowing an imported
/// type. A local declaration wins, as in the emitted module; a reference it
/// cannot answer, or a written hidden type name, is `bynk.uses.name_conflict`,
/// in a function body (the resolver's walk) and a handler body (the
/// checker's) alike.
#[cfg(test)]
mod shadowed_import_tests {
    use crate::testkit::analyse;

    /// Each imported kind: its declaration in `lib.util`, and an expression
    /// using it with the type that expression has.
    const IMPORTED: [(&str, &str, &str, &str); 3] = [
        (
            "function",
            "fn thing() -> Int {\n  1\n}\n",
            "thing()",
            "Int",
        ),
        (
            "asset",
            "asset thing = \"assets/x.json\"\n",
            "thing.url",
            "String",
        ),
        // #1927: writing an imported type's name (here, constructing one).
        (
            "type",
            "type thing = { n: Int }\n",
            "thing { n: 1 }.n",
            "Int",
        ),
    ];

    const LOCAL: [(&str, &str); 7] = [
        ("function", "fn thing() -> Int {\n  2\n}\n"),
        ("asset", "asset thing = \"assets/x.json\"\n"),
        ("type", "type thing = { n: Int }\n"),
        ("event", "event thing = { n: Int }\n"),
        (
            "service",
            "service thing {\n  on call() -> Effect[Int] {\n    Effect.pure(1)\n  }\n}\n",
        ),
        (
            "agent",
            "agent thing {\n  key id: String\n\n  store hits: Cell[Int]\n\n  on call ping() -> Effect[Int] {\n    1\n  }\n}\n",
        ),
        (
            "provider",
            "capability ProbeCap {\n  fn go() -> Effect[Int]\n}\n\nprovides ProbeCap = thing {\n  fn go() -> Effect[Int] {\n    Effect.pure(1)\n  }\n}\n",
        ),
    ];

    #[derive(Clone, Copy, Debug)]
    enum Use {
        None,
        FnBody,
        HandlerBody,
    }

    #[test]
    fn every_local_kind_against_every_import_and_use_site() {
        for (imported, idecl, expr, ty) in IMPORTED {
            for (local, ldecl) in LOCAL {
                for at in [Use::None, Use::FnBody, Use::HandlerBody] {
                    let using = match at {
                        Use::None => String::new(),
                        Use::FnBody => format!("fn useIt() -> {ty} {{\n  {expr}\n}}\n"),
                        Use::HandlerBody => format!(
                            "service user {{\n  on call() -> Effect[{ty}] {{\n    Effect.pure({expr})\n  }}\n}}\n"
                        ),
                    };
                    let lib = format!("commons lib.util\n\n{idecl}");
                    let app = format!("context app\n\nuses lib.util\n\n{ldecl}\n{using}");
                    let r = analyse(&[
                        ("lib/util.bynk", &lib),
                        ("app/main.bynk", &app),
                        ("assets/x.json", "{}"),
                    ]);
                    let cats = r.error_categories();
                    let case = format!("local {local} over imported {imported}, used {at:?}");
                    let want: Vec<&str> = match at {
                        // An unused shadow, and a same-kind one, are legal.
                        Use::None => vec![],
                        _ if local == imported => vec![],
                        // An event is a type: a local one over an imported
                        // type is #1824's type shadowing, and answers.
                        _ if imported == "type" && local == "event" => vec![],
                        // In a function body the resolver reads `thing.url`
                        // on a type name as a static member of the type, and
                        // rejects it under that code.
                        Use::FnBody if imported == "asset" && matches!(local, "type" | "event") => {
                            vec!["bynk.resolve.unknown_static_member"]
                        }
                        // A local agent answers a call in a handler (agent
                        // construction), wrongly here: one key, not none.
                        // Its diagnostic notes the hidden import.
                        Use::HandlerBody if imported == "function" && local == "agent" => {
                            vec!["bynk.agent.construction_arity"]
                        }
                        _ => vec!["bynk.uses.name_conflict"],
                    };
                    assert_eq!(cats, want, "{case}");
                }
            }
        }
    }

    /// A local declaration that can answer the reference does: a call to a
    /// local agent is the agent, not the hidden import (before #1925 the
    /// checker preferred the imported function, against the emitted module).
    #[test]
    fn a_local_agent_answers_its_own_call() {
        let r = analyse(&[
            (
                "lib/util.bynk",
                "commons lib.util\n\nfn thing() -> Int {\n  1\n}\n",
            ),
            (
                "app/main.bynk",
                "context app\n\nuses lib.util\n\nagent thing {\n  key id: String\n\n  store hits: Cell[Int]\n\n  on call ping() -> Effect[Int] {\n    1\n  }\n}\n\nservice web {\n  on call() -> Effect[Int] {\n    let n <- thing(\"k\").ping()\n    Effect.pure(n)\n  }\n}\n",
            ),
        ]);
        r.assert_clean();
    }

    /// Review of #1926: two used commons declaring one name are no ambiguity
    /// when the unit hides both behind its own declaration — an unused
    /// shadow stays legal (`phase_uses_name_conflicts` shares the predicate).
    #[test]
    fn a_local_service_hiding_two_used_imports_is_no_ambiguity() {
        let r = analyse(&[
            (
                "lib/a.bynk",
                "commons lib.a\n\nfn thing() -> Int {\n  1\n}\n",
            ),
            (
                "lib/b.bynk",
                "commons lib.b\n\nfn thing() -> Int {\n  2\n}\n",
            ),
            (
                "app/main.bynk",
                "context app\n\nuses lib.a\nuses lib.b\n\nservice thing {\n  on call() -> Effect[Int] {\n    Effect.pure(1)\n  }\n}\n",
            ),
        ]);
        r.assert_clean();
    }

    /// Review of #1926: a bare reference to a local function that hides an
    /// imported asset means the function; only a field read (`thing.url`)
    /// means the hidden asset.
    #[test]
    fn a_bare_local_function_keeps_its_own_advice() {
        let lib = (
            "lib/util.bynk",
            "commons lib.util\n\nasset thing = \"assets/x.json\"\n",
        );
        let file = |body: &str| {
            format!(
                "context app\n\nuses lib.util\n\nfn thing() -> Int {{\n  1\n}}\n\nfn useIt() -> Int {{\n  {body}\n}}\n"
            )
        };
        let bare = file("thing + 1");
        let r = analyse(&[lib, ("app/main.bynk", &bare), ("assets/x.json", "{}")]);
        assert_eq!(r.error_categories(), vec!["bynk.resolve.fn_without_call"]);
        let read = file("let u = thing.url\n  1");
        let r = analyse(&[lib, ("app/main.bynk", &read), ("assets/x.json", "{}")]);
        assert_eq!(r.error_categories(), vec!["bynk.uses.name_conflict"]);
    }

    /// #1927: the sum used across these tests, from a commons, with a
    /// function that makes one and one that reads one.
    const STATUS_LIB: (&str, &str) = (
        "lib/st.bynk",
        "commons lib.st\n\ntype Status =\n  | Done\n  | Pending\n  | Failed(reason: String)\n\nfn mk() -> Status {\n  Done\n}\n\nfn isDone(s: Status) -> Bool {\n  match s {\n    Done => true\n    _ => false\n  }\n}\n",
    );

    /// #1927: a bare variant of a hidden sum emits the sum's name, which the
    /// local declaration has taken, so constructing one is the conflict.
    /// Matching one is not: a pattern emits the variant's tag alone.
    #[test]
    fn a_hidden_sums_bare_variant_constructs_only_by_conflict() {
        let unit = |body: &str| {
            format!("commons app\n\nuses lib.st\n\nfn Status() -> Int {{\n  1\n}}\n\n{body}")
        };
        let construct = unit("fn f() -> Bool {\n  isDone(Done)\n}\n");
        let r = analyse(&[STATUS_LIB, ("app/main.bynk", &construct)]);
        assert_eq!(r.error_categories(), vec!["bynk.uses.name_conflict"]);
        // A payload variant, constructed by a bare call.
        let construct = unit("fn f() -> Bool {\n  isDone(Failed(\"x\"))\n}\n");
        let r = analyse(&[STATUS_LIB, ("app/main.bynk", &construct)]);
        assert_eq!(r.error_categories(), vec!["bynk.uses.name_conflict"]);
        let matched = unit(
            "fn f() -> Bool {\n  match mk() {\n    Done => true\n    Failed(r) => false\n    _ => false\n  }\n}\n",
        );
        let r = analyse(&[STATUS_LIB, ("app/main.bynk", &matched)]);
        r.assert_clean();
    }

    /// #1927: a value of a hidden type flows through the unit's code, and an
    /// imported declaration that uses the type still checks; only writing the
    /// name is the conflict.
    #[test]
    fn a_hidden_types_values_still_flow() {
        let r = analyse(&[
            STATUS_LIB,
            (
                "app/main.bynk",
                "context app\n\nuses lib.st\n\nagent Status {\n  key id: String\n\n  store hits: Cell[Int]\n\n  on call ping() -> Effect[Int] {\n    1\n  }\n}\n\nfn f() -> Bool {\n  let s = mk()\n  isDone(s)\n}\n",
            ),
        ]);
        r.assert_clean();
    }

    /// #1927: a local asset answers its own `.url`, even beside a hidden
    /// imported type of its name.
    #[test]
    fn a_local_asset_answers_its_own_url_beside_a_hidden_type() {
        let r = analyse(&[
            STATUS_LIB,
            (
                "app/main.bynk",
                "commons app\n\nuses lib.st\n\nasset Status = \"assets/x.json\"\n\nfn f() -> String {\n  Status.url\n}\n",
            ),
            ("assets/x.json", "{}"),
        ]);
        r.assert_clean();
    }

    /// Review of #1933: two used commons declaring the hidden name as a type
    /// and as a function. The local value hides both, and the type still
    /// gates the name whichever `uses` comes first.
    #[test]
    fn a_hidden_type_gates_whatever_the_uses_order() {
        let a = ("lib/a.bynk", "commons lib.a\n\ntype Status = { n: Int }\n");
        let b = (
            "lib/b.bynk",
            "commons lib.b\n\nfn Status() -> Int {\n  1\n}\n",
        );
        for (first, second) in [("lib.a", "lib.b"), ("lib.b", "lib.a")] {
            let app = format!(
                "context app\n\nuses {first}\nuses {second}\n\nservice Status {{\n  on call() -> Effect[Int] {{\n    Effect.pure(1)\n  }}\n}}\n\nfn f(s: Status) -> Int {{\n  s.n\n}}\n"
            );
            let r = analyse(&[a, b, ("app/main.bynk", &app)]);
            assert_eq!(
                r.error_categories(),
                vec!["bynk.uses.name_conflict"],
                "uses {first} then {second}"
            );
        }
    }

    /// Review of #1933: a local function hiding an imported type answers a
    /// bare reference as a function value.
    #[test]
    fn a_local_function_hiding_a_type_is_a_function_value() {
        let r = analyse(&[
            (
                "lib/util.bynk",
                "commons lib.util\n\ntype thing = { n: Int }\n",
            ),
            (
                "app/main.bynk",
                "commons app\n\nuses lib.util\n\nfn thing(x: Int) -> Int {\n  x + 1\n}\n\nfn f() -> List[Int] {\n  [1, 2].map(thing)\n}\n",
            ),
        ]);
        r.assert_clean();
    }

    /// #1927: a local value named like a type a context consumes conflicts at
    /// the `consumes` clause, as a local type of that name already does.
    #[test]
    fn a_local_value_named_like_a_consumed_export_conflicts() {
        let r = analyse(&[
            (
                "shop.bynk",
                "context shop\n\nexports transparent { Order }\n\ntype Order = { id: String }\n",
            ),
            (
                "app.bynk",
                "context app\n\nconsumes shop\n\nservice Order {\n  on call() -> Effect[Int] {\n    Effect.pure(1)\n  }\n}\n",
            ),
        ]);
        assert_eq!(r.error_categories(), vec!["bynk.consumes.name_conflict"]);
    }

    /// The diagnostic names both declarations and where the local one is.
    #[test]
    fn the_conflict_names_both_declarations() {
        let r = analyse(&[
            (
                "lib/util.bynk",
                "commons lib.util\n\nfn thing() -> Int {\n  1\n}\n",
            ),
            (
                "app/main.bynk",
                "context app\n\nuses lib.util\n\ntype thing = { n: Int }\n\nfn useIt() -> Int {\n  thing()\n}\n",
            ),
        ]);
        let e = r
            .analysis
            .errors
            .iter()
            .find(|e| e.error.category == "bynk.uses.name_conflict")
            .expect("a conflict");
        assert_eq!(
            e.error.message,
            "`thing` names this unit's type, which hides the function `thing` from `lib.util`"
        );
    }
}

#[cfg(test)]
mod firstparty_injection_tests {
    use super::{
        ErrorSink, ParsedFile, UnitKind, firstparty, firstparty_file_id, inject_firstparty, lexer,
        parser,
    };
    use bynk_syntax::span::FileId;
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    /// The seven references a project can make to a first-party unit, in a
    /// fixed order: `consumes` for the two adapters, `uses` for the commons.
    const REFS: [&str; 7] = [
        "consumes bynk { Clock }",
        "consumes bynk.cloudflare { Kv }",
        "uses bynk.map",
        "uses bynk.list",
        "uses bynk.string",
        "uses bynk.locale",
        "uses bynk.locale.types",
    ];

    /// #1956: every injected first-party unit's spans carry its reserved
    /// `FileId` — distinct per unit and never `FileId::UNKNOWN` — so a label
    /// into one is never taken for the user's file.
    #[test]
    fn injected_units_carry_their_reserved_file_id() {
        let ids: BTreeSet<FileId> = firstparty::UNITS.iter().map(firstparty_file_id).collect();
        assert_eq!(ids.len(), firstparty::UNITS.len());
        assert!(!ids.contains(&FileId::UNKNOWN));
        let mut parsed = project((1 << REFS.len()) - 1);
        let injected = inject_firstparty(&mut parsed, &mut ErrorSink::new());
        assert!(!injected.is_empty());
        for unit in injected {
            let pf = parsed
                .iter()
                .find(|pf| pf.identity_path() == std::path::Path::new(unit.identity_path))
                .expect("an injected unit is in `parsed`");
            assert_eq!(
                pf.unit().span().file,
                firstparty_file_id(unit),
                "{}",
                unit.name
            );
        }
    }

    /// A one-file project whose context makes the references `mask` selects.
    fn project(mask: u32) -> Vec<ParsedFile> {
        let mut uses = String::new();
        let mut consumes = String::new();
        for (bit, line) in REFS.iter().enumerate() {
            if mask & (1 << bit) != 0 {
                let into = if line.starts_with("uses") {
                    &mut uses
                } else {
                    &mut consumes
                };
                into.push_str(line);
                into.push('\n');
            }
        }
        let src = format!("context t\n\n{uses}{consumes}\nfn f() -> Int {{ 1 }}\n");
        let toks = lexer::tokenize(&src).expect("lexes");
        let (unit, _) =
            parser::parse_unit_with_warnings_from(&toks, &src, &mut { 0 }).expect("parses");
        vec![ParsedFile::synthetic(
            PathBuf::from("t.bynk"),
            PathBuf::from("t.bynk"),
            src,
            unit,
            UnitKind::Context,
        )]
    }

    /// The units the hand-written injection (before S1, #1945) chose for the
    /// references in `mask`, written out from its gates.
    fn previous_injection(mask: u32) -> BTreeSet<&'static str> {
        let has = |bit: u32| mask & (1 << bit) != 0;
        let (bynk, cloudflare) = (has(0), has(1));
        let (map, list, string, locale, locale_types) = (has(2), has(3), has(4), has(5), has(6));
        let mut out = BTreeSet::new();
        let mut add = |cond: bool, name: &'static str| {
            if cond {
                out.insert(name);
            }
        };
        // Interop I5 (#1972): `bynk.cloudflare` consumes `bynk` for
        // `Service`'s `Request`/`Response`, so it brings `bynk` (and so its
        // `bynk.locale.types`) with it.
        let bynk = bynk || cloudflare;
        add(bynk, "bynk");
        add(cloudflare, "bynk.cloudflare");
        add(map, "bynk.map");
        add(map || locale || list, "bynk.list");
        add(locale || string, "bynk.string");
        // The `bynk` adapter's own `uses bynk.locale.types`, seen because the
        // adapter was injected before the scan ran.
        add(locale || locale_types || bynk, "bynk.locale.types");
        add(locale, "bynk.locale");
        out
    }

    /// Following the sources' own `uses`/`consumes` injects exactly what the
    /// hand-written gates did, for every combination of references.
    #[test]
    fn the_closure_matches_the_previous_gates_for_every_combination() {
        for mask in 0..(1u32 << REFS.len()) {
            let mut parsed = project(mask);
            let mut errors = ErrorSink::new();
            let injected = inject_firstparty(&mut parsed, &mut errors);
            assert!(
                errors.is_empty(),
                "mask {mask:#09b}: a first-party source failed to parse"
            );
            let names: BTreeSet<&str> = injected.iter().map(|u| u.name).collect();
            assert_eq!(names, previous_injection(mask), "mask {mask:#09b}");
        }
    }

    /// Injected units are pushed after the project's own, in ascending
    /// `id_block` order — the order the hand-written blocks pushed them in —
    /// and the returned entries follow the same order.
    #[test]
    fn injected_units_are_pushed_in_id_block_order() {
        let mut parsed = project((1 << REFS.len()) - 1);
        let mut errors = ErrorSink::new();
        let injected = inject_firstparty(&mut parsed, &mut errors);
        let pushed: Vec<String> = parsed[1..]
            .iter()
            .map(|pf| pf.unit().name().joined())
            .collect();
        assert_eq!(
            pushed,
            [
                "bynk",
                "bynk.cloudflare",
                "bynk.map",
                "bynk.list",
                "bynk.string",
                "bynk.locale.types",
                "bynk.locale",
            ]
        );
        let returned: Vec<&str> = injected.iter().map(|u| u.name).collect();
        assert_eq!(returned, pushed);
        assert!(injected.windows(2).all(|w| w[0].id_block < w[1].id_block));
    }

    /// A project that references no first-party unit injects nothing.
    #[test]
    fn nothing_is_injected_unreferenced() {
        let mut parsed = project(0);
        let injected = inject_firstparty(&mut parsed, &mut ErrorSink::new());
        assert!(injected.is_empty());
        assert_eq!(parsed.len(), 1);
    }

    /// The injection names no first-party unit: adding one to the standard
    /// library is a `bynk-std` registry entry plus its files (R-6). A per-unit
    /// constant reappearing here means a unit is being special-cased again.
    #[test]
    fn injection_is_registry_driven() {
        let src = include_str!("project_model.rs");
        let start = src.find("pub fn phase_parse(").expect("phase_parse");
        let end = src[start..]
            .find("\n/// #1710: for each unit with a file the strict parse rejected")
            .map(|rel| start + rel)
            .expect("the item after inject_firstparty");
        let body = &src[start..end];
        for name in firstparty::UNITS.iter().map(|u| u.name) {
            assert!(!body.contains(&format!("\"{name}\"")), "names {name}");
        }
        let per_unit_constant = body
            .lines()
            .find(|l| l.contains("firstparty::") && (l.contains("_UNIT") || l.contains("_SRC")));
        assert_eq!(per_unit_constant, None, "references a per-unit constant");
    }
}

/// #2223: rewrite canonical dependency unit names in `error`'s text to the
/// form a person reads (`bynk_project::display_names`).
fn display_canonical_names(error: &mut CompileError) {
    if error.message.contains('$') {
        error.message = bynk_project::display_names(&error.message);
    }
    for note in &mut error.notes {
        if note.contains('$') {
            *note = bynk_project::display_names(note);
        }
    }
    for (_, label) in &mut error.labels {
        if label.contains('$') {
            *label = bynk_project::display_names(label);
        }
    }
}
