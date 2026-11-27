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
    CAPABILITY_SIGNATURE, RECORD_FIELD, REQUEST_RECORD_PARAM, SUM_PAYLOAD,
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
    check_group_module_docs, check_path_name_alignment, detect_dependency_cycles,
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
#[derive(Clone)]
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
    /// #3431: every project source, kept only when some file declares a
    /// `SignedCookie` actor, so `signed_cookie_unprefixed` can tell whether its
    /// cookie name is read anywhere else before offering a rename.
    cookie_sources: Vec<(FileId, Arc<str>)>,
    /// The `(address, length)` of each parsed source `cookie_sources` was built
    /// from, so a repeat `register_parsed` over the same files skips the rebuild.
    cookie_key: Vec<(usize, usize)>,
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
            cookie_sources: Vec::new(),
            cookie_key: Vec::new(),
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
        let key: Vec<(usize, usize)> = parsed
            .iter()
            .map(|pf| (pf.source().as_ptr() as usize, pf.source().len()))
            .collect();
        if key != self.cookie_key {
            self.cookie_key = key;
            self.cookie_sources = Self::cookie_sources_of(parsed);
        }
        self.register_files(parsed);
    }
    /// #3431: the project's sources, when some file declares a `SignedCookie`.
    fn cookie_sources_of(parsed: &[ParsedFile]) -> Vec<(FileId, Arc<str>)> {
        if parsed.iter().any(|pf| pf.source().contains("SignedCookie")) {
            parsed
                .iter()
                .filter_map(|pf| {
                    let id = bynk_project::parse_cache::file_id_for(&pf.abs_path()?);
                    Some((id, Arc::from(pf.source())))
                })
                .collect()
        } else {
            Vec::new()
        }
    }
    fn register_files(&mut self, parsed: &[ParsedFile]) {
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
            bynk_syntax::Severity::Warning | bynk_syntax::Severity::Hint => {
                self.warnings.push(attributed)
            }
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
        self.extend_attributed(errs);
    }
    /// [`Self::extend_attributed_by_span`] for a caller that has already called
    /// [`Self::register_parsed`] on the final `parsed` set. Registering hashes
    /// every file's path, so a caller that attributes once per unit (the
    /// per-unit check loops) registers once up front and calls this: doing it
    /// per call made that loop cost units times files (#3243).
    pub fn extend_attributed(&mut self, errs: impl IntoIterator<Item = CompileError>) {
        for e in errs {
            let file = self.files.get(&e.span.file).cloned();
            self.push_for(file.as_deref(), e);
        }
    }
    /// Append everything `other` collected (#3243 S2.3): the per-file scratch
    /// sink a file's check wrote into, folded into the project's in the order
    /// the checks ran, so the result is what one shared sink would hold.
    pub fn absorb(&mut self, other: ErrorSink) {
        self.entries.extend(other.entries);
        self.warnings.extend(other.warnings);
    }
    /// True when no **error-severity** diagnostic has been collected — the
    /// build-failure gate. Warnings do not count (ADR 0117).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    /// Consume the sink, yielding the non-failing **warnings** (ADR 0117).
    pub fn into_warnings(self) -> Vec<AttributedError> {
        let mut warnings = self.warnings;
        attach_cookie_rename_fix(&self.cookie_sources, &mut warnings);
        attribute_labels(&self.files, &mut warnings);
        warnings
    }
    /// Consume the sink, yielding errors then warnings — the full diagnostic
    /// list the LSP and a failed build render together.
    pub fn into_all(self) -> Vec<AttributedError> {
        let mut all = self.entries;
        all.extend(self.warnings);
        attach_cookie_rename_fix(&self.cookie_sources, &mut all);
        attribute_labels(&self.files, &mut all);
        all
    }
    /// The count of **error-severity** diagnostics.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Rewrite each error-severity diagnostic pushed since the sink held `from`
    /// of them (see [`Self::len`]), for a check that wraps another's errors with
    /// where its input came from (#3942).
    pub fn map_errors_from(&mut self, from: usize, f: impl Fn(CompileError) -> CompileError) {
        for entry in self.entries.iter_mut().skip(from) {
            entry.error = f(entry.error.clone());
        }
    }
}

/// #3431: offer `signed_cookie_unprefixed`'s rename to `__Host-…` only when no
/// other site in the project mentions the cookie name. A read site (a typed
/// `cookies` field or `@name("x")`, `RequestCookies.get("x")`, a test) names it
/// as a string literal, so the test is deliberately textual and conservative:
/// any other occurrence of the quoted name anywhere in the project's sources,
/// comments included, keeps the diagnostic note-only. The rename logs existing
/// users out, so it is `MaybeIncorrect`: a quick fix, never fix-all's.
fn attach_cookie_rename_fix(sources: &[(FileId, Arc<str>)], errors: &mut [AttributedError]) {
    for ae in errors {
        let e = &mut ae.error;
        if e.category != "bynk.actor.signed_cookie_unprefixed" || !e.suggestions.is_empty() {
            continue;
        }
        let Some((_, text)) = sources.iter().find(|(id, _)| *id == e.span.file) else {
            continue;
        };
        let Some(literal) = text.get(e.span.range()) else {
            continue;
        };
        let name = match literal.strip_prefix('"').and_then(|l| l.strip_suffix('"')) {
            Some(n) if !n.is_empty() && !n.contains(['\\', '"']) => n,
            _ => continue,
        };
        let mentions: usize = sources
            .iter()
            .map(|(_, t)| t.matches(literal).count())
            .sum();
        if mentions != 1 {
            continue;
        }
        // A `__Secure-` name is swapped, not stacked: `__Host-sid`, not `__Host-__Secure-sid`.
        let base = match name.get(..9) {
            Some(p) if p.eq_ignore_ascii_case("__Secure-") => &name[9..],
            _ => name,
        };
        if base.is_empty() {
            continue;
        }
        e.suggestions.push(bynk_syntax::error::Suggestion {
            message: format!(
                "rename the cookie to `__Host-{base}` (no other site in the project names `{name}`; existing users are logged out when the name changes)"
            ),
            edits: vec![(e.span, format!("\"__Host-{base}\""))],
            applicability: bynk_syntax::error::Applicability::MaybeIncorrect,
        });
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

/// #3941: resolve every `const X: T = env(NAME[, default])` to the literal
/// `bynk.toml` gives it in the `default` environment, so every later phase sees
/// an ordinary constant. The per-environment build selects another environment
/// ([`crate::env_consts::resolve`] takes its name). A manifest whose variables
/// do not read is the manifest check's to report, and leaves the constants to
/// their defaults.
pub fn resolve_env_consts(
    parsed: &mut [ParsedFile],
    project_root: &std::path::Path,
    overlay: &std::collections::HashMap<std::path::PathBuf, String>,
    errors: &mut ErrorSink,
) {
    let default = bynk_project::vars::DEFAULT_ENVIRONMENT;
    resolve_env_consts_for(parsed, project_root, overlay, default, errors);
}

/// [`resolve_env_consts`] for the named environment (`bynk deploy --env`).
pub fn resolve_env_consts_for(
    parsed: &mut [ParsedFile],
    project_root: &std::path::Path,
    overlay: &std::collections::HashMap<std::path::PathBuf, String>,
    environment: &str,
    errors: &mut ErrorSink,
) {
    let vars = bynk_project::vars::try_read_vars_with(project_root, overlay).unwrap_or_default();
    let outcome = crate::env_consts::resolve(parsed, &vars, environment);
    errors.extend_attributed_by_span(parsed, outcome.source);
    for e in outcome.manifest {
        errors.push_for(Some(std::path::Path::new("bynk.toml")), e);
    }
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
/// ([`bynk_syntax::parser::merge_syntax_errors`]). #2807: a lex error recovers too.
fn all_syntax_errors(source: &str, strict: Vec<CompileError>) -> Vec<CompileError> {
    let (tokens, lex_errors) = bynk_syntax::lexer::tokenize_recovering(source);
    let recovered = bynk_syntax::parser::parse_units_recovering(&tokens, source).errors;
    let recovered = bynk_syntax::lexer::without_lex_echoes(&lex_errors, &tokens, recovered);
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

    // #3955: a unit that mentions `compare`/`sortWith`/`Ordering` reaches `bynk.ordering`.
    crate::implicit_uses::add_implicit_uses(&mut parsed);
    // #4181: a commons that names a transparent type of the `bynk` adapter reaches it.
    crate::implicit_uses::add_implicit_consumes(&mut parsed);
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
    layout: &bynk_project::worker_layout::WorkerLayout,
    unit_tables: &mut HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
) {
    // #3536: recorded here, the phase that already holds the groups, kinds,
    // parsed files and mutable tables of both pipelines.
    let selections = consume_selections(groups, kinds, parsed);
    crate::symbols::record_unselected_consumed_types(unit_tables, &selections);
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
                    let mut error = CompileError::new(
                        "bynk.capability.base_not_instantiable",
                        d.base.span,
                        format!(
                            "`{owner}.{}` has no named instances: a derived capability's base must be a platform resource kind",
                            d.base.name
                        ),
                    );
                    // #3300: a one-per-Worker capability is used bare.
                    if crate::firstparty::singleton(&owner, &d.base.name).is_some() {
                        error = error.with_note(format!(
                            "a Worker has one `{0}` binding, so `{0}` is used bare: write `given {0}` where it is needed, not a capability over it",
                            d.base.name
                        ));
                    }
                    errors.push_for(Some(&path), error);
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
                        } else if let Some(value) = {
                            // #3942: an error about a value a constant gave says so.
                            let mark = errors.len();
                            let value = instance_option_value(
                                ann,
                                option_kind,
                                &d.base.name,
                                &path,
                                errors,
                            );
                            if let Some(arg) = ann.args.first() {
                                errors.map_errors_from(mark, |e| {
                                    crate::annotation_consts::with_const_origin(e, arg)
                                });
                            }
                            value
                        } {
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
                                            && layout.script_name(unit) == *value
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
                                            layout.script_name(context)
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
                        let allowed = if d.base.name == "Analytics" {
                            // P3b (#2582): the dataset's name is derived.
                            "`Analytics` instances take no options: `bynk deploy` names the dataset for the context and the capability".to_string()
                        } else if kind.options.is_empty() {
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
                // L7 (#2414): Workers Cache is one Worker-wide setting, so a
                // context declares at most one `EdgeCache` instance.
                if crate::symbols::is_edge_cache_base(&owner, &d.base.name)
                    && let Some((earlier, _)) =
                        table.derived.iter().find(|(_, inst)| inst.is_edge_cache())
                {
                    errors.push_for(
                        Some(&path),
                        CompileError::new(
                            "bynk.cloudflare.edge_cache_duplicate",
                            c.span,
                            format!(
                                "context `{name}` already declares `EdgeCache` instance `{earlier}`"
                            ),
                        )
                        .with_note(format!(
                            "the edge cache is one setting for the whole Worker, so one instance turns it on and purges it; use `{earlier}`"
                        )),
                    );
                    continue;
                }
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
    phase_edge_cache_reach(groups, parsed, unit_tables, errors);
    crate::containers::phase_container_instance_names(groups, parsed, unit_tables, errors);
}

/// L7 (#2414, Decision D): an `EdgeCache` purge reaches the cache only from
/// the Worker's own `fetch`, so the instance is refused where anything else
/// runs: an agent handler (a Durable Object,
/// `bynk.cloudflare.edge_cache_in_agent`), a queue consumer
/// (`bynk.cloudflare.edge_cache_in_queue`), an `on schedule` handler
/// (`bynk.cloudflare.edge_cache_in_schedule`: deployed, its purge answers
/// `success` and clears nothing), and an exported capability, which a consumer
/// builds in its own Worker (`bynk.cloudflare.edge_cache_exported`).
/// A handler reaches the instance
/// through its own `given`, or through a local capability whose provider's
/// `given` does, at any depth.
pub fn phase_edge_cache_reach(
    groups: &BTreeMap<String, Vec<usize>>,
    parsed: &[ParsedFile],
    unit_tables: &HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
) {
    for (name, indices) in groups {
        let Some(table) = unit_tables.get(name) else {
            continue;
        };
        // Each capability that reaches an instance, with the instance it
        // reaches.
        let mut reaches: BTreeMap<String, String> = table
            .derived
            .iter()
            .filter(|(_, d)| d.is_edge_cache())
            .map(|(cap, _)| (cap.clone(), cap.clone()))
            .collect();
        if reaches.is_empty() {
            continue;
        }
        loop {
            let mut grew = false;
            for (cap, provider) in &table.providers {
                if reaches.contains_key(cap) {
                    continue;
                }
                let via = provider
                    .given
                    .iter()
                    .filter(|g| !g.is_cross_context())
                    .find_map(|g| reaches.get(g.key()).cloned());
                if let Some(instance) = via {
                    reaches.insert(cap.clone(), instance);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        for &i in indices {
            let path = parsed[i].identity_path();
            for item in parsed[i].items() {
                // A consuming context builds an exported capability's provider
                // in its own Worker, whose purge would miss this one's cache.
                if let CommonsItem::Capability(c) = item
                    && c.derived.is_none()
                    && table.exported_capabilities.contains(&c.name.name)
                    && let Some(instance) = reaches.get(&c.name.name)
                {
                    errors.push_for(
                        Some(&path),
                        CompileError::new(
                            "bynk.cloudflare.edge_cache_exported",
                            c.span,
                            format!(
                                "exported capability `{}` reaches `EdgeCache` instance `{instance}`, which purges only from this context's own Worker",
                                c.name.name
                            ),
                        )
                        .with_note(
                            "a context that consumes it would build its provider in its own Worker, whose purge misses this Worker's cache; export a service that purges instead",
                        ),
                    );
                }
                // Review of #2417: an opted-in Worker shares a `scope: public`
                // `2xx` with every later caller. `public` lets the cache store
                // even a response to a request carrying `Authorization` (RFC
                // 9111 §3.5, confirmed deployed), so such a route must not
                // depend on who calls it.
                if let CommonsItem::Service(svc) = item
                    && matches!(svc.protocol, ServiceProtocol::Http)
                {
                    for h in &svc.handlers {
                        let public = h.annotations.iter().any(|a| {
                            a.name.name == "cache"
                                && a.args.iter().any(|arg| {
                                    arg.label.as_ref().is_some_and(|l| l.name == "scope")
                                        && matches!(&arg.value.kind, ExprKind::Ident(id) if id.name == "public")
                                })
                        });
                        let Some(by) = h.by_clause.as_ref().or(svc.default_by.as_ref()) else {
                            continue;
                        };
                        let closed = by.actors.iter().find(|a| {
                            let single = ByClause {
                                binder: None,
                                actors: vec![(*a).clone()],
                                span: by.span,
                            };
                            !crate::actors::sum_has_open_member(&single, &table.actors)
                        });
                        if let (true, Some(actor)) = (public, closed) {
                            errors.push_for(
                                Some(&path),
                                CompileError::new(
                                    "bynk.cloudflare.edge_cache_public_authenticated",
                                    by.span,
                                    format!(
                                        "a `@cache(scope: public)` route by `{}` would share one caller's response with every caller at Cloudflare's edge",
                                        actor.name
                                    ),
                                )
                                .with_note(format!(
                                    "context `{name}` declares `EdgeCache`, and the edge cache keys a response by its URL alone; use `scope: private`, or serve the route `by Visitor`"
                                )),
                            );
                        }
                    }
                }
                let (handlers, code, place, note) = match item {
                    CommonsItem::Agent(a) => (
                        &a.handlers,
                        "bynk.cloudflare.edge_cache_in_agent",
                        format!("agent `{}`", a.name.name),
                        "an agent runs in a Durable Object, whose purge would not reach this Worker's cache; purge from the service handler that calls the agent",
                    ),
                    CommonsItem::Service(svc)
                        if matches!(svc.protocol, ServiceProtocol::Queue { .. }) =>
                    {
                        (
                            &svc.handlers,
                            "bynk.cloudflare.edge_cache_in_queue",
                            format!("queue consumer `{}`", svc.name.name),
                            "a queue consumer is not the entrypoint the cache serves, and its purge is not known to reach that cache; purge from a service handler",
                        )
                    }
                    CommonsItem::Service(svc) if matches!(svc.protocol, ServiceProtocol::Cron) => (
                        &svc.handlers,
                        "bynk.cloudflare.edge_cache_in_schedule",
                        format!("`on schedule` service `{}`", svc.name.name),
                        "Cloudflare accepts a purge from a cron run but it clears nothing from the cache the Worker's `fetch` serves; purge from a service handler",
                    ),
                    _ => continue,
                };
                for h in handlers {
                    for g in h.given.iter().filter(|g| !g.is_cross_context()) {
                        let Some(instance) = reaches.get(g.key()) else {
                            continue;
                        };
                        let how = if g.key() == instance {
                            String::new()
                        } else {
                            format!(" (through `{}`)", g.key())
                        };
                        errors.push_for(
                            Some(&path),
                            CompileError::new(
                                code,
                                g.span,
                                format!(
                                    "{place} reaches `EdgeCache` instance `{instance}`{how}, which purges only from the Worker's `fetch`"
                                ),
                            )
                            .with_note(note),
                        );
                    }
                }
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
        "EdgeCache" => "Edge",
        "Queue" => "OrderEvents",
        "Analytics" => "Signups",
        "D1" => "Orders",
        "StoredSecret" => "StripeKey",
        "Email" => "Mail",
        "Postgres" => "Main",
        "MySql" => "Legacy",
        "Vectorize" => "Docs",
        "Container" => "Renderer",
        "AiSearch" => "Kb",
        "MtlsCertificate" => "Backend",
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
        firstparty::JURISDICTION_OPTION => "\"eu\"",
        firstparty::RATELIMIT_LIMIT_OPTION => "100",
        firstparty::RATELIMIT_PERIOD_OPTION => "60.seconds",
        firstparty::QUEUE_NAME_OPTION => "\"order-events\"",
        firstparty::STORED_SECRET_STORE_OPTION => "\"payments\"",
        firstparty::STORED_SECRET_SECRET_OPTION => "\"stripe-live\"",
        firstparty::EMAIL_TO_OPTION => "\"ops@shop.example\"",
        firstparty::EMAIL_ALLOWED_RECIPIENTS_OPTION | firstparty::EMAIL_ALLOWED_SENDERS_OPTION => {
            "[\"orders@shop.example\"]"
        }
        firstparty::AI_SEARCH_INSTANCE_OPTION => "\"support-docs\"",
        firstparty::MTLS_CERTIFICATE_OPTION => "\"acme-client\"",
        firstparty::VECTORIZE_DIMENSIONS_OPTION => "768",
        firstparty::VECTORIZE_METRIC_OPTION => "\"cosine\"",
        firstparty::VECTORIZE_INDEXED_OPTION => "[\"category:string\"]",
        firstparty::CONTAINER_BUILD_OPTION => "\"containers/renderer\"",
        firstparty::CONTAINER_PORT_OPTION => "8080",
        firstparty::CONTAINER_SLEEP_AFTER_OPTION => "5.minutes",
        firstparty::CONTAINER_INSTANCE_TYPE_OPTION => "\"lite\"",
        firstparty::CONTAINER_FORWARD_OPTION => "[\"API_KEY\"]",
        _ => "\"legacy-billing\"",
    }
}

/// Why a `Vectorize` `@dimensions` literal is outside Cloudflare's range of
/// 32 to 1536: `("least", 32)` or `("most", 1536)`. `None` when it is inside, or
/// not positive (which the positive-`Int` check reports).
fn vectorize_dimensions_problem(value: i64) -> Option<(&'static str, i64)> {
    if value > firstparty::VECTORIZE_MAX_DIMENSIONS {
        Some(("most", firstparty::VECTORIZE_MAX_DIMENSIONS))
    } else if value > 0 && value < firstparty::VECTORIZE_MIN_DIMENSIONS {
        Some(("least", firstparty::VECTORIZE_MIN_DIMENSIONS))
    } else {
        None
    }
}

/// #3513 A: an `MtlsCertificate` `@certificate` that is not a certificate name. A
/// certificate is named by whoever uploaded it, in any text, so only a name the account
/// could not have given one is refused: an empty one, or one with a control character.
fn mtls_certificate_error(
    base: &str,
    option: &str,
    text: &str,
    span: bynk_syntax::span::Span,
) -> Option<CompileError> {
    (base == "MtlsCertificate"
        && option == crate::firstparty::MTLS_CERTIFICATE_OPTION
        && (text.trim().is_empty() || text.chars().any(char::is_control)))
    .then(|| {
        CompileError::new(
            "bynk.capability.invalid_option",
            span,
            format!("{text:?} is not a certificate name a `{base}` can reach"),
        )
        .with_note(
            "name the certificate as it was uploaded to the account (`wrangler mtls-certificate list` shows it), for example `@certificate(\"acme-client\")`",
        )
    })
}

/// #3480: an `AiSearch` `@instance` that is not an instance name. The instance
/// is a path segment of the REST call that links it, and Cloudflare's API
/// documents an instance id as 1 to 64 lowercase letters, digits, hyphens and
/// underscores.
fn ai_search_instance_error(
    base: &str,
    option: &str,
    text: &str,
    span: bynk_syntax::span::Span,
) -> Option<CompileError> {
    (base == "AiSearch"
        && option == crate::firstparty::AI_SEARCH_INSTANCE_OPTION
        && !is_ai_search_instance_name(text))
    .then(|| {
        CompileError::new(
            "bynk.capability.invalid_option",
            span,
            format!("`{text}` is not an instance name a `{base}` can reach"),
        )
        .with_note(
            "an instance name is 1 to 64 lowercase letters, digits, `-` or `_`, the form Cloudflare's API documents",
        )
    })
}

/// #3479 (DECISION A): a `Vectorize` `@metric` outside the closed set.
fn vectorize_metric_error(
    base: &str,
    option: &str,
    text: &str,
    span: bynk_syntax::span::Span,
) -> Option<CompileError> {
    (base == "Vectorize"
        && option == crate::firstparty::VECTORIZE_METRIC_OPTION
        && !crate::firstparty::VECTORIZE_METRICS.contains(&text))
    .then(|| {
        CompileError::new(
            "bynk.capability.invalid_option",
            span,
            format!("`{text}` is not a distance metric a `{base}` index can use"),
        )
        .with_note(format!(
            "the metrics are {}",
            crate::firstparty::VECTORIZE_METRICS
                .iter()
                .map(|m| format!("`\"{m}\"`"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    })
}

/// #3942 B: the bare name an option was given, when it was one that did not
/// resolve to a literal (see `annotation_consts::rewrite_instance_options`).
fn bare_name_arg(ann: &bynk_syntax::ast::Annotation) -> Option<String> {
    ann.args.first().and_then(|a| match &a.value.kind {
        ExprKind::Ident(id) => Some(id.name.clone()),
        // A list option's element that did not resolve (`@allowedRecipients([Ops])`).
        ExprKind::ListLit(items, ..) => items.iter().find_map(|i| match &i.kind {
            ExprKind::Ident(id) => Some(id.name.clone()),
            _ => None,
        }),
        _ => None,
    })
}

/// An instance option's own error, with a note on what a constant must be to
/// stand there when the option was given a name (#3942 B).
fn invalid_option_error(
    option: &str,
    named: Option<&str>,
    span: bynk_syntax::span::Span,
    message: String,
) -> CompileError {
    let error = CompileError::new("bynk.capability.invalid_option", span, message).with_note(
        format!("write `@{option}({})`, for example", option_sample(option)),
    );
    match named {
        Some(name) => error.with_note(format!(
            "`{name}` is not a constant declared exactly the option's type and bound to a literal, or to another such constant"
        )),
        None => error,
    }
}

/// Interop I5 (#1972): the value of one instance option: one positional
/// literal of the option's kind (P3, #2342). `@worker("name")` names a Worker,
/// so it must be a Worker name as Cloudflare takes one: lowercase letters,
/// digits, `-` and `_`, at most 63 characters, not starting or ending with
/// `-`. `@jurisdiction` is one of R2's; `@limit` a positive `Int`; `@period`
/// one of the rate limiting windows; a `Queue`'s `@name` a queue name (P2,
/// #2578). `None`, with the error pushed, when the
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
    let named = bare_name_arg(ann);
    let invalid =
        |span, message: String| invalid_option_error(option, named.as_deref(), span, message);
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
        (firstparty::OptionKind::StrList, ExprKind::ListLit(items, ..)) => {
            let mut out = Vec::new();
            for item in items {
                let ExprKind::StrLit(text) = &item.kind else {
                    errors.push_for(
                        Some(path),
                        invalid(
                            item.span,
                            format!("`@{option}` takes a list of string literals"),
                        ),
                    );
                    return None;
                };
                out.push(text.clone());
            }
            if out.is_empty() {
                errors.push_for(
                    Some(path),
                    invalid(
                        arg.span,
                        format!(
                            "`@{option}` takes at least one {}",
                            if base == "Container" {
                                "entry"
                            } else {
                                "address"
                            }
                        ),
                    ),
                );
                return None;
            }
            OptionValue::List(out)
        }
        (firstparty::OptionKind::StrList, _) => {
            errors.push_for(
                Some(path),
                invalid(
                    arg.span,
                    format!("`@{option}` takes a list of string literals"),
                ),
            );
            return None;
        }
        (firstparty::OptionKind::PositiveInt, ExprKind::IntLit { value, .. })
            if base == "Vectorize"
                && option == firstparty::VECTORIZE_DIMENSIONS_OPTION
                && vectorize_dimensions_problem(*value).is_some() =>
        {
            let (bound, limit) = vectorize_dimensions_problem(*value).unwrap_or_default();
            errors.push_for(
                Some(path),
                CompileError::new(
                    "bynk.capability.invalid_option",
                    arg.span,
                    format!("`@{option}` is at {bound} {limit} for a `{base}` index"),
                )
                .with_note("Cloudflare creates an index only with a dimension count in its supported range, 32 to 1536"),
            );
            return None;
        }
        (firstparty::OptionKind::PositiveInt, ExprKind::IntLit { value, .. })
            if base == "Container"
                && option == firstparty::CONTAINER_PORT_OPTION
                && crate::containers::port_problem(*value).is_some() =>
        {
            errors.push_for(
                Some(path),
                CompileError::new(
                    "bynk.capability.invalid_option",
                    arg.span,
                    format!("`@{option}` is not a port a `{base}` can listen on"),
                )
                .with_note("a port is a TCP port, 1 to 65535"),
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
        (firstparty::OptionKind::Duration, _) if base == "Container" => OptionValue::Seconds(
            container_sleep_after(&arg.value.kind, arg.span, path, errors)?,
        ),
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
    if let OptionValue::List(items) = &value {
        // #3477: a `Container`'s `@forward` names Worker secrets.
        if base == "Container" {
            return container_forward_ok(items, ann, path, errors).then_some(value);
        }
        // #3479: a `Vectorize`'s `@indexed` entries are metadata indexes.
        if base == "Vectorize" {
            return vectorize_indexed_ok(items, ann, path, errors).then_some(value);
        }
        // Every address is checked as the platform will see it (#3302).
        if !email_list_ok(items, base, ann, path, errors) {
            return None;
        }
        return Some(value);
    }
    let OptionValue::Str(text) = &value else {
        return Some(value);
    };
    if base == "Email" && option == crate::firstparty::EMAIL_TO_OPTION && !is_email_address(text) {
        errors.push_for(
            Some(path),
            invalid(
                ann.span,
                format!(
                    "`{}` is not an email address a `{base}` can use",
                    text.escape_debug()
                ),
            )
            .with_note("an address is `name@domain`, with no spaces or control characters"),
        );
        return None;
    }
    if option == crate::firstparty::JURISDICTION_OPTION
        && !jurisdiction_ok(text, base, ann, path, errors)
    {
        return None;
    }
    if let Some(error) = vectorize_metric_error(base, option, text, ann.span) {
        errors.push_for(Some(path), error);
        return None;
    }
    // #3477: a `Container`'s `@build` and `@instanceType`.
    if base == "Container" && !container_str_ok(text, option, ann, path, errors) {
        return None;
    }
    // Hyperdrive H5 (#3322, DECISION B): `@tls` is one of a closed set.
    if option == crate::firstparty::MYSQL_TLS_OPTION
        && !crate::firstparty::MYSQL_TLS_MODES
            .iter()
            .any(|(mode, _)| mode == text)
    {
        errors.push_for(
            Some(path),
            CompileError::new(
                "bynk.capability.invalid_option",
                ann.span,
                format!("`{text}` is not a TLS mode a `{base}` can use"),
            )
            .with_note(format!(
                "the modes are {}; `\"required\"` is the default",
                crate::firstparty::MYSQL_TLS_MODES
                    .iter()
                    .map(|(m, _)| format!("`\"{m}\"`"))
                    .collect::<Vec<_>>()
                    .join(", ")
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
    if let Some(error) = ai_search_instance_error(base, option, text, ann.span) {
        errors.push_for(Some(path), error);
        return None;
    }
    if let Some(error) = mtls_certificate_error(base, option, text, ann.span) {
        errors.push_for(Some(path), error);
        return None;
    }
    if base == "StoredSecret"
        && matches!(
            option,
            crate::firstparty::STORED_SECRET_STORE_OPTION
                | crate::firstparty::STORED_SECRET_SECRET_OPTION
        )
        && !is_secrets_store_name(text)
    {
        let what = if option == crate::firstparty::STORED_SECRET_STORE_OPTION {
            "store"
        } else {
            "secret"
        };
        errors.push_for(
            Some(path),
            CompileError::new(
                "bynk.capability.invalid_option",
                ann.span,
                format!("`{text}` is not a {what} name a `{base}` can read"),
            )
            .with_note("a Secrets Store name is 1 to 255 letters, digits, `-` or `_`"),
        );
        return None;
    }
    if option == crate::firstparty::QUEUE_NAME_OPTION && base == "Queue" && !is_queue_name(text) {
        errors.push_for(
            Some(path),
            CompileError::new(
                "bynk.capability.invalid_option",
                ann.span,
                format!("`{text}` is not a queue name a `{base}` can send to"),
            )
            .with_note(
                "a queue name is 1 to 63 lowercase letters, digits or `-`, and starts and ends with a letter or digit",
            ),
        );
        return None;
    }
    Some(value)
}

/// #3302: whether every entry of an `Email` instance's address list is an
/// address the platform takes. Pushes the error and returns `false` otherwise.
fn email_list_ok(
    items: &[String],
    base: &str,
    ann: &bynk_syntax::ast::Annotation,
    path: &std::path::Path,
    errors: &mut ErrorSink,
) -> bool {
    let option = ann.name.name.as_str();
    let Some(item) = items.iter().find(|item| !is_email_address(item)) else {
        return true;
    };
    errors.push_for(
        Some(path),
        CompileError::new(
            "bynk.capability.invalid_option",
            ann.span,
            format!(
                "`{}` is not an email address a `{base}` can use",
                item.escape_debug()
            ),
        )
        .with_note(format!(
            "write `@{option}({})`, for example",
            option_sample(option)
        ))
        .with_note("an address is `name@domain`, with no spaces or control characters"),
    );
    false
}

/// Whether `text` is a jurisdiction an `R2` bucket or a `D1` database can be
/// pinned to. Pushes the error and returns `false` otherwise.
fn jurisdiction_ok(
    text: &str,
    base: &str,
    ann: &bynk_syntax::ast::Annotation,
    path: &std::path::Path,
    errors: &mut ErrorSink,
) -> bool {
    if crate::firstparty::is_jurisdiction(text) {
        return true;
    }
    errors.push_for(
        Some(path),
        CompileError::new(
            "bynk.capability.invalid_option",
            ann.span,
            format!(
                "`{text}` is not a jurisdiction a `{base}` {} can be pinned to",
                if base == "D1" { "database" } else { "bucket" }
            ),
        )
        .with_note(crate::firstparty::jurisdictions_note()),
    );
    false
}

/// #3477: whether a `Container`'s string option `option` is a value the
/// platform takes: `@build` a directory inside the project, `@instanceType` one
/// of a closed set. Pushes the error and returns `false` otherwise.
fn container_str_ok(
    text: &str,
    option: &str,
    ann: &bynk_syntax::ast::Annotation,
    path: &std::path::Path,
    errors: &mut ErrorSink,
) -> bool {
    let error = if option == firstparty::CONTAINER_BUILD_OPTION {
        crate::containers::build_path_problem(text).map(|problem| {
            CompileError::new(
                "bynk.container.build_path",
                ann.span,
                format!("`@build(\"{}\")` {problem}", text.escape_debug()),
            )
            .with_note(
                "`@build` names the directory that holds the `Dockerfile`, relative to the project root, for example `@build(\"containers/renderer\")`",
            )
        })
    } else if option == firstparty::CONTAINER_INSTANCE_TYPE_OPTION
        && !firstparty::CONTAINER_INSTANCE_TYPES.contains(&text)
    {
        Some(
            CompileError::new(
                "bynk.capability.invalid_option",
                ann.span,
                format!("`{text}` is not an instance type a `Container` can start with"),
            )
            .with_note(format!(
                "the instance types are {}",
                firstparty::CONTAINER_INSTANCE_TYPES
                    .iter()
                    .map(|t| format!("`\"{t}\"`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        )
    } else {
        None
    };
    let Some(error) = error else {
        return true;
    };
    errors.push_for(Some(path), error);
    false
}

/// #3477: a `Container`'s `@sleepAfter` in whole seconds: more than zero and at
/// most 6 hours. Pushes the error and returns `None` otherwise.
fn container_sleep_after(
    kind: &ExprKind,
    span: bynk_syntax::span::Span,
    path: &std::path::Path,
    errors: &mut ErrorSink,
) -> Option<i64> {
    let seconds = match kind {
        ExprKind::DurationLit { millis, .. } => crate::containers::sleep_after_seconds(*millis),
        _ => None,
    };
    if seconds.is_none() {
        errors.push_for(
            Some(path),
            CompileError::new(
                "bynk.capability.invalid_option",
                span,
                "`@sleepAfter` is not a time a `Container` can sleep after".to_string(),
            )
            .with_note(
                "write a whole number of seconds, more than zero and at most 6 hours, for example `5.minutes`",
            ),
        );
    }
    seconds
}

/// #3477 (DECISION J): whether a `Container`'s `@forward` is a list of Worker
/// secret names a process environment can hold. Pushes the error and returns
/// `false` otherwise.
fn container_forward_ok(
    items: &[String],
    ann: &bynk_syntax::ast::Annotation,
    path: &std::path::Path,
    errors: &mut ErrorSink,
) -> bool {
    let Some(message) = crate::containers::forward_problem(items) else {
        return true;
    };
    errors.push_for(
        Some(path),
        CompileError::new("bynk.capability.invalid_option", ann.span, message).with_note(
            "write `@forward([\"API_KEY\"])`: the names of Worker secrets, each a letter or `_` then letters, digits or `_`; their values are never written into source or config",
        ),
    );
    false
}

/// #3479 (DECISION C): whether a `Vectorize`'s `@indexed` entries are a list of
/// at most [`firstparty::VECTORIZE_MAX_INDEXED`] metadata indexes, each
/// `name:type` with a type the platform indexes, none repeated. Pushes the
/// error and returns `false` otherwise.
fn vectorize_indexed_ok(
    items: &[String],
    ann: &bynk_syntax::ast::Annotation,
    path: &std::path::Path,
    errors: &mut ErrorSink,
) -> bool {
    let types = firstparty::VECTORIZE_INDEXED_TYPES;
    let problem = if items.len() > firstparty::VECTORIZE_MAX_INDEXED {
        Some(format!(
            "an index has at most {} metadata indexes, and `@indexed` lists {}",
            firstparty::VECTORIZE_MAX_INDEXED,
            items.len()
        ))
    } else {
        items.iter().enumerate().find_map(|(i, item)| {
            let Some((name, ty)) = item.split_once(':') else {
                return Some(format!("`{}` is not `name:type`", item.escape_debug()));
            };
            if !types.contains(&ty) {
                return Some(format!(
                    "`{}` is not a type a metadata index holds",
                    ty.escape_debug()
                ));
            }
            if !is_metadata_property(name) {
                return Some(format!(
                    "`{}` is not a metadata property a filter can name",
                    name.escape_debug()
                ));
            }
            items[..i]
                .iter()
                .any(|p| p.split_once(':').is_some_and(|(n, _)| n == name))
                .then(|| format!("`{name}` is listed twice"))
        })
    };
    let Some(message) = problem else {
        return true;
    };
    errors.push_for(
        Some(path),
        CompileError::new("bynk.capability.invalid_option", ann.span, message).with_note(format!(
            "write `@indexed([\"category:string\", \"year:number\"])`: at most {} entries, each `name:type` with the type {}",
            firstparty::VECTORIZE_MAX_INDEXED,
            types
                .iter()
                .map(|t| format!("`{t}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )),
    );
    false
}

/// Whether `name` is a metadata property a filter can name (#3479): a dotted
/// path whose every segment is non-empty, has no `"` or `|` and does not start
/// with `$`, at most 512 characters in all. A dot nests (`author.name`).
fn is_metadata_property(name: &str) -> bool {
    name.chars().count() <= 512
        && name
            .split('.')
            .all(|seg| !seg.is_empty() && !seg.starts_with('$') && !seg.contains(['"', '|']))
}

/// Whether `name` is a Cloudflare queue name ([`instance_option_value`]):
/// Cloudflare allows letters, digits and `-`, starting and ending with a letter
/// or digit, 1 to 63 characters. Lowercase only, as every name `bynk deploy`
/// derives is, and so `env_qualify`'s `-<env>` suffix reads the same. The suffix
/// counts toward the 63, which this cannot see: a name near the limit can
/// overflow once qualified, and nothing checks that before `wrangler queues create`.
pub(crate) fn is_queue_name(name: &str) -> bool {
    let alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    (1..=63).contains(&name.len())
        && name.bytes().all(|b| alnum(b) || b == b'-')
        && name.bytes().next().is_some_and(alnum)
        && name.bytes().last().is_some_and(alnum)
}

/// Whether `text` can be an email address (#3302): `local@domain`, one `@`,
/// both sides non-empty, a host name after it, no whitespace, control or angle-bracket characters.
/// The platform judges the rest; this only keeps a typo, or a header-shaped
/// string, out of a deploy-time policy that is written into configuration.
pub(crate) fn is_email_address(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        // A host name: the domain is looked up by the deploy.
        && domain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && !text
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | ','))
}

/// Whether `name` is a Secrets Store's or a secret's name as Cloudflare takes
/// one ([`instance_option_value`], #3301): 1 to 255 letters, digits, `-` or
/// `_`.
pub(crate) fn is_secrets_store_name(name: &str) -> bool {
    (1..=255).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Whether `name` can name an AI Search instance ([`instance_option_value`]).
fn is_ai_search_instance_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
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
/// relative project root (`bynk compile .`, the ordinary CLI shape) leaves
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
            if conflicts_with_first_party_pin(dep, &adapter, &identity, errors) {
                continue;
            }
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

/// ADR 0799 C: the toolchain pins its own npm packages exactly, and a project
/// installs one version of each package, so an adapter may name such a package
/// only at that version. Reports `bynk.requires.conflicting_range` and says so.
fn conflicts_with_first_party_pin(
    dep: &bynk_syntax::ast::RequiresDep,
    adapter: &str,
    identity: &Path,
    errors: &mut ErrorSink,
) -> bool {
    let Some(pin) = crate::firstparty::FIRST_PARTY_NPM
        .iter()
        .find(|p| p.package == dep.package && p.version != dep.range)
    else {
        return false;
    };
    errors.push_for(
        Some(identity),
        CompileError::new(
            "bynk.requires.conflicting_range",
            dep.span,
            format!(
                "adapter `{adapter}` requires `{}` at `{}`, but the toolchain pins it at `{}`",
                dep.package, dep.range, pin.version
            ),
        )
        .with_note(
            "the toolchain's own binding uses this package at an exact version; an adapter that needs it must require that version",
        ),
    );
    true
}

/// #3984/#4348: the `uses` map a suite's view sees: each target's own, plus the test-only commons
/// the `suites` (indices into `parsed`) `uses`, so those names reach them as they reach the
/// target's own commons. Only the test pass reads it, so no deployable module imports a
/// test-only commons. Checking passes one suite at a time, so a test-only commons is in scope only
/// where it is `uses`d; the emitted test module passes every suite.
pub fn unit_uses_with_suite_uses(
    unit_uses: &HashMap<String, Vec<String>>,
    parsed: &[ParsedFile],
    suites: &[usize],
) -> HashMap<String, Vec<String>> {
    let test_only: HashSet<String> = parsed
        .iter()
        .filter_map(|pf| match pf.unit() {
            SourceUnit::Commons(c) if c.test_only => Some(c.name.joined()),
            _ => None,
        })
        .collect();
    let mut out = unit_uses.clone();
    for pf in suites.iter().map(|&i| &parsed[i]) {
        let SourceUnit::Suite(t) = pf.unit() else {
            continue;
        };
        for u in t.uses.iter().map(|u| u.target.joined()) {
            if test_only.contains(&u) {
                let entry = out.entry(t.target.joined()).or_default();
                if !entry.contains(&u) {
                    entry.push(u);
                }
            }
        }
    }
    out
}

/// #3984: a `commons x.y for tests` is the test build's alone. A unit that is not a suite or
/// another test-only commons and `uses` one is `bynk.uses.test_only_commons`, and a deployable
/// build (`tests` off) leaves every test-only commons out, as it does a suite.
pub fn phase_test_only_commons(parsed: &mut Vec<ParsedFile>, tests: bool, errors: &mut ErrorSink) {
    let is_test_only = |pf: &ParsedFile| matches!(pf.unit(), SourceUnit::Commons(c) if c.test_only);
    let names: HashSet<String> = parsed
        .iter()
        .filter(|pf| is_test_only(pf))
        .map(|pf| match pf.unit() {
            SourceUnit::Commons(c) => c.name.joined(),
            _ => String::new(),
        })
        .collect();
    if names.is_empty() {
        return;
    }
    let mut refused = false;
    for pf in parsed.iter() {
        if is_test_only(pf) || matches!(pf.unit(), SourceUnit::Suite(_)) {
            continue;
        }
        for u in pf
            .uses()
            .iter()
            .filter(|u| names.contains(&u.target.joined()))
        {
            refused = true;
            errors.push_for(
                Some(pf.identity_path().as_path()),
                CompileError::new(
                    "bynk.uses.test_only_commons",
                    u.span,
                    format!(
                        "`{}` is a test-only commons, which only a suite or another test-only commons may `uses`",
                        u.target.joined()
                    ),
                )
                .with_note("a test-only commons (`commons x.y for tests`) is left out of every deployable, so production code cannot depend on it; move what it needs into a production commons"),
            );
        }
    }
    // A refused `uses` leaves the commons in, so it is reported once, not again as unknown.
    if !tests && !refused {
        parsed.retain(|pf| !is_test_only(pf));
    }
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
        // #4111: `consumes bynk { Secret as Token }` writes the type as `Token` here, so a
        // file that renames a type sees it under the new name too.
        let aliased = with_consumed_type_aliases(&types, pf.consumes());
        check_function_type_boundary_items(
            pf.items(),
            aliased.as_ref().unwrap_or(&types),
            &mut file_errors,
        );
        for err in file_errors {
            errors.push_for(Some(&pf.identity_path()), err);
        }
    }
}

/// `types` plus, for each `consumes U { T as Alias }` of a file, `Alias` naming the declaration
/// of `T`; `None` when the file renames nothing, so the common case copies nothing.
fn with_consumed_type_aliases(
    types: &HashMap<String, Arc<TypeDecl>>,
    consumes: &[ConsumesDecl],
) -> Option<HashMap<String, Arc<TypeDecl>>> {
    let mut out: Option<HashMap<String, Arc<TypeDecl>>> = None;
    for item in consumes.iter().flat_map(|c| c.selected.iter().flatten()) {
        let (Some(alias), Some(decl)) = (&item.alias, types.get(&item.name.name)) else {
            continue;
        };
        out.get_or_insert_with(|| types.clone())
            .entry(alias.name.clone())
            .or_insert_with(|| Arc::clone(decl));
    }
    out
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

/// #3192: a generic type may refer to itself only uniformly (its own parameters,
/// in order); any other recursive edge is polymorphic recursion, refused at the
/// declaration with `bynk.generics.polymorphic_recursion`. Uniform recursion
/// declares and works in memory and is refused only where a codec is required.
fn check_polymorphic_recursion(
    decl: &TypeDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    for (callee, span) in bynk_syntax::ast::polymorphic_recursion_edges(decl, types) {
        let name = &decl.name.name;
        let params = decl
            .type_params
            .iter()
            .map(|p| p.name.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let note = if callee == *name {
            format!(
                "a recursive generic type must pass its parameters through unchanged, as `{callee}[{params}]`"
            )
        } else {
            format!(
                "a mutually recursive generic may be applied only to `{name}`'s own type parameters ({params}) or to types that mention none; wrapping a parameter (`List[T]`) makes the set of instantiations unbounded"
            )
        };
        errors.push(
            CompileError::new(
                "bynk.generics.polymorphic_recursion",
                span,
                format!(
                    "`{name}` refers to `{callee}` with arguments other than its own type parameters — polymorphic recursion generates an unbounded set of instantiations"
                ),
            )
            .with_note(note),
        );
    }
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
                    check_polymorphic_recursion(t, types, errors);
                    // #4111: `bynk`'s own `Request` holds an `Authorization`, which is how a secret
                    // reaches `Fetch`; its fields are not the ones a program declares.
                    if !crate::context_checks::is_firstparty_decl(t) {
                        for f in &r.fields {
                            reject_fn_types(&f.type_ref, RECORD_FIELD, types, errors);
                        }
                    }
                }
                TypeBody::Sum(s) => {
                    check_polymorphic_recursion(t, types, errors);
                    for v in &s.variants {
                        for p in &v.payload {
                            reject_fn_types(&p.type_ref, SUM_PAYLOAD, types, errors);
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
                        reject_fn_types(&p.type_ref, CAPABILITY_SIGNATURE, types, errors);
                    }
                    // v0.102 (§2.9.1): a capability operation may *produce* a
                    // held value — it is the canonical held source — so an
                    // `Effect[Connection[F]]` return is admitted.
                    if !type_ref_is_held(&op.return_type) {
                        reject_fn_types(&op.return_type, CAPABILITY_SIGNATURE, types, errors);
                    }
                }
            }
            CommonsItem::Service(s) => {
                for h in &s.handlers {
                    for p in &h.params {
                        // #2341 (decision A): an HTTP handler's `headers` is
                        // the one place `RequestHeaders` may appear.
                        let http_headers = matches!(h.kind, HandlerKind::Http { .. })
                            && ((p.name.name == "headers"
                                && matches!(p.type_ref, TypeRef::RequestHeaders(_)))
                                // #2854: and `query` is the one place `RequestQuery` may.
                                || (p.name.name == "query"
                                    && matches!(p.type_ref, TypeRef::RequestQuery(_)))
                                // #3277: and `cookies` the one place `RequestCookies` may.
                                || (p.name.name == "cookies"
                                    && matches!(p.type_ref, TypeRef::RequestCookies(_))));
                        // #2341 (decision C): and `on rejected`'s parameter is
                        // the one place `BoundaryRejection` may appear.
                        let http_headers = http_headers
                            || (h.kind == HandlerKind::Rejected
                                && matches!(p.type_ref, TypeRef::BoundaryRejection(_)));
                        // v0.102 (§2.9.4): the framework may supply a held
                        // value as a handler parameter (the `on open`
                        // connection), so a `Connection[F]` parameter is
                        // admitted.
                        // #2854: `RequestQuery` off an HTTP route has no request
                        // whose query string it could be.
                        if matches!(p.type_ref, TypeRef::RequestQuery(_))
                            && !matches!(h.kind, HandlerKind::Http { .. })
                        {
                            errors.push(
                                CompileError::new(
                                    "bynk.http.query_not_http",
                                    p.type_ref.span(),
                                    "`RequestQuery` is the query string of an HTTP request, so only an `on http` handler's `query` parameter can have it",
                                )
                                .with_note("a handler that is not an HTTP route has no query string to read"),
                            );
                            continue;
                        }
                        // #3277: `RequestCookies` off an HTTP route has no request
                        // whose cookies it could be.
                        if matches!(p.type_ref, TypeRef::RequestCookies(_))
                            && !matches!(h.kind, HandlerKind::Http { .. })
                        {
                            errors.push(
                                CompileError::new(
                                    "bynk.http.cookies_not_http",
                                    p.type_ref.span(),
                                    "`RequestCookies` is the cookies of an HTTP request, so only an `on http` handler's `cookies` parameter can have it",
                                )
                                .with_note("a handler that is not an HTTP route has no cookies to read"),
                            );
                            continue;
                        }
                        // #3276: a `@multipart` handler's `body` is where its `File`
                        // parts are received.
                        let multipart_body = p.name.name == "body"
                            && h.annotations.iter().any(|a| a.name.name == "multipart");
                        // #3343: and the record a request reader fills (`cookies`, `headers` (#3911), `query`,
                        // a `@form` body) is where `@name` fields are allowed.
                        let request_record = matches!(h.kind, HandlerKind::Http { .. })
                            && (matches!(p.name.name.as_str(), "cookies" | "headers" | "query")
                                || (p.name.name == "body"
                                    && h.annotations.iter().any(|a| a.name.name == "form")));
                        // #3380: a `@raw` handler's `body` may be a `Stream[Bytes]`, the
                        // request body as it arrives; without `@raw` it has no wire form,
                        // so it is refused with the fix rather than a generic boundary
                        // error. Any other stream position keeps `stream_at_boundary`.
                        let is_http_body = matches!(h.kind, HandlerKind::Http { .. })
                            && p.name.name == "body";
                        let has_raw = h.annotations.iter().any(|a| a.name.name == "raw");
                        // Any stream on a `@raw` body passes this check: `@raw`'s own check
                        // refuses an element type but `Bytes` (`raw_body_not_bytes`).
                        let streamed_body = is_http_body
                            && matches!(p.type_ref, TypeRef::Stream(..))
                            && has_raw;
                        if is_http_body
                            && crate::context_checks::is_byte_stream(&p.type_ref)
                            && !has_raw
                        {
                            errors.push(
                                CompileError::new(
                                    "bynk.http.stream_body_not_raw",
                                    p.type_ref.span(),
                                    "a `Stream[Bytes]` `body` needs `@raw`",
                                )
                                .with_note(
                                    "a request body has a JSON wire form unless the route says otherwise; write `@raw(contentType: \"…\")` and a `@limit` to receive it as a stream",
                                ),
                            );
                            continue;
                        }
                        if streamed_body {
                            continue;
                        }
                        if !type_ref_is_held(&p.type_ref) && !http_headers && !multipart_body {
                            // Only the `@name` arm is relaxed for a request record;
                            // every other boundary check still runs on it.
                            let what = if request_record {
                                REQUEST_RECORD_PARAM
                            } else {
                                "a service handler signature"
                            };
                            reject_fn_types(&p.type_ref, what, types, errors);
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
            | CommonsItem::Embed(_)
            // #2617: a constant's initialiser is static, so it can never hold
            // a function value; its type needs no boundary rule.
            | CommonsItem::Const(_) => {}
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

/// Phases 4, 5 and 5a: build the unit tables, resolve each unit's `uses`, then
/// resolve a constant named by a literal-only annotation argument to the literal
/// it denotes (#3445, [`crate::annotation_consts`]). The last rewrites `parsed`
/// and the tables built from it, ahead of every reader of annotation arguments.
pub fn phase_tables_uses_consts(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &mut [ParsedFile],
    policy: &crate::stability::StabilityPolicy,
    errors: &mut ErrorSink,
    refs: &mut RefSink,
) -> (HashMap<String, UnitTable>, HashMap<String, Vec<String>>) {
    let mut unit_tables = phase_symbol_tables(groups, kinds, parsed, errors);
    let unit_uses = phase_resolve_uses(groups, kinds, parsed, &unit_tables, policy, errors);
    // #4455: the items of a braced `uses` list are references to what they select.
    crate::uses_view::record_selected_items(&unit_uses, parsed, groups, &unit_tables, refs);
    crate::annotation_consts::resolve_annotation_consts(
        groups,
        kinds,
        parsed,
        &mut unit_tables,
        &unit_uses,
        refs,
    );
    (unit_tables, unit_uses)
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

/// #3536: each context's or adapter's `consumes` clauses as `(target, listed
/// names)` — `None` for the whole-module form (no braces), `Some(names)` for a
/// braced list — across all the unit's files, for
/// [`crate::symbols::record_unselected_consumed_types`].
pub fn consume_selections(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    parsed: &[ParsedFile],
) -> crate::symbols::ConsumeSelections {
    let mut out = HashMap::new();
    for (name, indices) in groups {
        if !matches!(kinds.get(name), Some(UnitKind::Context | UnitKind::Adapter)) {
            continue;
        }
        let clauses: Vec<(String, Option<Vec<String>>)> = indices
            .iter()
            .flat_map(|&i| parsed[i].consumes())
            .map(|c| {
                let names = c
                    .selected
                    .as_ref()
                    .map(|items| items.iter().map(|n| n.name.name.clone()).collect());
                (c.target.joined(), names)
            })
            .collect();
        out.insert(name.clone(), clauses);
    }
    out
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
    HashMap<String, HashMap<String, crate::symbols::ConsumedTypeAlias>>,
) {
    let mut unit_consumes: HashMap<String, Vec<String>> = HashMap::new();
    // #2887: `consumes U { T as Alias }` — unit → alias → the type it renames.
    let mut unit_type_aliases: HashMap<String, HashMap<String, crate::symbols::ConsumedTypeAlias>> =
        HashMap::new();
    // v0.17: `consumes U { Cap, … }` flattens selected caps into the consumer's
    // local namespace. unit → bare-cap → consumed unit providing it.
    let mut unit_flattened: HashMap<String, HashMap<String, String>> = HashMap::new();
    // #2765: does `target` export the type `name` (`exports transparent|opaque`)?
    let exports_type = |target: &str, name: &str| {
        groups.get(target).is_some_and(|files| {
            files.iter().any(|&j| {
                parsed[j].exports().iter().any(|clause| {
                    matches!(clause.kind, ExportKind::Type(_))
                        && clause.names.iter().any(|e| e.name.name == name)
                })
            })
        })
    };
    // #2765: type names already selected, per (file, target), so a repeat in
    // one file is refused as a repeated capability is.
    let mut type_selected: HashSet<(usize, String, String)> = HashSet::new();
    for (name, indices) in groups {
        let kind = *kinds.get(name).unwrap();
        let mut consumes_targets: Vec<String> = Vec::new();
        let mut flattened: HashMap<String, String> = HashMap::new();
        // #1857: the file that first selected each flattened capability, so a
        // file of a split context may repeat another file's selection.
        let mut flattened_in: HashMap<String, usize> = HashMap::new();
        // #2887: the aliases of this unit, the alias each `(target, type)` was
        // given (and in which file), and the types selected bare. A context's
        // `consumes` apply to all its files, so another file may repeat an
        // alias; only a pair that disagrees is refused.
        let mut type_aliases: HashMap<String, crate::symbols::ConsumedTypeAlias> = HashMap::new();
        let mut alias_of: HashMap<(String, String), (String, usize)> = HashMap::new();
        let mut bare_types: HashSet<(String, String)> = HashSet::new();
        let mut alias_sites: HashMap<String, (usize, Span)> = HashMap::new();
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
                // #2765: braces that select only types name no capability, so
                // they trip the same guard.
                let selects_only_types = c.selected.as_ref().is_some_and(|names| {
                    let caps = unit_tables.get(&target).map(|t| &t.exported_capabilities);
                    !names.is_empty()
                        && names.iter().all(|n| {
                            !caps.is_some_and(|c| c.contains(&n.name.name))
                                && exports_type(&target, &n.name.name)
                        })
                });
                if kind == UnitKind::Adapter && (c.selected.is_none() || selects_only_types) {
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
                // #4044: a commons consumes types only: braced, from an adapter.
                if kind == UnitKind::Commons
                    && let Some(err) = commons_consumes_refusal(
                        c,
                        &target,
                        target_kind,
                        unit_tables,
                        &exports_type,
                    )
                {
                    errors.push_for(Some(&parsed[i].identity_path()), err);
                    continue;
                }
                // v0.18: adapter dependencies are adapter-to-adapter (spec §4.5) —
                // an adapter consuming a *context* would pull service logic into
                // the host boundary.
                if kind == UnitKind::Adapter && target_kind == UnitKind::Context {
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        adapter_consumes_context(name, &target, c.span),
                    );
                    continue;
                }
                if target == *name {
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        consumes_self_reference(kind, name, c.span),
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
                    for item in names {
                        let cap = &item.name;
                        for err in crate::cap_params::selection_gates(
                            policy,
                            &target,
                            cap,
                            parsed[i].is_synthetic(),
                            kind == UnitKind::Commons && exported.contains(&cap.name),
                        ) {
                            errors.push_for(Some(&parsed[i].identity_path()), err);
                        }
                        // #2765: the braces may also name a type `U` exports
                        // (`consumes bynk { TimedOut }`). Exported types are
                        // already in scope for any `consumes U`, so a type
                        // selection has nothing to flatten; it only states the
                        // dependency.
                        if let Some(alias) = &item.alias {
                            let key = (target.clone(), cap.name.clone());
                            let refuse =
                                |code: &'static str, span: Span, msg: String, note: String| {
                                    CompileError::new(code, span, msg).with_note(note)
                                };
                            if exported.contains(&cap.name) {
                                errors.push_for(
                                    Some(&parsed[i].identity_path()),
                                    refuse(
                                        "bynk.consumes.type_alias_capability",
                                        alias.span,
                                        format!(
                                            "`{}` is a capability of `{target}`; only exported types can be aliased",
                                            cap.name
                                        ),
                                        "a capability keeps its own name: select it bare (`consumes U { Cap }`), or qualify it as `given U.Cap`".to_string(),
                                    ),
                                );
                                continue;
                            }
                            if !exports_type(&target, &cap.name) {
                                errors.push_for(
                                    Some(&parsed[i].identity_path()),
                                    refuse(
                                        "bynk.consumes.type_alias_unknown_export",
                                        cap.span,
                                        format!(
                                            "`{target}` does not export a type named `{}`, so there is nothing to alias",
                                            cap.name
                                        ),
                                        "only an exported type can be aliased: `consumes U { Type as Alias }`".to_string(),
                                    ),
                                );
                                continue;
                            }
                            if !type_selected.insert((i, target.clone(), cap.name.clone())) {
                                errors.push_for(
                                    Some(&parsed[i].identity_path()),
                                    refuse(
                                        "bynk.consumes.type_alias_repeated",
                                        cap.span,
                                        format!(
                                            "`{}` is selected from `{target}` more than once in this file",
                                            cap.name
                                        ),
                                        "a type is aliased once; remove the repeat".to_string(),
                                    ),
                                );
                                continue;
                            }
                            if let Some((prev, prev_file)) = alias_of.get(&key)
                                && (*prev != alias.name || *prev_file == i)
                            {
                                errors.push_for(
                                    Some(&parsed[i].identity_path()),
                                    refuse(
                                        "bynk.consumes.type_alias_repeated",
                                        alias.span,
                                        format!(
                                            "`{}` from `{target}` is already aliased as `{prev}`",
                                            cap.name
                                        ),
                                        "a type is in scope under one alias; remove one of them"
                                            .to_string(),
                                    ),
                                );
                                continue;
                            }
                            if bare_types.contains(&key) {
                                errors.push_for(
                                    Some(&parsed[i].identity_path()),
                                    refuse(
                                        "bynk.consumes.type_alias_repeated",
                                        alias.span,
                                        format!(
                                            "`{}` from `{target}` is selected both bare and aliased",
                                            cap.name
                                        ),
                                        "the type is in scope as the alias only; drop the bare selection".to_string(),
                                    ),
                                );
                                continue;
                            }
                            let this = crate::symbols::ConsumedTypeAlias {
                                target: target.clone(),
                                original: cap.name.clone(),
                            };
                            if let Some(other) = type_aliases.get(&alias.name)
                                && *other != this
                            {
                                errors.push_for(
                                    Some(&parsed[i].identity_path()),
                                    refuse(
                                        "bynk.consumes.type_alias_conflict",
                                        alias.span,
                                        format!(
                                            "alias `{}` already names `{}` from `{}`",
                                            alias.name, other.original, other.target
                                        ),
                                        "give each aliased type its own name".to_string(),
                                    ),
                                );
                                continue;
                            }
                            alias_sites
                                .entry(alias.name.clone())
                                .or_insert((i, alias.span));
                            alias_of.insert(key, (alias.name.clone(), i));
                            type_aliases.insert(alias.name.clone(), this);
                            // Both names lead to the real declaration.
                            refs.record_in_unit(cap.span, SymbolKind::Type, &cap.name, &target);
                            refs.record_in_unit(alias.span, SymbolKind::Type, &cap.name, &target);
                            continue;
                        }
                        if !exported.contains(&cap.name) && exports_type(&target, &cap.name) {
                            if alias_of.contains_key(&(target.clone(), cap.name.clone())) {
                                errors.push_for(
                                    Some(&parsed[i].identity_path()),
                                    CompileError::new(
                                        "bynk.consumes.type_alias_repeated",
                                        cap.span,
                                        format!(
                                            "`{}` from `{target}` is selected both bare and aliased",
                                            cap.name
                                        ),
                                    )
                                    .with_note("the type is in scope as the alias only; drop the bare selection"),
                                );
                                continue;
                            }
                            bare_types.insert((target.clone(), cap.name.clone()));
                            if !type_selected.insert((i, target.clone(), cap.name.clone())) {
                                errors.push_for(
                                    Some(&parsed[i].identity_path()),
                                    CompileError::new(
                                        "bynk.consumes.capability_name_clash",
                                        cap.span,
                                        format!(
                                            "`{}` is selected from `{target}` more than once in this file",
                                            cap.name
                                        ),
                                    ),
                                );
                                continue;
                            }
                            refs.record_in_unit(cap.span, SymbolKind::Type, &cap.name, &target);
                            continue;
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
        // An alias is a type name; a capability of the same name (flattened
        // from a `consumes`, or local) would bind it twice in the emitted
        // module and in the unit's own scope.
        let mut clashing: Vec<&String> = type_aliases
            .keys()
            .filter(|a| flattened.contains_key(*a) || local_caps.contains(*a))
            .collect();
        clashing.sort();
        for alias in clashing {
            let (file, span) = alias_sites[alias];
            errors.push_for(
                Some(&parsed[file].identity_path()),
                CompileError::new(
                    "bynk.consumes.type_alias_conflict",
                    span,
                    format!("alias `{alias}` is also the name of a capability in this context"),
                )
                .with_note("choose an alias that is not a capability's name"),
            );
        }
        unit_flattened.insert(name.clone(), flattened);
        if !type_aliases.is_empty() {
            unit_type_aliases.insert(name.clone(), type_aliases);
        }
    }
    (unit_consumes, unit_flattened, unit_type_aliases)
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

/// #2618 (decision C): a public declaration may not name a `private` type of
/// its own unit in its signature, since a unit that `uses` it could neither
/// name nor build a value of that type. Checked positions: a public `fn`'s
/// parameter and return types (a method's too, unless its type is itself
/// private, when the method is out of an importer's reach with it), a public
/// type's record fields and variant payloads, a public constant's type
/// annotation, and a context's `exports` list.
/// Each is `bynk.visibility.private_in_public`.
///
/// A service, agent, capability, provider, actor or event signature is not
/// checked: how far those reach is a context's `exports` question, and the
/// cross-context structural rule (§6.5) is unchanged.
pub fn phase_private_in_public(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    errors: &mut ErrorSink,
) {
    for (unit, indices) in groups {
        let mut private_types: HashSet<String> = HashSet::new();
        let mut type_params: HashMap<String, Vec<TypeParam>> = HashMap::new();
        for &i in indices {
            for item in parsed[i].items() {
                if let CommonsItem::Type(t) = item {
                    if t.private.is_some() {
                        private_types.insert(t.name.name.clone());
                    }
                    type_params.insert(t.name.name.clone(), t.type_params.clone());
                }
            }
        }
        if private_types.is_empty() {
            continue;
        }
        for &i in indices {
            let file = parsed[i].identity_path();
            let mut report = |at: Span, decl: &str, ty: &str| {
                errors.push_for(
                    Some(&file),
                    CompileError::new(
                        "bynk.visibility.private_in_public",
                        at,
                        format!("public {decl} names `{ty}`, a private type of `{unit}`"),
                    )
                    .with_note(format!(
                        "a unit that uses `{unit}` could neither name nor build a `{ty}`; make `{ty}` public, or mark the declaration `private` too"
                    )),
                );
            };
            let first_private = |r: &TypeRef, vars: &HashSet<&str>| -> Option<String> {
                let mut names = Vec::new();
                type_ref_names(r, vars, &mut names);
                names.into_iter().find(|n| private_types.contains(n))
            };
            for item in parsed[i].items() {
                match item {
                    CommonsItem::Fn(f) if f.private.is_none() => {
                        let (decl, attached) = match &f.name {
                            FnName::Free(id) => (format!("function `{}`", id.name), &[][..]),
                            FnName::Method {
                                type_name,
                                method_name,
                            } => {
                                if private_types.contains(&type_name.name) {
                                    continue;
                                }
                                (
                                    format!("method `{}.{}`", type_name.name, method_name.name),
                                    type_params
                                        .get(&type_name.name)
                                        .map(Vec::as_slice)
                                        .unwrap_or_default(),
                                )
                            }
                        };
                        let vars: HashSet<&str> = f
                            .type_params
                            .iter()
                            .chain(attached)
                            .map(|p| p.name.name.as_str())
                            .collect();
                        let positions = f
                            .params
                            .iter()
                            .map(|p| &p.type_ref)
                            .chain(std::iter::once(&f.return_type));
                        for r in positions {
                            if let Some(ty) = first_private(r, &vars) {
                                report(r.span(), &decl, &ty);
                            }
                        }
                    }
                    CommonsItem::Type(t) if t.private.is_none() => {
                        let vars: HashSet<&str> =
                            t.type_params.iter().map(|p| p.name.name.as_str()).collect();
                        let decl = format!("type `{}`", t.name.name);
                        let positions: Vec<&TypeRef> = match &t.body {
                            TypeBody::Record(r) => r.fields.iter().map(|f| &f.type_ref).collect(),
                            TypeBody::Sum(s) => s
                                .variants
                                .iter()
                                .flat_map(|v| v.payload.iter().map(|p| &p.type_ref))
                                .collect(),
                            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => Vec::new(),
                        };
                        for r in positions {
                            if let Some(ty) = first_private(r, &vars) {
                                report(r.span(), &decl, &ty);
                            }
                        }
                    }
                    // A public constant's annotation is part of what an
                    // importer sees, like a public function's signature.
                    CommonsItem::Const(c) if c.private.is_none() => {
                        if let Some(ty) = first_private(&c.ty, &HashSet::new()) {
                            report(c.ty.span(), &format!("constant `{}`", c.name.name), &ty);
                        }
                    }
                    _ => {}
                }
            }
            for e in parsed[i].exports() {
                for n in &e.names {
                    if private_types.contains(&n.name.name) {
                        report(n.name.span, "`exports` list", &n.name.name);
                    }
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
        // #2618 review: `private` on a `messages` block never does what it
        // suggests, so it warns. Negotiation (`detect_context_message_bundle`,
        // symbols.rs) reads the bundle's visibility off its `@reference`
        // block alone: a private reference keeps the whole bundle out of
        // every context that uses this commons, and `private` on any other
        // block of a bundle whose reference is public changes nothing. The
        // commons' own `render` uses every block either way. Only a commons
        // gets here (a context's or adapter's `messages` block is already
        // `bynk.messages.outside_commons`, above), and only with exactly one
        // `@reference` (a missing or doubled one is already an error).
        if let (Some((_, reference)), [_]) = (reference_block, reference_sites.as_slice()) {
            for &i in indices {
                for item in parsed[i].items() {
                    let CommonsItem::Messages(m) = item else {
                        continue;
                    };
                    let Some(private_span) = m.private else {
                        continue;
                    };
                    let is_reference = std::ptr::eq(m, reference);
                    let (message, note) = if is_reference {
                        (
                            format!(
                                "a `private` messages bundle is never negotiated by a context that uses `{name}`"
                            ),
                            format!(
                                "remove `private` so that a context that uses `{name}` negotiates its locale against this bundle; this commons' own `render` reads the bundle either way"
                            ),
                        )
                    } else if reference.private.is_some() {
                        // The reference block's own warning covers the bundle.
                        continue;
                    } else {
                        (
                            format!(
                                "`private` has no effect on this `messages` block: the bundle's `@reference` block is public, so a context that uses `{name}` still negotiates locale \"{}\"",
                                m.tag
                            ),
                            "remove `private`: a bundle's visibility follows its `@reference` block"
                                .to_string(),
                        )
                    };
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.visibility.private_messages",
                            private_span,
                            message,
                        )
                        .with_note(note),
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
                let Some(locale_ident) = c
                    .selected
                    .iter()
                    .flatten()
                    .map(|i| &i.name)
                    .find(|id| id.name == "Locale")
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
    // Each bound is checked on its own (#990); a range's inverted bounds are
    // the shared range check's `bynk.types.empty_range_pattern` (#2699).
    let (lo, hi) = match &dispatch.pattern {
        SchemaVersionPattern::Literal(n) => (Some(*n), Some(*n)),
        SchemaVersionPattern::Range(r) => (r.lo, r.hi),
        SchemaVersionPattern::Wildcard => return,
    };
    if lo.into_iter().chain(hi).any(|n| n <= 0) {
        errors.push_for(
            Some(identity_path),
            CompileError::new(
                "bynk.event.bad_schema_dispatch",
                dispatch.span,
                "`via schema(...)`'s version bounds must be positive `Int` literals",
            ),
        );
        return;
    }
    if let SchemaVersionPattern::Range(r) = &dispatch.pattern
        && let Some(err) = crate::checker::empty_range_error(r, "`via schema(...)`")
    {
        errors.push_for(Some(identity_path), err);
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
        EventPatternValue::Literal {
            value: lit, span, ..
        } => {
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

/// One deployment unit the platform lock checks (#3562): the contexts that
/// deploy together and the union of their native platforms.
#[derive(Debug, PartialEq, Eq)]
struct LockUnit {
    /// The `[deploy.workers]` group's key, for a unit that is one.
    group: Option<String>,
    /// The contexts in the unit, in the order their `consumes` are looked up.
    members: Vec<String>,
    /// The unit's native platforms, each with the unit that brings it in.
    native: BTreeMap<Platform, String>,
}

/// The deployment units to check, from each context's native platforms:
/// under `workers`, one per `[deploy.workers]` group (the union of its members',
/// since they share a Worker) and one per context in no group; under `bundle`,
/// the union of everything (the whole program co-locates).
fn lock_units(
    target: BuildTarget,
    per_context: Vec<(String, BTreeMap<Platform, String>)>,
    worker_groups: &bynk_project::worker_groups::WorkerGroups,
) -> Vec<LockUnit> {
    let union_of = |natives: Vec<&BTreeMap<Platform, String>>| {
        let mut union = BTreeMap::new();
        for native in natives {
            for (p, unit) in native {
                union.entry(*p).or_insert_with(|| unit.clone());
            }
        }
        union
    };
    match target {
        BuildTarget::Workers => {
            let mut units = Vec::new();
            for (key, group) in worker_groups {
                let found: Vec<_> = per_context
                    .iter()
                    .filter(|(ctx, _)| group.contexts.contains(ctx))
                    .collect();
                let native = union_of(found.iter().map(|(_, n)| n).collect());
                if !native.is_empty() {
                    units.push(LockUnit {
                        group: Some(key.clone()),
                        members: found.iter().map(|(ctx, _)| ctx.clone()).collect(),
                        native,
                    });
                }
            }
            let grouped = |ctx: &str| {
                worker_groups
                    .values()
                    .any(|g| g.contexts.iter().any(|c| c == ctx))
            };
            units.extend(
                per_context
                    .into_iter()
                    .filter(|(ctx, _)| !grouped(ctx))
                    .map(|(ctx, native)| LockUnit {
                        group: None,
                        members: vec![ctx],
                        native,
                    }),
            );
            units
        }
        BuildTarget::Bundle => {
            let native = union_of(per_context.iter().map(|(_, n)| n).collect());
            match per_context.into_iter().next() {
                Some((owner, _)) if !native.is_empty() => vec![LockUnit {
                    group: None,
                    members: vec![owner],
                    native,
                }],
                _ => Vec::new(),
            }
        }
    }
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
    declaring_unit: &str,
    consumed: &[String],
    aliases: &HashMap<String, String>,
    flattened: &HashMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for s in table.services.values() {
        for h in &s.handlers {
            scan_cross_caps(
                &mut out,
                &h.given,
                declaring_unit,
                consumed,
                aliases,
                flattened,
            );
        }
    }
    for a in table.agents.values() {
        for h in &a.handlers {
            scan_cross_caps(
                &mut out,
                &h.given,
                declaring_unit,
                consumed,
                aliases,
                flattened,
            );
        }
    }
    out
}

/// The cross-context capabilities a context's **providers** reference in their
/// own `given`, as `deps_key -> consumed_context` (1B.1b, `ComposePlan`). The
/// Workers `compose.ts` instantiates these at the top level beside the
/// handlers' ([`handler_cross_caps`]); the Bundle root reaches them only
/// through a provider's own recursive instantiation.
pub fn provider_cross_caps(
    table: &UnitTable,
    declaring_unit: &str,
    consumed: &[String],
    aliases: &HashMap<String, String>,
    flattened: &HashMap<String, String>,
) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for p in table.providers.values() {
        scan_cross_caps(
            &mut out,
            &p.given,
            declaring_unit,
            consumed,
            aliases,
            flattened,
        );
    }
    out
}

/// One `given` list's cross-context capabilities, first reference winning.
fn scan_cross_caps(
    out: &mut BTreeMap<String, String>,
    given: &[CapRef],
    declaring_unit: &str,
    consumed: &[String],
    aliases: &HashMap<String, String>,
    flattened: &HashMap<String, String>,
) {
    for c in given {
        // Events track, slice 0 (spine #936): `Events.emit` is
        // intercepted entirely at the call site (release-at-commit
        // buffering) and never calls through a constructed provider —
        // there is no `EventsProvider` for compose to build, so the
        // first-party `Events` must never become a compose deps entry.
        // The canonical first-party rule (1A.33), which also covers a
        // capability declared by `bynk` itself.
        if firstparty::resolve(c.key(), declaring_unit, flattened)
            == Some(firstparty::FirstPartyCap::Events)
        {
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
    for (key, cctx) in handler_cross_caps(table, ctx, &consumed, &aliases, &flattened) {
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
    // #3328 (DECISION I): inbound email is Cloudflare's, whether or not the
    // context reaches a capability — the types alone would not lock it.
    if table
        .services
        .values()
        .any(|s| matches!(s.protocol, ServiceProtocol::Email { .. }))
    {
        referenced.insert(crate::firstparty::CLOUDFLARE_UNIT.to_string());
    }
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
    worker_groups: &bynk_project::worker_groups::WorkerGroups,
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
    let units = lock_units(target, per_context, worker_groups);
    for LockUnit {
        group,
        members,
        native,
    } in units
    {
        let Some(violation) = lock_violation(&native, selected) else {
            continue;
        };
        // `context `a.b``, or, for a `[deploy.workers]` group, `Worker group `key``.
        let ctx = match &group {
            Some(key) => format!("Worker group `{key}`"),
            None => format!("context `{}`", members[0]),
        };
        let span_for = |unit: &str| {
            members
                .iter()
                .find_map(|m| {
                    groups
                        .get(m)
                        .and_then(|idx| consumes_span_of(parsed, idx, unit))
                })
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
                            "{ctx} uses the platform-native capabilities of `{unit}`, which run only on the `{}` platform, but the build selects `--platform {}`",
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
                            "one deployment unit (via {ctx}) uses platform-native capabilities from two mutually-exclusive platforms: `{}` (from `{}`) and `{}` (from `{}`)",
                            a.0.as_str(),
                            a.1,
                            b.0.as_str(),
                            b.1,
                        ),
                    )
                    .with_note(if group.is_some() {
                        "the members of a `[deploy.workers]` group share one Worker: move the conflicting contexts into different groups (or out of any group), or remove one of the platform-native dependencies"
                    } else {
                        "split the consumers into separate deployment units (`--target workers`), or remove one of the platform-native dependencies"
                    }),
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
/// `bynk-driver::project::run_checks` — that call site's own comment claimed
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

/// The project-wide topology checks, in the order their diagnostics appear:
/// #1822 (on Workers, a required local capability needs a provider), #3328 (one
/// service per email address across the whole project, on every target), #3644 (one
/// consuming context per queue) and
/// #2306 (on Workers, a WebSocket `on open` and the agent it transfers to share a
/// file). One call for the build and the editor path, which both run them.
pub fn phase_topology(
    target: BuildTarget,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
) {
    phase_workers_unprovided_capabilities(target, parsed, groups, kinds, unit_tables, errors);
    crate::email::phase_duplicate_addresses(parsed, groups, kinds, errors);
    crate::email_fallback::phase_fallback_claimed(parsed, groups, kinds, errors);
    crate::queue_consumers::phase_duplicate_queue_consumers(parsed, groups, kinds, errors);
    phase_workers_ws_open_split(target, parsed, groups, kinds, unit_tables, errors);
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
    phase_unprovided_capabilities(
        &UnprovidedReason::Workers,
        parsed,
        groups,
        kinds,
        unit_tables,
        errors,
    );
}

/// Why a context's required local capability has to have a provider in it,
/// which is what the `bynk.capability.not_provided` message and note say.
pub enum UnprovidedReason {
    /// #1822: each context is a Worker, which builds every capability its
    /// handlers are given.
    Workers,
    /// A bundle with a composition root: `composeApp` builds every context's
    /// deps itself, so the host has no `__makeSurface(deps)` call to supply a
    /// capability through. `trigger` is the context that brings the root in,
    /// and `because` what it does that needs one (for example "consumes the
    /// services of `shop.stock`"). Without this check the gap reached `tsc`
    /// as a member missing from a `__makeSurface` argument in `compose.ts`,
    /// reported as a compiler bug (`bynk.codegen.tsc_failed`).
    BundleComposition { trigger: String, because: String },
}

/// Whether a bundle's composition root builds this context: `composeApp`
/// makes a deps object only to feed a context's `__makeSurface`, so a context
/// with no services (agents, capabilities or providers only) is left to the
/// host. Shared by the driver's `emit_composition_root` and
/// [`phase_unprovided_capabilities`], so the check refuses exactly the
/// contexts the root builds.
pub fn composed_in_bundle_root(table: &UnitTable) -> bool {
    !table.services.is_empty()
}

/// The shared body of the Workers and bundle-composition checks: each
/// context's required local capability (a `given` on a service or agent
/// handler, or on a provider) that has no provider, once per capability, at
/// its first `given`.
pub fn phase_unprovided_capabilities(
    reason: &UnprovidedReason,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
) {
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context) {
            continue;
        }
        let Some(table) = unit_tables.get(name) else {
            continue;
        };
        if matches!(reason, UnprovidedReason::BundleComposition { .. })
            && !composed_in_bundle_root(table)
        {
            continue;
        }
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
                let cap_name = &cap.name.name;
                let error = match reason {
                    UnprovidedReason::Workers => CompileError::new(
                        "bynk.capability.not_provided",
                        cap.span,
                        format!(
                            "capability `{cap_name}` is required here but has no provider in context `{name}`; a Worker builds every capability its handlers are given"
                        ),
                    )
                    .with_note(format!(
                        "add `provides {cap_name} = …` to `{name}`, or build for `--target bundle`, where the host supplies it"
                    )),
                    UnprovidedReason::BundleComposition { trigger, because } => {
                        CompileError::new(
                            "bynk.capability.not_provided",
                            cap.span,
                            format!(
                                "capability `{cap_name}` is required here but has no provider in context `{name}`; this program's composition root builds every context, so nothing else can supply it"
                            ),
                        )
                        .with_note(format!(
                            "`{trigger}` {because}, so the bundle has a composition root (`composeApp`), which builds `{name}` with the providers it declares"
                        ))
                        .with_note(format!(
                            "add `provides {cap_name} = …` to `{name}`; a test `stub` replaces it in the cases that stub it, but the program still needs one"
                        ))
                    }
                };
                errors.push_for(Some(&parsed[i].identity_path()), error);
            }
        }
    }
}

/// Each unit's resolved calls, by expression: the slice of the checker's
/// [`checker::Callee`] map the composition-root predicate reads. The driver
/// keeps one per unit for emission; [`crate::analysis`] gathers the same
/// slice from the files it checks, so `bynk check` and the editor decide
/// whether a bundle has a composition root from the same facts.
pub type UnitCallees = HashMap<String, HashMap<ExprId, checker::Callee>>;

/// What the whole-program call checks read besides the tables: every unit's
/// kept callees, the units each `uses`, and the project's type interner.
pub type CallFacts<'a> = (
    &'a UnitCallees,
    &'a HashMap<String, Vec<String>>,
    &'a Arc<checker::Types>,
);

/// Whether a resolved call is `Events.emit`, the one callee
/// [`unit_table_uses_emit`] looks for. `Events.emit[...]` dispatches through
/// the ordinary capability-call path (`checker/calls.rs`).
pub fn is_events_emit(callee: &checker::Callee) -> bool {
    matches!(
        callee,
        checker::Callee::Capability {
            op,
            first_party: Some(firstparty::FirstPartyCap::Events),
            ..
        } if op == "emit"
    )
}

/// Events track, slice 0 (spine #936): does any handler or provider op in
/// this unit emit? Reads the checker's own resolved `Callee` for every
/// expression in every service handler, agent handler and provider op, not
/// the receiver's spelling, so a local type that shadows `Events` does not
/// count (`1204_events_emit_shadowed_by_local_type`).
///
/// `callees` is `None` only for a unit whose own check never ran. Every
/// caller runs on an otherwise-clean program, where every unit was checked,
/// and a silent `false` would disable the emitter's events wiring with no
/// diagnostic, so it is a `debug_assert` rather than a quiet default.
pub fn unit_table_uses_emit(
    table: &UnitTable,
    callees: Option<&HashMap<ExprId, checker::Callee>>,
) -> bool {
    let Some(callees) = callees else {
        debug_assert!(
            false,
            "unit_table_uses_emit: no Callee map for a checked unit"
        );
        return false;
    };
    let bodies = table
        .services
        .values()
        .flat_map(|s| s.handlers.iter().map(|h| &h.body))
        .chain(
            table
                .agents
                .values()
                .flat_map(|a| a.handlers.iter().map(|h| &h.body)),
        )
        .chain(
            table
                .providers
                .values()
                .flat_map(|p| p.ops.iter().map(|op| &op.body)),
        );
    let mut stack: Vec<&Expr> = Vec::new();
    for body in bodies {
        for s in &body.statements {
            statement_exprs(s, &mut stack);
        }
        stack.push(&body.tail);
    }
    while let Some(e) = stack.pop() {
        if callees.get(&e.id).is_some_and(is_events_emit) {
            return true;
        }
        stack.extend(expr_children(e));
    }
    false
}

/// The context that makes a bundle emit its composition root (`compose.ts`,
/// `composeApp`) and what it does that needs one, phrased to follow the
/// context's name, or `None` when the bundle has no root. Shared by the
/// driver's `emit_composition_root` and [`phase_bundle_composition_unprovided`]
/// (which both `bynk check` and the editor's analysis run), so the check and
/// the emitter cannot disagree about whether there is a root: `composeApp`
/// builds every context's deps itself, so once there is one the host cannot
/// supply a capability through `__makeSurface(deps)`, and a required local
/// capability needs a provider, as it does on Workers.
pub fn composition_root_trigger(
    kinds: &BTreeMap<String, UnitKind>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_callees: &UnitCallees,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
) -> Option<(String, String)> {
    // Identify contexts that consume something whose surface has services.
    // Sorted, so the context named is the same on every run.
    let mut consumers: Vec<(&String, &Vec<String>)> = unit_consumes.iter().collect();
    consumers.sort();
    for (name, targets) in consumers {
        if kinds.get(name) != Some(&UnitKind::Context) {
            continue;
        }
        if let Some(t) = targets.iter().find(|t| {
            unit_tables
                .get(*t)
                .is_some_and(|other| !other.services.is_empty())
        }) {
            return Some((name.clone(), format!("consumes `{t}`, which has services")));
        }
    }
    // v0.15: also compose when a context uses a consumed context's capability
    // (in a handler or in a provider's `given`) — the consumer must instantiate
    // the provided capability's provider locally.
    for (name, kind) in kinds {
        if *kind != UnitKind::Context {
            continue;
        }
        let Some(table) = unit_tables.get(name) else {
            continue;
        };
        let consumed = unit_consumes.get(name).cloned().unwrap_or_default();
        let aliases = unit_consumes_aliases.get(name).cloned().unwrap_or_default();
        let flattened = unit_flattened.get(name).cloned().unwrap_or_default();
        let uses_cross_cap = !handler_cross_caps(table, name, &consumed, &aliases, &flattened)
            .is_empty()
            || table.providers.values().any(|p| {
                p.given.iter().any(|g| {
                    g.is_cross_context()
                        // v0.18: a bare given flattened from `consumes U
                        // { Cap }` is cross-unit too — its provider lives
                        // in the consumed unit.
                        || (g.prefix().is_none() && flattened.contains_key(g.key()))
                })
            });
        // Events track, slice 0 (spine #936): a context whose handlers emit
        // needs its own `__eventsDispatch` closure built by compose
        // (§ `discover_event_subscribers`) even when it consumes nothing and
        // no other context consumes it — `Events` is filtered out of
        // `handler_cross_caps` (there is no `EventsProvider`), so without
        // this check a publish-only context would never get a compose entry
        // and its service would simply never be called. S4a (#1966): a
        // derived capability's instance is its base's platform provider,
        // built by compose like a flattened one.
        let because = if uses_cross_cap {
            "is given a capability another unit provides"
        } else if unit_table_uses_emit(table, unit_callees.get(name)) {
            "emits events"
        } else if !table.derived.is_empty() {
            "derives a capability"
        } else {
            continue;
        };
        return Some((name.clone(), because.to_string()));
    }
    None
}

/// On the bundle target a context's required local capability may be left to
/// the host, which supplies it through `__makeSurface(deps)`, but only while
/// the bundle has no composition root ([`composition_root_trigger`]):
/// `composeApp` builds the deps of every context it composes
/// ([`composed_in_bundle_root`]), so once a context brings a root in, a
/// capability with no provider is the Workers gap again. Raised at the
/// `given` rather than left to `tsc` against `compose.ts`
/// (`bynk.codegen.tsc_failed`). Called by the driver's `run_checks` and by
/// [`crate::analysis`], after the per-unit checks (it reads `unit_callees`),
/// and only on an otherwise-clean program: a unit that failed to check has no
/// callee map, and the root is only emitted for a clean one.
#[allow(clippy::too_many_arguments)]
pub fn phase_bundle_composition_unprovided(
    target: BuildTarget,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_tables: &HashMap<String, UnitTable>,
    (unit_callees, unit_uses, tys): CallFacts<'_>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    errors: &mut ErrorSink,
) {
    // #3877, #4305: a dead-letter queue's consumer against the source consumer, by the same
    // comparison as a producer's send.
    crate::queue_delivery::phase_dead_letter_consumers(
        parsed,
        groups,
        kinds,
        &crate::queue_producers::Project {
            tables: unit_tables,
            uses: unit_uses,
            consumes: unit_consumes,
        },
        errors,
    );
    // #3899: a signed queue's dead-letter consumer is signed too.
    crate::queue_delivery::phase_dead_letter_signing(parsed, groups, kinds, errors);
    // #3879: a producer's message type against the queue's consumer.
    crate::queue_producers::phase_queue_producers(
        parsed,
        groups,
        kinds,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_callees,
        tys,
        errors,
    );
    // #4441: a `Producer` refinement names only contexts that produce to the queue.
    crate::producer_admission::phase_producer_admission(parsed, groups, kinds, unit_tables, errors);
    // #3890: the whole-program call checks share this entry point, which both
    // pipelines call once the per-unit checks are done: a call the callee's
    // `Caller` refinement does not admit, and an `IsContext(…)` name that is no
    // context.
    crate::caller_admission::phase_caller_admission(
        parsed,
        groups,
        kinds,
        unit_tables,
        unit_callees,
        errors,
    );
    if target != BuildTarget::Bundle || !errors.is_empty() {
        return;
    }
    let Some((trigger, because)) = composition_root_trigger(
        kinds,
        unit_consumes,
        unit_consumes_aliases,
        unit_tables,
        unit_callees,
        unit_flattened,
    ) else {
        return;
    };
    phase_unprovided_capabilities(
        &UnprovidedReason::BundleComposition { trigger, because },
        parsed,
        groups,
        kinds,
        unit_tables,
        errors,
    );
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

/// #2959: on the Workers target, the edge (`compose.ts`) addresses the Durable
/// Object hosting a WebSocket connection *before* the `on open` body runs, so the
/// key of the body's connection transfer must be something it can evaluate on its
/// own: a route parameter, a constant of the context (its own, or a public one
/// of a commons it `uses`, #2967) or a literal
/// ([`crate::websocket::classify_open_key`]). Any other key — a `let`-bound local,
/// a field access, a call — used to be emitted as a name `compose.ts` does not
/// bind (a `tsc` failure) or replaced by the first route parameter (the upgrade
/// silently reached a different instance than the one the body addressed). On the
/// bundle target the open is an ordinary surface method that evaluates any key,
/// so the check is Workers-only, like [`phase_workers_ws_open_split`].
pub fn phase_workers_ws_open_key(
    target: BuildTarget,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
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
        // #2967: the constants visible in the context, a used commons' public
        // ones included, as the emitter classifies the key against.
        let uses = unit_uses.get(name).map(Vec::as_slice).unwrap_or_default();
        let consts = crate::consts::visible_consts(table, uses, |u| unit_tables.get(u));
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
                    let key = crate::websocket::classify_open_key(open, target_agent.key, |n| {
                        consts.contains_key(n)
                    });
                    if !matches!(key, crate::websocket::WsOpenKey::Other) {
                        continue;
                    }
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.ws.open_transfer_key",
                            target_agent.key.span,
                            format!(
                                "the key of this `on open`'s transfer to agent `{}` is not a route parameter, a constant or a literal, so the Workers upgrade cannot address the agent's Durable Object",
                                target_agent.agent
                            ),
                        )
                        .with_note(
                            "on Workers the upgrade is routed to the hosting Durable Object before the `on open` body runs; key the agent by a parameter of `on open`, a `const` of the context or of a commons it `uses`, or a literal",
                        ),
                    );
                }
            }
        }
    }
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

/// #4270: whether the consumed export `type_name` of `target` takes the place of
/// a type a braced `consumes` of a first-party unit left out. Only a selected
/// export of a user unit does, and only from a first-party unit this context
/// consumes: a local declaration, or a first-party type reached any other way
/// (`uses`), still wins over (or conflicts with) it.
fn displaces_unselected_import(
    table: &crate::symbols::UnitTable,
    type_name: &str,
    target: &str,
    imported_from: &HashMap<String, String>,
    consumed: &[String],
    has_local_value: bool,
) -> bool {
    !has_local_value
        && !crate::symbols::is_first_party_name(target)
        && table.unselected_consumed_types.contains(type_name)
        && imported_from.get(type_name).is_some_and(|o| {
            // The occupant is a first-party unit this context consumes (whose
            // braces left the name out), not one it reaches with `uses`.
            crate::symbols::is_first_party_name(o) && consumed.contains(o)
        })
}

/// #2887: the alias the `name_conflict` note suggests for `target`'s export
/// `type_name`: the target's last segment, capitalised, then the type
/// (`bynk.crypto` + `Digest` → `CryptoDigest`).
fn suggest_alias(target: &str, type_name: &str) -> String {
    let last = target.rsplit('.').next().unwrap_or(target);
    let mut chars = last.chars();
    let head: String = chars
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    format!("{head}{}{type_name}", chars.as_str())
}

/// `bynk.consumes.self_reference`: a unit that `consumes` itself.
fn consumes_self_reference(kind: UnitKind, name: &str, span: Span) -> CompileError {
    let kind_word = if kind == UnitKind::Adapter {
        "adapter"
    } else {
        "context"
    };
    CompileError::new(
        "bynk.consumes.self_reference",
        span,
        format!("{kind_word} `{name}` cannot `consumes` itself"),
    )
}

/// `bynk.adapter.consumes_context`: adapter dependencies are adapter-to-adapter
/// (spec §4.5), so an adapter consuming a context would pull service logic into
/// the host boundary.
fn adapter_consumes_context(name: &str, target: &str, span: Span) -> CompileError {
    CompileError::new(
        "bynk.adapter.consumes_context",
        span,
        format!(
            "adapter `{name}` cannot `consumes` the context `{target}` — adapter dependencies are adapter-to-adapter"
        ),
    )
    .with_note(
        "an adapter may only depend on capabilities exported by other adapters (e.g. the `bynk` surface)",
    )
}

/// #4044: why a commons may not write this `consumes`, or `None` when it names
/// only types an adapter exports, in braces. A commons is shared, pure code, so
/// it takes types from another unit and nothing else: no whole-unit or aliased
/// form (those bring services), no capability, and no context (a context's
/// exported types belong to its boundary, and naming one would tie shared code
/// to one deployable). `T as Alias` stays allowed, as in a context's `consumes`.
fn commons_consumes_refusal(
    c: &bynk_syntax::ast::ConsumesDecl,
    target: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
    exports_type: &dyn Fn(&str, &str) -> bool,
) -> Option<CompileError> {
    const CODE: &str = "bynk.commons.consumes_not_types";
    let capabilities = unit_tables.get(target).map(|t| &t.exported_capabilities);
    let refuse = |span: Span, msg: String, note: &str| {
        Some(CompileError::new(CODE, span, msg).with_note(note.to_string()))
    };
    let Some(names) = &c.selected else {
        let form = if c.alias.is_some() {
            "an aliased"
        } else {
            "a whole-unit"
        };
        return refuse(
            c.span,
            format!("a commons cannot write {form} `consumes {target}`"),
            "a commons may list types only: write `consumes U { Type, … }`",
        );
    };
    if target_kind == UnitKind::Context {
        return refuse(
            c.target.span,
            format!("a commons cannot consume the context `{target}`"),
            "a context's exported types belong to its boundary, so a commons that named one would tie shared code to one deployable; a commons consumes types of an adapter",
        );
    }
    for item in names {
        let n = &item.name;
        if capabilities.is_some_and(|caps| caps.contains(&n.name)) {
            // #3959: a first-party unit's capability, contract only, for a
            // function's capability parameters and shared capabilities.
            if crate::firstparty::unit(target).is_some() {
                continue;
            }
            return refuse(
                n.span,
                format!(
                    "`{}` is a capability of `{target}`, which is not a first-party unit",
                    n.name
                ),
                "a commons may list a capability only from a first-party unit (`bynk`, `bynk.cloudflare`, …), for its functions' `given`; from a project's adapter it lists types, and a context uses the adapter's capabilities with `consumes` and `given`",
            );
        }
        if !exports_type(target, &n.name) {
            return refuse(
                n.span,
                format!("`{target}` does not export a type named `{}`", n.name),
                "a commons may list only the types an adapter exports: `consumes U { Type, … }`",
            );
        }
    }
    None
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
        // #2887: the exports of `t` this unit renames (`consumes t { T as U }`),
        // original → alias. An aliased export is in scope under the alias only,
        // and its declaration (and the references to it in `t`'s other exports)
        // take the alias as their name, so it is a type of its own here even
        // when a local `T` exists.
        let renames: HashMap<String, String> = unit_info[name]
            .table
            .consumed_type_aliases
            .iter()
            .filter(|(_, a)| a.target == *t)
            .map(|(alias, a)| (a.original.clone(), alias.clone()))
            .collect();
        for (type_name, vis) in used_exports {
            let Some(decl) = used.types.get(type_name) else {
                continue;
            };
            let original = type_name;
            let aliased = renames.contains_key(original);
            let type_name = renames.get(original).unwrap_or(original);
            let decl = &crate::symbols::rename_consumed_decl(decl, type_name, &renames);
            // #1927: a local value of the export's name (a function, asset,
            // service, agent or provider) takes the emitted module's binding
            // as a type of that name would, so it conflicts the same way.
            let local_value = crate::symbols::local_value_kind(&unit_info[name].table, type_name)
                .map(|(kind, _)| kind)
                .filter(|kind| !matches!(*kind, "type" | "event"));
            // #3536: a type the braces leave out is not in scope as a name of
            // its own, so a declaration of this unit's wins over it, silently.
            // #4270: not when the occupant is itself an unselected first-party
            // import (`bynk`'s `Refusal`): this export is selected, so it wins.
            let displaces_unselected = displaces_unselected_import(
                &unit_info[name].table,
                type_name,
                t,
                imported_from,
                &unit_info[name].consumes,
                local_value.is_some(),
            );
            if displaces_unselected {
                combined_types.remove(type_name);
                combined_methods.remove(type_name);
                consumed_types.remove(type_name);
            } else if unit_info[name]
                .table
                .unselected_consumed_types
                .contains(type_name)
                && (combined_types.contains_key(type_name) || local_value.is_some())
            {
                continue;
            }
            if combined_types.contains_key(type_name) || local_value.is_some() {
                // Name conflict between local/uses and consumed export.
                let consumes_site = consumes_span_of(parsed, &unit_info[name].files, t);
                let consumes_span = consumes_site.map(|(_, s)| s).unwrap_or_default();
                let consumes_file = consumes_site.map(|(i, _)| parsed[i].identity_path());
                let in_scope = match local_value {
                    Some(kind) => format!("this context declares a {kind} of the same name"),
                    // #4270: name the other source when an import holds the name.
                    None => match imported_from.get(type_name) {
                        Some(other) => {
                            format!("a type of the same name is already in scope from `{other}`")
                        }
                        None => "a type of the same name is already in scope".to_string(),
                    },
                };
                // An alias clashing with what is in scope, or a bare export
                // clashing with one of this unit's aliases: the alias is the
                // name to change either way.
                let clashes_alias = !aliased
                    && unit_info[name]
                        .table
                        .consumed_type_aliases
                        .contains_key(type_name);
                if aliased || clashes_alias {
                    let what = if aliased {
                        format!("aliases an export of `{t}` as `{type_name}`, but {in_scope}")
                    } else {
                        format!(
                            "consumes `{t}` which exports type `{type_name}`, but an alias of the same name is in scope"
                        )
                    };
                    errors.push_for(
                        consumes_file.as_deref(),
                        CompileError::new(
                            "bynk.consumes.type_alias_conflict",
                            consumes_span,
                            format!("context `{name}` {what}"),
                        )
                        .with_note("choose an alias no other declaration in this context uses"),
                    );
                    continue;
                }
                errors.push_for(consumes_file.as_deref(),
                    CompileError::new(
                        "bynk.consumes.name_conflict",
                        consumes_span,
                        format!(
                            "context `{name}` consumes `{t}` which exports type `{type_name}`, but {in_scope}",
                        ),
                    )
                    .with_note(format!(
                        "alias it so the two no longer share a name: `consumes {t} {{ …, {type_name} as {t_alias} }}`, then write `{t_alias}` for `{t}`'s type",
                        t_alias = suggest_alias(t, type_name),
                    )),
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
            if let Some(mt) = used.methods.get(original) {
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
    let mut consumed_signatures = consumed_service_seeds(name, unit_info);
    // #2617: a used commons' constant names its annotation's types in the
    // commons' scope, as an imported function's signature does.
    consumed_signatures.extend(used_const_seeds(name, unit_info));
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
    // A `system` harness: also reach what its participants' actors and
    // service handlers name ([`harness_seeds`]).
    harness: bool,
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
    let seeds = if harness {
        harness_seeds(imported, unit_tables)
    } else {
        Vec::new()
    };
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
        seeds,
        |r, owner, bound| {
            // #3760: a name a participant's handler signature reaches but the
            // view already binds receives the value structurally, as for a
            // unit's consumed signatures (#1932), so it is not a conflict.
            if r.signature {
                return;
            }
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

/// The extra seeds of a `system` harness's view, each resolved in its
/// declaring participant's scope:
///
/// - #3064: the `identity` types of the participants' actors;
/// - #3760: the parameter and return types of their service handlers. A case
///   drives those handlers and binds what they return, so a commons type that
///   only a handler's signature names (`-> Effect[HttpResult[Note]]`) must
///   resolve, or the result is untyped and an `is` test on it is unchecked.
///   A signature crosses to the case structurally, as a consumed service's
///   does (#1932), so a name the view already binds is not a conflict.
///
/// Every identity is seeded before any signature. The walk marks a commons
/// type visited however it was first reached, so a signature reach that came
/// first would swallow the conflict an identity reaching the same type
/// reports (review of #3835).
fn harness_seeds(imported: &[String], unit_tables: &HashMap<String, UnitTable>) -> Vec<Reached> {
    let mut seeds = Vec::new();
    let tables = || {
        imported
            .iter()
            .filter_map(|u| Some((u, unit_tables.get(u)?)))
    };
    for (u, t) in tables() {
        let mut actors: Vec<_> = t.actors.iter().collect();
        actors.sort_by_key(|(n, _)| n.as_str());
        for (_, a) in actors {
            if let Some(id) = &a.identity {
                let mut names = Vec::new();
                type_ref_names(id, &HashSet::new(), &mut names);
                push_refs(names, u, u, &mut seeds);
            }
        }
    }
    for (u, t) in tables() {
        let mut services: Vec<&ServiceDecl> = t.services.values().collect();
        services.sort_by(|a, b| a.name.name.cmp(&b.name.name));
        for h in services.into_iter().flat_map(|s| &s.handlers) {
            let mut names = Vec::new();
            for p in &h.params {
                type_ref_names(&p.type_ref, &HashSet::new(), &mut names);
            }
            type_ref_names(&h.return_type, &HashSet::new(), &mut names);
            push_refs_with(names, u, u, true, &mut seeds);
        }
    }
    seeds
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

/// #2617: the type names the constants of every commons `name` uses name in
/// their annotations, resolved in that commons' scope, in a stable order. A
/// constant the unit hides behind a local declaration, or one its commons
/// marks `private`, is not imported, so it reaches nothing.
fn used_const_seeds(name: &str, unit_info: &BTreeMap<String, UnitInfo>) -> Vec<Reached> {
    let mut out = Vec::new();
    let Some(info) = unit_info.get(name) else {
        return out;
    };
    for t in &info.uses {
        let Some(used) = unit_info.get(t) else {
            continue;
        };
        let mut consts: Vec<&ConstDecl> = used
            .table
            .consts
            .values()
            .filter(|c| c.private.is_none())
            .filter(|c| crate::symbols::local_value_kind(&info.table, &c.name.name).is_none())
            .collect();
        consts.sort_by(|a, b| a.name.name.cmp(&b.name.name));
        for c in consts {
            let mut names = Vec::new();
            type_ref_names(&c.ty, &HashSet::new(), &mut names);
            push_refs(names, t, t, &mut out);
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
    /// The reaches in key order, so a rendering of them is the same for two equal
    /// values however their `HashMap`s iterate (the language server hashes it).
    pub(crate) fn sorted(&self) -> Vec<(&(String, String), &ShadowedType)> {
        let mut v: Vec<_> = self.0.iter().collect();
        v.sort_by(|a, b| a.0.cmp(b.0));
        v
    }

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
        | TypeRef::List(t, _)
        | TypeRef::Set(t, _)
        | TypeRef::Query(t, _)
        | TypeRef::Stream(t, _)
        | TypeRef::Connection(t, _)
        | TypeRef::History(t, _) => type_ref_names(t, vars, out),
        TypeRef::HttpResult(t, e, _) => {
            type_ref_names(t, vars, out);
            if let Some(e) = e {
                type_ref_names(e, vars, out);
            }
        }
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
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
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
        // #2618: an importer never sees a used unit's `private` declarations.
        let used = unit_info
            .get(t)
            .expect("used unit present")
            .table
            .importable();
        let used = local_table.seen_through(t, &used);
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
        // #2387, slice 2: likewise a used commons' embeds.
        for embed_name in used.embeds.keys() {
            if crate::symbols::local_value_kind(local_table, embed_name).is_none()
                && !combined_fns.contains_key(embed_name)
                && !imported_from.contains_key(embed_name)
            {
                imported_from.insert(embed_name.clone(), t.clone());
                imported_from_kind.insert(embed_name.clone(), UnitKind::Commons);
            }
        }
        // #2617: and its constants.
        for const_name in used.consts.keys() {
            if crate::symbols::local_value_kind(local_table, const_name).is_none()
                && !combined_fns.contains_key(const_name)
                && !imported_from.contains_key(const_name)
            {
                imported_from.insert(const_name.clone(), t.clone());
                imported_from_kind.insert(const_name.clone(), UnitKind::Commons);
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
    let mut out = shadowed_imports_in(
        &info.table,
        info.uses.iter().filter_map(|t| {
            unit_info
                .get(t)
                .map(|u| (t, info.table.seen_through(t, &u.table)))
        }),
    );
    crate::uses_view::add_ambiguous_imports(&info.table, &mut out);
    out
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
        // #2618: a `private` declaration is never imported, so it hides nothing.
        let used = used.importable();
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
                    ambiguous: Vec::new(),
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
    // #4044: a commons may `consumes` an adapter that `uses` it, so the cycle
    // check walks each unit's `uses` edges too.
    let mut unit_uses: HashMap<String, Vec<String>> = HashMap::new();
    for (name, indices) in groups {
        for &i in indices {
            unit_uses
                .entry(name.clone())
                .or_default()
                .extend(parsed[i].uses().iter().map(|u| u.target.joined()));
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
    detect_dependency_cycles(
        unit_consumes,
        &unit_uses,
        &consumes_sites,
        &mut cycle_errors,
    );
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
            bound: None,
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
            exec_plan: Default::default(),
            kind: None,
            test_only: false,
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
            consumed_type_aliases: StdHashMap::new(),
            unselected_consumed_types: Default::default(),
            unlisted_consumed_types: Default::default(),
            assets: StdHashMap::new(),
            embeds: StdHashMap::new(),
            consts: StdHashMap::new(),
            derived: StdHashMap::new(),
            use_views: Default::default(),
            ambiguous_uses: Default::default(),
            use_origin: Default::default(),
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
    /// must still be recognised as native. `bynk-tests/tests/fixtures/negative/
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
            false,
        );
        assert_eq!(
            reach.hidden.get("Repo").map(String::as_str),
            Some("t.core"),
            "{:?}",
            reach.hidden
        );
        assert!(types.contains_key("Repo"));
    }

    /// Review of #3835: a participant's handler signature reaches a commons
    /// type without a conflict (#3760), but that reach must not claim the
    /// type first and so swallow the conflict another participant's actor
    /// identity reports for it (#3064). The walk marks a commons type visited
    /// however it was reached, so identities are seeded before signatures.
    #[test]
    fn a_signature_reach_does_not_swallow_an_identity_conflict() {
        let lib = table(
            "t.lib",
            UnitKind::Commons,
            "commons t.lib\n\ntype X = String where NonEmpty\n\ntype Wrap = { x: X }\n\ntype Id = { x: X }\n",
        );
        let a = table(
            "t.a",
            UnitKind::Context,
            "context t.a\n\nuses t.lib\n\nservice s {\n  on call() -> Effect[Wrap] by Caller { Effect.pure(Wrap { x: X.unsafe(\"x\") }) }\n}\n",
        );
        let b = table(
            "t.b",
            UnitKind::Context,
            "context t.b\n\nuses t.lib\n\nactor U { auth = Internal, identity = Id }\n",
        );
        // The suite's own `X`, a different declaration from `t.lib`'s.
        let mine = table(
            "t.mine",
            UnitKind::Commons,
            "commons t.mine\n\ntype X = Int\n",
        );
        let mut types = mine.types.clone();
        let tables: HashMap<String, UnitTable> = [
            ("t.lib".to_string(), lib),
            ("t.a".to_string(), a),
            ("t.b".to_string(), b),
            ("t.mine".to_string(), mine),
        ]
        .into();
        let uses: HashMap<String, Vec<String>> = [
            ("t.a".to_string(), vec!["t.lib".to_string()]),
            ("t.b".to_string(), vec!["t.lib".to_string()]),
        ]
        .into();
        let reach = close_view_types(
            &["t.a".to_string(), "t.b".to_string()],
            &tables,
            &uses,
            &mut types,
            &HashMap::new(),
            &mut HashMap::new(),
            true,
        );
        assert!(
            reach
                .conflicts
                .iter()
                .any(|c| c.name == "X" && c.via == "t.b"),
            "{:?}",
            reach.conflicts
        );
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

    /// #3243: attributing against files registered once up front — what the
    /// per-unit check loops do — puts an error in the file its span names, and
    /// leaves one whose span matches no registered file unattributed.
    #[test]
    fn extend_attributed_uses_the_files_registered_up_front() {
        let known = FileId(40);
        let mut sink = ErrorSink::new();
        sink.register_file(known, PathBuf::from("src/a.bynk"));
        sink.extend_attributed([CompileError::new(
            "bynk.resolve.duplicate_service",
            Span::new_in(known, 0, 1),
            "in a registered file",
        )]);
        sink.extend_attributed([CompileError::new(
            "bynk.resolve.duplicate_service",
            Span::new_in(FileId(41), 0, 1),
            "in no registered file",
        )]);
        let all = sink.into_all();
        assert_eq!(all[0].source_path.as_deref(), Some(Path::new("src/a.bynk")));
        assert_eq!(all[1].source_path, None);
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
            "service thing {\n  on call() -> Effect[Int] by Caller {\n    Effect.pure(1)\n  }\n}\n",
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
                            "service user {{\n  on call() -> Effect[{ty}] by Caller {{\n    Effect.pure({expr})\n  }}\n}}\n"
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
                "context app\n\nuses lib.util\n\nagent thing {\n  key id: String\n\n  store hits: Cell[Int]\n\n  on call ping() -> Effect[Int] {\n    1\n  }\n}\n\nservice web {\n  on call() -> Effect[Int] by Caller {\n    let n <- thing(\"k\").ping()\n    Effect.pure(n)\n  }\n}\n",
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
                "context app\n\nuses lib.a\nuses lib.b\n\nservice thing {\n  on call() -> Effect[Int] by Caller {\n    Effect.pure(1)\n  }\n}\n",
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
                "context app\n\nuses {first}\nuses {second}\n\nservice Status {{\n  on call() -> Effect[Int] by Caller {{\n    Effect.pure(1)\n  }}\n}}\n\nfn f(s: Status) -> Int {{\n  s.n\n}}\n"
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
                "context app\n\nconsumes shop\n\nservice Order {\n  on call() -> Effect[Int] by Caller {\n    Effect.pure(1)\n  }\n}\n",
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

    const WITHIN_SVC: &str = "\nservice s {\n  on call() -> Effect[String] by Caller {\n    let r <- Effect.pure(1).within(2.seconds)\n    Effect.pure(\"x\")\n  }\n}\n";

    /// #2765: the braces may name a type the target exports, so the form the
    /// `timed_out_not_in_scope` note recommends is accepted.
    #[test]
    fn consumes_bynk_may_select_an_exported_type() {
        for consumes in [
            "consumes bynk { TimedOut }",
            "consumes bynk { Clock, TimedOut }",
        ] {
            let src = format!("context app\n\n{consumes}\n{WITHIN_SVC}");
            let r = analyse(&[("app.bynk", &src)]);
            assert!(
                r.error_categories().is_empty(),
                "{consumes}: {:?}",
                r.error_categories()
            );
        }
    }

    /// A type repeated in one file is refused, as a repeated capability is.
    #[test]
    fn a_repeated_type_selection_is_refused() {
        let src = format!("context app\n\nconsumes bynk {{ TimedOut, TimedOut }}\n{WITHIN_SVC}");
        let r = analyse(&[("app.bynk", &src)]);
        assert_eq!(
            r.error_categories(),
            vec!["bynk.consumes.capability_name_clash"]
        );
    }

    /// A braced name that is neither an exported capability nor an exported
    /// type is still refused.
    #[test]
    fn consumes_bynk_still_refuses_an_unknown_name() {
        let r = analyse(&[("app.bynk", "context app\n\nconsumes bynk { NoSuchThing }\n")]);
        assert_eq!(
            r.error_categories(),
            vec!["bynk.given.cross_context_unknown_capability"]
        );
    }

    /// #4270: a type the file selects from a consumed context keeps its name
    /// when `bynk` exports one of the same (`Refusal`) that the braces leave out.
    const ADMISSION: &str = "context registry.admission\n\nexports transparent { Refusal }\n\ntype Refusal = { code: String, message: String }\n";
    const USE_REFUSAL: &str = "fn describe(r: Refusal) -> String {\n  r.code\n}\n";

    #[test]
    fn a_selected_consumed_type_wins_over_an_unselected_bynk_export() {
        for consumes in [
            "consumes bynk { Clock }\nconsumes registry.admission { Refusal }",
            "consumes registry.admission { Refusal }\nconsumes bynk { Clock }",
            "consumes registry.admission { Refusal }",
        ] {
            let store = format!("context registry.store\n\n{consumes}\n\n{USE_REFUSAL}");
            analyse(&[
                ("registry/admission.bynk", ADMISSION),
                ("registry/store.bynk", &store),
            ])
            .assert_clean();
        }
    }

    #[test]
    fn selecting_a_name_from_bynk_and_a_consumed_context_is_a_name_conflict_naming_both() {
        let store = format!(
            "context registry.store\n\nconsumes bynk {{ Refusal }}\nconsumes registry.admission {{ Refusal }}\n\n{USE_REFUSAL}"
        );
        let r = analyse(&[
            ("registry/admission.bynk", ADMISSION),
            ("registry/store.bynk", &store),
        ]);
        let e = r
            .analysis
            .errors
            .iter()
            .find(|e| e.error.category == "bynk.consumes.name_conflict")
            .expect("both selections claim `Refusal`");
        assert!(
            e.error.message.contains("registry.admission"),
            "{}",
            e.error.message
        );
        assert!(
            e.error.message.contains("from `bynk`"),
            "{}",
            e.error.message
        );
    }

    /// The note recommends only forms that work.
    #[test]
    fn timed_out_not_in_scope_note_names_the_accepted_form() {
        let src = format!("context app\n{WITHIN_SVC}");
        let r = analyse(&[("app.bynk", &src)]);
        let e = r
            .analysis
            .errors
            .iter()
            .find(|e| e.error.category == "bynk.within.timed_out_not_in_scope")
            .expect("TimedOut is not in scope without `consumes bynk`");
        assert!(
            e.error
                .notes
                .iter()
                .any(|n| n.contains("consumes bynk { TimedOut }")),
            "{:?}",
            e.error.notes
        );
    }
}

/// #2887: `consumes U { T as Alias }` renames an exported type in the consuming
/// context, so it no longer clashes with a local or `uses` type of that name.
#[cfg(test)]
mod consumes_alias_tests {
    use crate::stability::StabilityPolicy;
    use crate::testkit::{Analysed, analyse_with};

    const REPORT: (&str, &str) = (
        "report.bynk",
        "commons report\n\ntype Digest = { total: Int }\n",
    );

    fn run(files: &[(&str, &str)]) -> Analysed {
        analyse_with(files, Some(StabilityPolicy::permissive()))
    }

    fn app(consumes: &str, body: &str) -> String {
        format!("context app\n\nuses report\n{consumes}\n\n{body}\n")
    }

    const USES_BOTH: &str = "fn empty() -> Digest { Digest { total: 0 } }\n\
        fn name(d: CryptoDigest) -> String {\n  match d {\n    Sha256 => \"a\"\n    Sha384 => \"b\"\n    Sha512 => \"c\"\n  }\n}\n\
        service hashing {\n  on call() -> Effect[Bytes] by Caller given Crypto {\n    Crypto.digest(CryptoDigest.Sha256, Bytes.fromUtf8(\"x\"))\n  }\n}";

    #[test]
    fn an_aliased_export_clears_the_clash_and_works_everywhere() {
        let src = app(
            "consumes bynk.crypto { Crypto, Digest as CryptoDigest }",
            USES_BOTH,
        );
        let a = run(&[REPORT, ("app.bynk", &src)]);
        a.assert_clean();
        // The unaliased `Digest` is still the commons record.
        assert_eq!(a.resolves_to("app.bynk", "Digest {").unit, "report");
    }

    #[test]
    fn an_alias_names_the_real_declaration_for_the_ide() {
        // A user adapter as the target, so its declarations are in the index
        // (a first-party unit's references are tokens only).
        let lib = (
            "lib.bynk",
            "adapter lib\n\nbinding \"./lib.binding.ts\"\n\nexports transparent { Digest }\n\ntype Digest = enum { Fast, Slow }\n",
        );
        let src = app(
            "consumes lib { Digest as LibDigest }",
            "fn pick() -> LibDigest { LibDigest.Fast }\nfn empty() -> Digest { Digest { total: 0 } }",
        );
        let a = run(&[
            REPORT,
            lib,
            ("lib.binding.ts", "export {};\n"),
            ("app.bynk", &src),
        ]);
        a.assert_clean();
        for needle in ["Digest as", "LibDigest {", "LibDigest.Fast"] {
            let key = a.resolves_to("app.bynk", needle);
            assert_eq!(
                (key.unit.as_str(), key.name.as_str()),
                ("lib", "Digest"),
                "{needle}"
            );
        }
        assert_eq!(a.resolves_to("app.bynk", "Digest { total").unit, "report");
    }

    /// #3536: braces that list only `Clock` do not bring `Method` in, and no
    /// listed capability's signature reaches it, so the unit's own `Method` is
    /// not in conflict with the standard library's.
    #[test]
    fn an_unlisted_export_yields_to_the_units_own_type() {
        let src = app(
            "consumes bynk { Clock }",
            "type Method = | Fetch | Skip\n\nfn pick() -> Method { Fetch }",
        );
        run(&[REPORT, ("app.bynk", &src)]).assert_clean();
    }

    /// A type a listed capability's signatures use is still the standard
    /// library's, so the unit's own type of that name still conflicts, with a
    /// note that names the way out.
    #[test]
    fn the_unaliased_form_still_clashes_with_a_note_that_names_the_way_out() {
        let src = app(
            "consumes bynk.crypto { Crypto }",
            "fn empty() -> Digest { Digest { total: 0 } }",
        );
        let a = run(&[REPORT, ("app.bynk", &src)]);
        assert_eq!(a.error_categories(), vec!["bynk.consumes.name_conflict"]);
        let e = &a.analysis.errors[0].error;
        assert!(
            e.notes.iter().any(|n| n.contains("Digest as CryptoDigest")),
            "{:?}",
            e.notes
        );
    }

    #[test]
    fn a_refused_alias_does_not_take_over_the_name_it_clashed_with() {
        // `Report` is the local type; the clashing alias is refused and must not
        // redirect hover or go-to-definition to the module's `Digest`.
        let src = "context app\n\nconsumes bynk.crypto { Crypto, Digest as Report }\n\ntype Report = { total: Int }\n\nfn f(r: Report) -> Int { r.total }\n";
        let a = run(&[("app.bynk", src)]);
        assert_eq!(
            a.error_categories(),
            vec!["bynk.consumes.type_alias_conflict"]
        );
        // The unit does not check past the refusal, so the proof is that the
        // index neither panics nor files the alias under the module's type:
        // the local `Report` stays the one declaration of that name.
        let reports: Vec<_> = a
            .analysis
            .index
            .symbols
            .keys()
            .filter(|k| k.name == "Report" || k.name == "Digest")
            .collect();
        assert!(
            reports
                .iter()
                .all(|k| k.unit == "app" && k.name == "Report"),
            "{reports:?}"
        );
    }

    #[test]
    fn the_conflict_note_for_a_context_also_names_the_alias_form() {
        // An alias works for a context's type as for an adapter's, so the note
        // may recommend it there.
        let lib = (
            "lib.bynk",
            "context lib\n\nexports transparent { Code }\n\ntype Code = enum { A, B }\n",
        );
        let codes = ("codes.bynk", "commons codes\n\ntype Code = { n: Int }\n");
        let app = "context app\n\nuses codes\nconsumes lib\n\nfn one() -> Int { 1 }\n";
        let a = run(&[lib, codes, ("app.bynk", app)]);
        assert_eq!(a.error_categories(), vec!["bynk.consumes.name_conflict"]);
        assert!(
            a.analysis.errors[0]
                .error
                .notes
                .iter()
                .any(|n| n.contains("Code as LibCode")),
            "{:?}",
            a.analysis.errors[0].error.notes
        );
    }

    #[test]
    fn the_original_name_is_not_in_scope_once_aliased() {
        let src = app(
            "consumes bynk.crypto { Crypto, Digest as CryptoDigest }",
            "fn f(d: Digest) -> Int { 1 }",
        );
        // `Digest` is the commons record here; handing it where the module's
        // enum is expected is a type error, not a silent conflation.
        let a = run(&[REPORT, ("app.bynk", &src)]);
        a.assert_clean();
        let bad = app(
            "consumes bynk.crypto { Crypto, Digest as CryptoDigest }",
            "service s {\n  on call() -> Effect[Bytes] by Caller given Crypto {\n    Crypto.digest(Digest { total: 1 }, Bytes.fromUtf8(\"x\"))\n  }\n}",
        );
        let a = run(&[REPORT, ("app.bynk", &bad)]);
        assert!(
            !a.error_categories().is_empty(),
            "the local Digest is not the crypto Digest"
        );
    }

    #[test]
    fn an_alias_in_one_file_of_a_split_context_applies_to_the_unit() {
        let one = "context app\n\nuses report\nconsumes bynk.crypto { Crypto, Digest as CryptoDigest }\n\nfn empty() -> Digest { Digest { total: 0 } }\n";
        let two = "context app\n\nuses report\nconsumes bynk.crypto { Crypto, Digest as CryptoDigest }\n\nfn first() -> CryptoDigest { CryptoDigest.Sha256 }\n";
        run(&[REPORT, ("app/one.bynk", one), ("app/two.bynk", two)]).assert_clean();
        let disagree =
            "context app\n\nuses report\nconsumes bynk.crypto { Crypto, Digest as Other }\n";
        let a = run(&[REPORT, ("app/one.bynk", one), ("app/two.bynk", disagree)]);
        assert!(
            a.error_categories()
                .contains(&"bynk.consumes.type_alias_repeated"),
            "{:?}",
            a.error_categories()
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
        add(list, "bynk.list");
        add(string, "bynk.string");
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

    /// A consumer resolves an exported capability's operations against the
    /// unit's exported types only, so an operation naming a type the unit
    /// declares but does not export types as `()` there, with no diagnostic
    /// at the cause (#2634). Every local type an exported capability's
    /// signatures name is therefore in an `exports` clause, as is every one a
    /// transparent export reached that way carries in a field or variant.
    #[test]
    fn exported_capability_signatures_name_only_exported_types() {
        use bynk_syntax::ast::{
            CommonsItem, ExportKind, RecordField, SourceUnit, TypeBody, Visibility,
        };
        use std::collections::{HashMap, HashSet};
        let mut missing = Vec::new();
        for unit in firstparty::UNITS
            .iter()
            .filter(|u| u.kind == firstparty::UnitKind::Adapter)
        {
            let toks = lexer::tokenize(unit.source).expect("lexes");
            let (parsed, _) = parser::parse_unit_with_warnings_from(&toks, unit.source, &mut { 0 })
                .expect("parses");
            let SourceUnit::Adapter(adapter) = parsed else {
                panic!("{} is registered as an adapter", unit.name);
            };
            let exported = |kind: fn(&ExportKind) -> bool| -> HashSet<&str> {
                adapter
                    .exports
                    .iter()
                    .filter(|c| kind(&c.kind))
                    .flat_map(|c| c.names.iter().map(|e| e.name.name.as_str()))
                    .collect()
            };
            let types = exported(|k| matches!(k, ExportKind::Type(_)));
            let transparent = exported(|k| matches!(k, ExportKind::Type(Visibility::Transparent)));
            let capabilities = exported(|k| matches!(k, ExportKind::Capability));
            // Each local type with the names its fields and variants carry:
            // a consumer reads those through a transparent export, so they
            // must resolve there too.
            let fields = |fs: &[RecordField], vars: &HashSet<&str>| {
                let mut out = Vec::new();
                for f in fs {
                    super::type_ref_names(&f.type_ref, vars, &mut out);
                }
                out
            };
            let local: HashMap<&str, Vec<String>> = adapter
                .items
                .iter()
                .filter_map(|item| match item {
                    CommonsItem::Type(t) => {
                        let vars = t.type_params.iter().map(|p| p.name.name.as_str()).collect();
                        let inner = match &t.body {
                            TypeBody::Record(r) => fields(&r.fields, &vars),
                            TypeBody::Sum(s) => {
                                let mut out = Vec::new();
                                for f in s.variants.iter().flat_map(|v| &v.payload) {
                                    super::type_ref_names(&f.type_ref, &vars, &mut out);
                                }
                                for e in &s.embeds {
                                    super::type_ref_names(&e.source_type, &vars, &mut out);
                                }
                                out
                            }
                            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => Vec::new(),
                        };
                        Some((t.name.name.as_str(), inner))
                    }
                    CommonsItem::Event(e) => Some((
                        e.name.name.as_str(),
                        fields(&e.body.fields, &HashSet::new()),
                    )),
                    _ => None,
                })
                .collect();
            for item in &adapter.items {
                let CommonsItem::Capability(cap) = item else {
                    continue;
                };
                if !capabilities.contains(cap.name.name.as_str()) {
                    continue;
                }
                for op in &cap.ops {
                    let vars = op
                        .type_params
                        .iter()
                        .map(|p| p.name.name.as_str())
                        .collect();
                    let mut names = Vec::new();
                    for p in &op.params {
                        super::type_ref_names(&p.type_ref, &vars, &mut names);
                    }
                    super::type_ref_names(&op.return_type, &vars, &mut names);
                    // Walk on through transparent exports, keeping the path
                    // (`Fetch.send` → `Response` → `Body`) for the message.
                    let mut work: Vec<(String, String)> = names
                        .into_iter()
                        .map(|n| {
                            (
                                n.clone(),
                                format!("`{}.{}` → `{n}`", cap.name.name, op.name.name),
                            )
                        })
                        .collect();
                    let mut seen = HashSet::new();
                    while let Some((name, path)) = work.pop() {
                        let Some(inner) = local.get(name.as_str()) else {
                            continue;
                        };
                        if !seen.insert(name.clone()) {
                            continue;
                        }
                        if !types.contains(name.as_str()) {
                            missing.push(format!(
                                "{}: {path}, which is not in an `exports` clause",
                                unit.name
                            ));
                        } else if transparent.contains(name.as_str()) {
                            work.extend(
                                inner.iter().map(|n| (n.clone(), format!("{path} → `{n}`"))),
                            );
                        }
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "{}", missing.join("\n"));
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

#[cfg(test)]
mod metadata_property_tests {
    use super::is_metadata_property;

    #[test]
    fn a_dot_nests_and_each_segment_is_checked() {
        assert!(is_metadata_property("category"));
        assert!(is_metadata_property("author.name"));
        assert!(!is_metadata_property(""));
        assert!(!is_metadata_property("a..b"));
        assert!(!is_metadata_property(".a"));
        assert!(!is_metadata_property("a."));
        assert!(!is_metadata_property("$x"));
        assert!(!is_metadata_property("a.$x"));
        assert!(!is_metadata_property("a\"b"));
        assert!(!is_metadata_property(&"x".repeat(513)));
    }
}

/// The deployment units the platform lock checks (#3562): per context under
/// `workers`, a `[deploy.workers]` group's members together, the whole program
/// under `bundle`.
#[cfg(test)]
mod lock_unit_tests {
    use super::*;
    use bynk_project::worker_groups::{WorkerGroup, WorkerGroups};

    fn native(platform: Platform, unit: &str) -> BTreeMap<Platform, String> {
        BTreeMap::from([(platform, unit.to_string())])
    }

    fn per_context() -> Vec<(String, BTreeMap<Platform, String>)> {
        vec![
            (
                "a.x".to_string(),
                native(Platform::Cloudflare, "bynk.cloudflare"),
            ),
            ("a.y".to_string(), native(Platform::Node, "bynk.node")),
            (
                "a.z".to_string(),
                native(Platform::Cloudflare, "bynk.cloudflare"),
            ),
        ]
    }

    fn group(members: &[&str]) -> WorkerGroups {
        WorkerGroups::from([(
            "core".to_string(),
            WorkerGroup {
                contexts: members.iter().map(|m| m.to_string()).collect(),
                ..WorkerGroup::default()
            },
        )])
    }

    /// Without groups, each context is its own unit, as before.
    #[test]
    fn workers_without_groups_is_one_unit_per_context() {
        let units = lock_units(BuildTarget::Workers, per_context(), &WorkerGroups::new());
        assert_eq!(units.len(), 3);
        assert!(
            units
                .iter()
                .all(|u| u.group.is_none() && u.members.len() == 1)
        );
        assert!(units.iter().all(
            |u| lock_violation(&u.native, Platform::Cloudflare).is_some()
                == (u.members[0] == "a.y")
        ));
    }

    /// A group's members share a Worker, so a Cloudflare-native member and a
    /// Node-native one are one unit spanning two platforms.
    #[test]
    fn a_group_is_one_unit_over_its_members_union() {
        let units = lock_units(BuildTarget::Workers, per_context(), &group(&["a.x", "a.y"]));
        assert_eq!(units.len(), 2, "{units:?}");
        let core = &units[0];
        assert_eq!(core.group.as_deref(), Some("core"));
        assert_eq!(core.members, ["a.x", "a.y"]);
        assert!(matches!(
            lock_violation(&core.native, Platform::Cloudflare),
            Some(LockViolation::Conflict { .. })
        ));
        // `a.z`, in no group, stays its own unit, and is fine on Cloudflare.
        assert_eq!(units[1].members, ["a.z"]);
        assert!(lock_violation(&units[1].native, Platform::Cloudflare).is_none());
    }

    /// A group whose members need no native platform makes no unit.
    #[test]
    fn a_group_with_no_native_platforms_is_not_a_unit() {
        let units = lock_units(
            BuildTarget::Workers,
            vec![("a.x".to_string(), BTreeMap::new())],
            &group(&["a.x"]),
        );
        assert!(units.is_empty());
    }

    /// Under `bundle` the whole program is one unit, owned by the first context,
    /// whatever the groups say.
    #[test]
    fn bundle_is_one_unit_and_ignores_groups() {
        let units = lock_units(BuildTarget::Bundle, per_context(), &group(&["a.x", "a.y"]));
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].group, None);
        assert_eq!(units[0].members, ["a.x"]);
        assert_eq!(units[0].native.len(), 2);
    }
}

#[cfg(test)]
mod cross_caps_first_party_tests {
    //! The first-party `Events` skip in `scan_cross_caps` is
    //! `firstparty::resolve`, the single definition of bynk's own capabilities
    //! (1A.33): a capability flattened from `bynk`, or declared by `bynk`
    //! itself, never becomes a compose deps entry.
    use super::*;
    use bynk_syntax::ast::{CapRef, Ident, ProviderDecl};
    use bynk_syntax::span::Span;

    fn ident(name: &str) -> Ident {
        Ident {
            name: name.to_string(),
            span: Span::default(),
        }
    }

    fn bare(name: &str) -> CapRef {
        CapRef {
            context: None,
            name: ident(name),
            bound: None,
            span: Span::default(),
        }
    }

    fn flattened_from(unit: &str, caps: &[&str]) -> HashMap<String, String> {
        caps.iter()
            .map(|c| (c.to_string(), unit.to_string()))
            .collect()
    }

    fn scan(declaring_unit: &str, flattened: &HashMap<String, String>) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        scan_cross_caps(
            &mut out,
            &[bare("Events"), bare("Ledger")],
            declaring_unit,
            &[],
            &HashMap::new(),
            flattened,
        );
        out
    }

    #[test]
    fn a_flattened_bynk_events_is_skipped_and_other_flattened_caps_are_kept() {
        let flattened = flattened_from("bynk", &["Events"]);
        let mut flattened = flattened;
        flattened.insert("Ledger".to_string(), "bank".to_string());
        let got = scan("shop", &flattened);
        assert_eq!(got.keys().collect::<Vec<_>>(), ["Ledger"]);
    }

    #[test]
    fn an_events_declared_by_bynk_itself_is_skipped_without_being_flattened() {
        let mut flattened = HashMap::new();
        flattened.insert("Ledger".to_string(), "bank".to_string());
        let got = scan("bynk", &flattened);
        assert_eq!(got.keys().collect::<Vec<_>>(), ["Ledger"]);
    }

    #[test]
    fn an_events_flattened_from_another_unit_is_not_first_party() {
        let flattened = flattened_from("other", &["Events", "Ledger"]);
        let got = scan("shop", &flattened);
        assert_eq!(got.keys().collect::<Vec<_>>(), ["Events", "Ledger"]);
    }

    #[test]
    fn a_providers_given_skips_a_flattened_bynk_events_too() {
        let mut table = UnitTable::default();
        table.providers.insert(
            "Billing".to_string(),
            ProviderDecl {
                capability: ident("Billing"),
                provider_name: ident("BillingImpl"),
                given: vec![bare("Events"), bare("Ledger")],
                ops: Vec::new(),
                external: false,
                documentation: None,
                span: Span::default(),
                trivia: Default::default(),
            },
        );
        let mut flattened = flattened_from("bynk", &["Events"]);
        flattened.insert("Ledger".to_string(), "bank".to_string());
        let got = provider_cross_caps(&table, "shop", &[], &HashMap::new(), &flattened);
        assert_eq!(got.keys().collect::<Vec<_>>(), ["Ledger"]);
    }
}
