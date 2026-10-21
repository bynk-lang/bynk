use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use bynk_syntax::ast::{
    AdapterDecl, Case, Commons, CommonsItem, ConsumesDecl, ExportsDecl, SourceUnit, SuiteDecl,
    TestTier, Trivia, TypeRef, UsesDecl,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::roots::{Roots, UnitKind};

/// v0.118: a case's *effective* tier — its own `as <tier>`, else the suite
/// default, else `unit`.
pub fn case_effective_tier(case: &Case, suite: &SuiteDecl) -> TestTier {
    case.tier.or(suite.tier).unwrap_or(TestTier::Unit)
}

/// v0.118: whether a suite's *effective* tier is `system` — the suite default
/// is `system`, or any case opts up to `system`. Such a suite is emitted via
/// the wired cross-Worker (`Integration`) machinery; otherwise it stays
/// in-process (`Test`).
pub fn suite_effective_tier_is_system(suite: &SuiteDecl) -> bool {
    suite.tier == Some(TestTier::System)
        || suite.cases.iter().any(|c| c.tier == Some(TestTier::System))
}

/// Read a source file from the overlay (keyed by canonicalised absolute
/// path; falls back to the literal path so a not-yet-created overlay entry
/// still matches). Every caller into this module now supplies a complete
/// overlay — content-ownership track (#1086) slice 5 removed the disk-read
/// fallback this used to have on a miss, so an incomplete overlay is a real
/// `NotFound` error here, not a silent disk read.
///
/// Finding #55/#65: tries the literal path first, `canonicalize()` only on a
/// miss — an in-memory/wasm project's synthetic overlay keys never exist on
/// disk, so `canonicalize()` was a guaranteed-failing syscall on every read of
/// every such file, for no benefit (the literal-path lookup below already
/// finds the same entry).
pub fn read_source(path: &Path, overlay: &HashMap<PathBuf, String>) -> std::io::Result<String> {
    if let Some(text) = overlay.get(path) {
        return Ok(text.clone());
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(text) = overlay.get(&canonical) {
        return Ok(text.clone());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("no overlay entry for `{}`", path.display()),
    ))
}

/// An adapter's `.binding.ts` module: overlay-first (an open, unsaved
/// binding buffer), else a real disk read. Content-ownership track (#1086)
/// scope note, found under slice 5's implementation: unlike a project's
/// `.bynk` sources — enumerable ahead of time by extension, and this
/// track's actual charter — a binding module's *path* is only known once
/// its declaring adapter has been parsed (`adapter … { binding: "…" }`), so
/// no discovery walk (`bynk-testkit`, `bynk-driver::discovery`) can
/// pre-populate it into a sources map the way `.bynk` files are. Keeping a
/// disk-read fallback here — the CLI's real production path has always
/// worked exactly this way, `#1077`/`#1081` notwithstanding — is a
/// deliberate, narrow carve-out, not a straggler.
pub fn read_adapter_binding(
    path: &Path,
    overlay: &HashMap<PathBuf, String>,
) -> std::io::Result<String> {
    if let Some(text) = overlay.get(path) {
        return Ok(text.clone());
    }
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if let Some(text) = overlay.get(&canonical) {
        return Ok(text.clone());
    }
    fs::read_to_string(path)
}

/// A parsed `.bynk` file: its source, AST, and the two path forms it needs.
///
/// Slice 0: `source_path` and `identity_path` are **different things**, and
/// conflating them is what made a two-root project's file identity ambiguous.
/// They coincide for a single-root project, which is why one field sufficed
/// until `include` could hold two entries.
///
/// P4.0 (#1113, [DECISION B]): fields are crate-private now that `ParsedFile`
/// lives in `bynk-project` — `bynk-emit`'s `symbols`/`validate` read them
/// through the accessors below instead of the direct field pokes a
/// same-crate `pub(crate)` allowed before the move.
#[derive(Clone)]
pub struct ParsedFile {
    /// The path **relative to the `include` root that contains this file** —
    /// the form unit validation requires. `src/todos.bynk` under the `src`
    /// root is `todos.bynk`, which is what lets it declare `context todos`
    /// ([`crate::paths::unit_path_matches`], via
    /// [`crate::consistency::check_path_name_alignment`]). Prefixing this
    /// would make every unit in every project fail alignment.
    pub(crate) source_path: PathBuf,
    /// Slice 0: the path **relative to the project root** — this file's
    /// identity, unique across `include` roots. `src/todos.bynk` and
    /// `tests/todos.bynk` share a `source_path` (`todos.bynk`) but differ
    /// here. Everything that *keys* a file — the analysed snapshots, the
    /// diagnostic attribution — uses this; nothing that *validates a unit's
    /// name* may.
    ///
    /// Equal to `source_path` for a single-root project (`Roots::Single`
    /// resolves to one tree with an empty prefix), so single-root behaviour
    /// is unchanged by construction.
    pub(crate) identity_path: PathBuf,
    /// v0.72: the absolute path the compiler read this file from, used as the
    /// source-map `sources` entry so an editor's breakpoint (set on the real
    /// `.bynk` file) resolves to the same path the debugger loads. `None` for
    /// toolchain-injected synthetic units, which have no on-disk source.
    pub(crate) abs_path: Option<PathBuf>,
    pub(crate) source: String,
    pub(crate) unit: SourceUnit,
    pub(crate) kind: UnitKind,
    /// v0.17: true for toolchain-injected units (the `bynk` surface) — exempt
    /// from the reserved-namespace and missing-binding checks.
    pub(crate) synthetic: bool,
    /// #2223: the dependency package this file belongs to, or `None` for the
    /// project's own files and first-party units. Set by `bynk-check` once
    /// the package graph is known ([`Self::assign_package`]).
    pub(crate) package: Option<crate::packages::PackageId>,
}

impl ParsedFile {
    /// Construct directly — used by `bynk-emit`'s first-party synthetic-unit
    /// injection (`firstparty_parsed`), which builds a `ParsedFile` for a
    /// toolchain-supplied source (`bynk.bynk`, `bynk.cloudflare`, …) that
    /// never went through [`parse_sources`]'s discovery-driven path.
    pub fn synthetic(
        identity_path: PathBuf,
        source_path: PathBuf,
        source: String,
        unit: SourceUnit,
        kind: UnitKind,
    ) -> Self {
        ParsedFile {
            source_path,
            identity_path,
            abs_path: None,
            source,
            unit,
            kind,
            synthetic: true,
            package: None,
        }
    }

    /// General constructor — `bynk-emit`'s own tests use this to build a
    /// hand-rolled `ParsedFile` fixture (a specific `source_path`/
    /// `identity_path` pair, a non-synthetic unit) that neither
    /// [`Self::synthetic`] (forces `source_path == identity_path`,
    /// `synthetic: true`) nor [`parse_sources`] (needs a real token stream)
    /// fits.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        source_path: PathBuf,
        identity_path: PathBuf,
        abs_path: Option<PathBuf>,
        source: String,
        unit: SourceUnit,
        kind: UnitKind,
        synthetic: bool,
    ) -> Self {
        ParsedFile {
            source_path,
            identity_path,
            abs_path,
            source,
            unit,
            kind,
            synthetic,
            package: None,
        }
    }

    /// The path **relative to the `include` root that contains this file**.
    pub fn source_path(&self) -> PathBuf {
        self.source_path.clone()
    }

    /// The path **relative to the project root** — this file's identity,
    /// unique across `include` roots. See the field's own doc for why this
    /// and [`Self::source_path`] must not be conflated.
    pub fn identity_path(&self) -> PathBuf {
        self.identity_path.clone()
    }

    /// The absolute path this file was read from, when it has one — `None`
    /// for toolchain-injected synthetic units.
    pub fn abs_path(&self) -> Option<PathBuf> {
        self.abs_path.clone()
    }

    pub fn kind(&self) -> UnitKind {
        self.kind
    }

    /// #2223: the dependency package this file belongs to; `None` for the
    /// project's own files and first-party units.
    pub fn package(&self) -> Option<&crate::packages::PackageId> {
        self.package.as_ref()
    }

    /// #2223: mark this file as part of dependency `id`, and give it the path
    /// its units' canonical names map to (`deps/<org>/<package>/<rel>`), so
    /// name alignment and the emitted layout both follow from
    /// [`crate::commons_dir_for`]. The identity path — the real file, relative
    /// to the consuming project — is unchanged.
    pub fn assign_package(&mut self, id: crate::packages::PackageId) {
        self.source_path = id.out_dir().join(&self.source_path);
        self.package = Some(id);
    }

    /// Override the discovered kind — `bynk-emit`'s own tests use this to
    /// build a scenario's intermediate unit as a commons regardless of what
    /// AST shape (`context_using`, …) constructed it, without needing a
    /// second builder per kind.
    pub fn set_kind(&mut self, kind: UnitKind) {
        self.kind = kind;
    }

    /// True for toolchain-injected units (the `bynk` surface).
    pub fn is_synthetic(&self) -> bool {
        self.synthetic
    }

    pub fn unit(&self) -> &SourceUnit {
        &self.unit
    }

    /// Mutable access to the parsed unit — `bynk-emit`'s
    /// `normalize_service_defaults` (service `by`/`given` default injection)
    /// is the one caller that rewrites a unit's items in place, ahead of
    /// grouping/checking.
    pub fn unit_mut(&mut self) -> &mut SourceUnit {
        &mut self.unit
    }

    /// The raw source text this file was parsed from.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// v0.72: the source-map `sources` entry for this file — the absolute path
    /// the compiler read it from (forward slashes), so an editor breakpoint set
    /// on the real `.bynk` resolves to the same path the debugger loads. A
    /// project-relative name would resolve against the emitted `.ts`'s directory,
    /// which is the wrong place. Synthetic units (no on-disk source) fall back to
    /// their relative path.
    pub fn map_source_name(&self) -> String {
        self.abs_path
            .as_deref()
            .unwrap_or(self.source_path.as_path())
            .to_string_lossy()
            .replace('\\', "/")
    }

    /// State-migrations S3 (#2248): a context file's `removed agent`
    /// declarations; empty for every other unit.
    pub fn removed_agents(&self) -> &[bynk_syntax::ast::RemovedAgentDecl] {
        match &self.unit {
            SourceUnit::Context(c) => &c.removed_agents,
            _ => &[],
        }
    }

    pub fn items(&self) -> &Vec<CommonsItem> {
        match &self.unit {
            SourceUnit::Commons(c) => &c.items,
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Adapter(a) => &a.items,
            SourceUnit::Suite(_) => {
                // Tests don't contribute CommonsItem items; the production
                // pipeline never asks them to. Return a singleton empty vec.
                static EMPTY: std::sync::OnceLock<Vec<CommonsItem>> = std::sync::OnceLock::new();
                EMPTY.get_or_init(Vec::new)
            }
        }
    }

    /// P6.x (#1137): does this file declare a `messages { … }` block? The one
    /// predicate a `messages`-bundle emitter needs — whether to inject the
    /// `bynk.locale` `render` fallback import — without the caller having to
    /// walk [`items`](Self::items) and match [`CommonsItem::Messages`] itself.
    pub fn declares_messages(&self) -> bool {
        self.items()
            .iter()
            .any(|it| matches!(it, CommonsItem::Messages(_)))
    }

    pub fn uses(&self) -> &Vec<UsesDecl> {
        match &self.unit {
            SourceUnit::Commons(c) => &c.uses,
            SourceUnit::Context(c) => &c.uses,
            SourceUnit::Adapter(a) => &a.uses,
            SourceUnit::Suite(t) => &t.uses,
        }
    }

    pub fn consumes(&self) -> &[ConsumesDecl] {
        match &self.unit {
            SourceUnit::Commons(_) => &[],
            SourceUnit::Context(c) => &c.consumes,
            // v0.18: adapter-to-adapter capability dependencies (spec §4.5).
            SourceUnit::Adapter(a) => &a.consumes,
            // An integration test's participant edges are resolved separately
            // (the harness root consumes every participant); it has no
            // `consumes` of its own.
            SourceUnit::Suite(_) => &[],
        }
    }

    /// `exports` clauses, for the unit kinds that have them (contexts and
    /// adapters). Empty for commons/tests.
    pub fn exports(&self) -> &[ExportsDecl] {
        match &self.unit {
            SourceUnit::Context(c) => &c.exports,
            SourceUnit::Adapter(a) => &a.exports,
            _ => &[],
        }
    }

    pub fn adapter(&self) -> Option<&AdapterDecl> {
        match &self.unit {
            SourceUnit::Adapter(a) => Some(a),
            _ => None,
        }
    }

    pub fn test(&self) -> Option<&SuiteDecl> {
        match &self.unit {
            SourceUnit::Suite(t) => Some(t),
            _ => None,
        }
    }

    /// v0.119 (ADR 0155): the agent names this file's own `for all run:
    /// History[Agent]` properties drive — `emit_agent` gates the exported
    /// `__bynkDriveHistory_<Agent>` driver on membership. Empty for a
    /// non-suite file, or a suite with no such property.
    pub fn history_target_agent_names(&self) -> impl Iterator<Item = &str> {
        self.test()
            .into_iter()
            .flat_map(|t| &t.properties)
            .flat_map(|prop| &prop.forall.bindings)
            .filter_map(|b| match &b.type_ref {
                TypeRef::History(inner, _) => match inner.as_ref() {
                    TypeRef::Named(id) => Some(id.name.as_str()),
                    _ => None,
                },
                _ => None,
            })
    }

    /// v0.118: a suite whose *effective* tier is `system` is emitted through
    /// the wired cross-Worker machinery (the retired standalone `integration`
    /// path, now re-driven from tiers). Returns the underlying [`SuiteDecl`]
    /// when this file is such a suite.
    pub fn integration(&self) -> Option<&SuiteDecl> {
        match &self.unit {
            SourceUnit::Suite(t) if suite_effective_tier_is_system(t) => Some(t),
            _ => None,
        }
    }

    /// Build a synthetic Commons AST node carrying the given items, so the
    /// existing resolver/checker pipeline can be driven uniformly.
    pub fn as_synthetic_commons(&self, items: Vec<CommonsItem>) -> Commons {
        let (name, uses, documentation, form, span) = match &self.unit {
            SourceUnit::Commons(c) => (
                c.name.clone(),
                c.uses.clone(),
                c.documentation.clone(),
                c.form,
                c.span,
            ),
            SourceUnit::Context(c) => (
                c.name.clone(),
                c.uses.clone(),
                c.documentation.clone(),
                c.form,
                c.span,
            ),
            SourceUnit::Suite(t) => (
                t.target.clone(),
                t.uses.clone(),
                t.documentation.clone(),
                t.form,
                t.span,
            ),
            SourceUnit::Adapter(a) => (
                a.name.clone(),
                a.uses.clone(),
                a.documentation.clone(),
                a.form,
                a.span,
            ),
        };
        Commons {
            name,
            items,
            uses,
            documentation,
            form,
            span,
            trivia: Trivia::default(),
            trailing_comments: Vec::new(),
        }
    }
}

/// Parse already-read source text into a [`ParsedFile`]. The read happens
/// at the call site (v0.24): the pipeline owns the text for snapshots and
/// per-file error attribution, and the overlay supplies unsaved buffers.
/// Slice 0: `prefix` is this tree's project-root-relative `include` prefix
/// (`src`, `tests`, …), empty for a single-root project. It builds each file's
/// `identity_path`; `source_path` stays relative to `root` (the tree), which is
/// what unit validation reads. See [`ParsedFile`].
///
/// P8.4 (#1515): the actual tokenize+parse now runs through
/// [`crate::parse_cache::cached_parse`], keyed on the file's own absolute
/// path — durable across separate calls to this function, not just within
/// one, so a `FileId` (and the `ExprId`s a file's own expressions carry) no
/// longer resets every time a fresh analysis pass parses an unchanged file.
/// `next_expr_id`/`next_file_id` are no longer threaded in from the caller
/// for this reason — the cache owns both counters now, durably.
pub fn parse_sources(
    root: &Path,
    prefix: &Path,
    path: &Path,
    source: String,
) -> Result<(Vec<ParsedFile>, Vec<CompileError>), Vec<CompileError>> {
    // v0.72: cache (and identify a `ParsedFile`) by the *absolute* path —
    // `path` is relative when the compiler was invoked with a relative input
    // (`bynkc test .`), and a relative cache key would let the same file seen
    // through two differently-rooted calls collide, or worse, miss. Falls
    // back to `path` itself on the rare `std::path::absolute` failure, same
    // as the pre-existing `abs_path` field it also feeds.
    let abs_path = std::path::absolute(path).ok();
    let cache_key: &Path = abs_path.as_deref().unwrap_or(path);
    // v0.113: a file may declare more than one top-level unit — an *atomic*
    // file holding `commons`/`context` alongside a `suite` (DECISION S). Each
    // unit becomes its own `ParsedFile` sharing the file's source and path, so
    // the downstream grouping partitions *declarations* by kind: the source
    // units flow to the build, the suites to `bynkc test` only.
    // ADR 0117: a warning-severity parse diagnostic (an orphan doc block)
    // must not hard-fail discovery — the parsed units flow to the build and
    // the warnings ride out to the caller's severity-aware sink.
    let (_file_id, result) = crate::parse_cache::cached_parse(cache_key, &source);
    let (units, warnings) = result.map_err(|errors| (*errors).clone())?;
    let files = parsed_files(root, prefix, path, abs_path, &source, units.iter().cloned());
    Ok((files, (*warnings).clone()))
}

/// #1710: what a file the strict parse rejects still yields, from its
/// recovering parse: a [`ParsedFile`] per unit it recovered, the names of the
/// declarations recovery had to skip (references to them are known names, not
/// unknown ones; #1663's Decision B), and the recovering parse's diagnostics.
/// `files` is empty when no unit survived (a broken header); the diagnostics
/// still come from this recovery, with real spans. These files are for
/// diagnostics only: the strict parse's error still stands, so nothing they
/// belong to is ever emitted.
pub struct RecoveredSources {
    pub files: Vec<ParsedFile>,
    pub broken_decl_names: Vec<String>,
    pub errors: Vec<CompileError>,
}

/// #1710: [`parse_sources`] for a file whose strict parse failed, from the
/// recovering parse the parse cache keeps beside that failure. `None` only
/// when there is none: the source didn't lex.
pub fn parse_sources_recovering(
    root: &Path,
    prefix: &Path,
    path: &Path,
    source: String,
) -> Option<RecoveredSources> {
    let abs_path = std::path::absolute(path).ok();
    let cache_key: &Path = abs_path.as_deref().unwrap_or(path);
    let recovered = crate::parse_cache::cached_recovery(cache_key, &source)?;
    let files = parsed_files(
        root,
        prefix,
        path,
        abs_path,
        &source,
        recovered.units.iter().cloned(),
    );
    Some(RecoveredSources {
        files,
        broken_decl_names: recovered.broken_decl_names.clone(),
        errors: recovered.errors.clone(),
    })
}

/// One [`ParsedFile`] per unit of a file (a file may declare several, v0.113),
/// each classified by kind and sharing the file's source and paths.
fn parsed_files(
    root: &Path,
    prefix: &Path,
    path: &Path,
    abs_path: Option<PathBuf>,
    source: &str,
    units: impl Iterator<Item = SourceUnit>,
) -> Vec<ParsedFile> {
    let rel = path.strip_prefix(root).unwrap_or(path).to_path_buf();
    units
        .map(|unit| {
            let kind = match &unit {
                SourceUnit::Commons(_) => UnitKind::Commons,
                SourceUnit::Context(_) => UnitKind::Context,
                // v0.118: a suite whose effective tier is `system` is emitted
                // through the wired cross-Worker machinery (classified as
                // `Integration`); unit/integration-tier suites stay in-process.
                SourceUnit::Suite(t) if suite_effective_tier_is_system(t) => UnitKind::Integration,
                SourceUnit::Suite(_) => UnitKind::Test,
                SourceUnit::Adapter(_) => UnitKind::Adapter,
            };
            ParsedFile {
                abs_path: abs_path.clone(),
                identity_path: prefix.join(&rel),
                source_path: rel.clone(),
                source: source.to_string(),
                unit,
                kind,
                synthetic: false,
                package: None,
            }
        })
        .collect()
}

pub fn discover_bynk_files(
    root: &Path,
    excludes: &[PathBuf],
) -> Result<Vec<PathBuf>, CompileError> {
    if !root.exists() {
        return Err(CompileError::new(
            "bynk.project.no_root",
            Span::default(),
            format!("project root does not exist: {}", root.display()),
        ));
    }
    // v0.113: skip excluded subtrees (author `exclude` + the tool's own caches)
    // and hidden directories, so an `include` root at the project root does not
    // sweep up generated, vendored, or dot-directory `.bynk`.
    let is_excluded = |dir: &Path| {
        excludes.iter().any(|ex| dir == ex || dir.starts_with(ex))
            || dir
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.') && n != ".")
    };
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match fs::read_dir(&dir) {
            Ok(r) => r,
            Err(e) => {
                return Err(CompileError::new(
                    "bynk.project.read_failed",
                    Span::default(),
                    format!("could not read directory `{}`: {e}", dir.display()),
                ));
            }
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                if !is_excluded(&p) {
                    stack.push(p);
                }
            } else if p.extension().and_then(|e| e.to_str()) == Some("bynk") {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Slice A: the `.bynk` files these roots contain — the **same walk**
/// `compile_project` performs, honouring `exclude` and the tool's own `out`/
/// `node_modules` caches.
///
/// P4.2 (#1122, Decision B): moved here from `bynk-emit/src/project.rs` — its
/// body called only `bynk-project`-local functions already, with no
/// `bynk-emit`-specific state. `bynk-emit` re-exports it at its existing
/// `bynk_emit::project::discover_project_files` path so `read_disk_sources`
/// and `bynk-testkit` need no edit; `bynk-ide` calls this path directly.
pub fn discover_project_files(roots: &Roots) -> Vec<PathBuf> {
    let mut trees = roots.trees();
    // #2223: a manifest-backed project's path dependencies are part of it, so
    // the editor's content sweep and the test kit read their files too.
    if let Roots::Split { project_root, .. } = roots {
        trees.extend(crate::packages::dependency_trees(
            project_root,
            &HashMap::new(),
        ));
    }
    let excludes = roots.excludes();
    let mut out = Vec::new();
    for (root, _prefix) in &trees {
        // Every tree past the first is optional — a project may simply have
        // no such subtree (R3.9, #1113: every `include` entry is walked, not
        // just the first two). `unwrap_or_default` already treats a missing
        // root the same as "no files here" for every tree, first included —
        // no need to `root.exists()` before calling `discover_bynk_files`
        // (itself a `fs::read_dir`) just to decide whether to call it: that
        // would cost a redundant `stat()` per tree for the same answer.
        out.extend(discover_bynk_files(root, &excludes).unwrap_or_default());
    }
    out.sort();
    out.dedup();
    out
}

pub fn check_file_directory_conflicts(
    root: &Path,
    files: &[PathBuf],
) -> Result<(), Vec<CompileError>> {
    let mut errors: Vec<CompileError> = Vec::new();
    let mut bynk_files: HashSet<PathBuf> = HashSet::new();
    let mut dirs_with_bynk: HashSet<PathBuf> = HashSet::new();
    for p in files {
        let rel = p.strip_prefix(root).unwrap_or(p);
        bynk_files.insert(rel.to_path_buf());
        if let Some(parent) = rel.parent() {
            dirs_with_bynk.insert(parent.to_path_buf());
        }
    }
    for f in &bynk_files {
        let stem = f.with_extension("");
        if dirs_with_bynk.contains(&stem) {
            errors.push(
                CompileError::new(
                    "bynk.project.file_and_directory",
                    Span::default(),
                    format!(
                        "commons at `{}` is ambiguous: both `{}` and `{}/` exist with `.bynk` content",
                        f.with_extension("").display(),
                        f.display(),
                        stem.display()
                    ),
                )
                .with_note(
                    "a commons can be a single `.bynk` file OR a directory of `.bynk` files, not both",
                ),
            );
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #1662: discovery reports a missing project root as a diagnostic, which
    /// a library host (the LSP, a multi-root build) surfaces as-is; the CLI
    /// checks the path itself first, so only this test reaches the code.
    #[test]
    fn a_missing_root_is_reported_as_no_root() {
        let root = std::env::temp_dir().join(format!(
            "bynk-discovery-no-root-{}-does-not-exist",
            std::process::id()
        ));
        let err =
            discover_bynk_files(&root, &[]).expect_err("a missing root must not discover files");
        assert_eq!(err.category, "bynk.project.no_root");
    }

    /// Finding #55/#65: a synthetic path that never exists on disk (the
    /// in-memory/wasm case) must still resolve via the overlay's literal-path
    /// entry — `canonicalize()` on such a path always fails, so the fix tries
    /// the literal path first rather than paying for that failing syscall on
    /// every read.
    #[test]
    fn read_source_finds_a_synthetic_overlay_path_that_does_not_exist_on_disk() {
        let path = PathBuf::from("./__bynk_in_memory__/t.bynk");
        let mut overlay = HashMap::new();
        overlay.insert(path.clone(), "context t\n".to_string());
        let got = read_source(&path, &overlay).expect("the overlay entry must be found");
        assert_eq!(got, "context t\n");
    }

    /// Content-ownership track (#1086) slice 5: a real on-disk file with no
    /// overlay entry must now error, never silently fall back to reading it
    /// off disk — the disk-read fallback this test guards the absence of was
    /// deleted in this slice; every caller supplies a complete overlay.
    #[test]
    fn read_source_errors_on_a_real_file_with_no_overlay_entry_rather_than_reading_disk() {
        let dir = std::env::temp_dir().join(format!(
            "bynk-emit-discovery-fallback-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let path = dir.join("t.bynk");
        std::fs::write(&path, "context t\n").expect("write real file");
        let got = read_source(&path, &HashMap::new());
        std::fs::remove_dir_all(&dir).ok();
        assert!(
            got.is_err(),
            "a real file with no overlay entry must not be silently read from disk"
        );
        assert_eq!(got.unwrap_err().kind(), std::io::ErrorKind::NotFound);
    }
}
