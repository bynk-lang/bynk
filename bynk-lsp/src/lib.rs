//! `bynkc-lsp` — Bynk Language Server.
//!
//! Implements the LSP capabilities listed in `design/bynk-lsp-spec.md` §4.3:
//! synchronisation (Full), diagnostics, hover, go-to-definition and -type/-impl,
//! formatting, document symbols, completion, signature help, references, rename,
//! code actions, code lens, call hierarchy, document links, inlay hints,
//! semantic tokens, workspace symbols, real multi-root workspace folders, and
//! server-registered file watching. Built on `tower-lsp`.
//!
//! Architecture:
//! - [`Backend`] holds the server state (behind a `tokio::sync::RwLock`): a
//!   **map of projects** keyed by discovered root — each with its own config,
//!   analysis round, and published set — plus the workspace-folder discovery
//!   seeds and the client-global map of open documents. A request routes by URI
//!   to its project (its nearest enclosing `bynk.toml`); a file under none is
//!   single-file.
//! - Document changes trigger `schedule_diagnostics`, one generation-based
//!   debounce (a project-wide round via [`bynk_ide::diagnose_project_with`], or
//!   single-file [`bynk_ide::diagnose`]) that publishes the resulting
//!   diagnostics.
//! - Hover and definition consult the parsed AST for the file under the
//!   cursor; both are best-effort (return None for unrecognised positions).
//! - Formatting delegates to [`bynk_fmt::format_source`].
//!
//! Slice C (the `[lib]` seam): this crate exposes a library target so its
//! integration tests can `use bynk_lsp::…` instead of `#[path]`-including source
//! modules. The `pub mod`s below are exposed for that testing, **not** as a
//! stable API — `bynk-lsp` is a language-server binary and makes no library
//! compatibility promise.

pub mod agent_state_request;
pub mod architecture_request;
pub mod capability_fixes;
pub mod code_actions;
pub mod completion;
mod content;
mod document_symbols;
pub mod documentation_request;
pub mod emitted_request;
pub mod explain_request;
pub mod extract;
pub mod fill_arms;
pub mod fix_all;
pub mod hover;
pub mod index_queries;
mod inlay_hints;
pub mod inline;
mod locals_nav;
pub mod organise;
pub mod position;
mod project;
pub mod provider_stub;
mod publish;
mod schema_lock;
pub mod sequence_request;
mod signature_help;
mod structure;
pub mod symbols;
pub mod transport;
pub mod wire_contract_request;

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;
use tower_lsp::jsonrpc::Result as JsonRpcResult;
use tower_lsp::lsp_types::request::{
    GotoImplementationParams, GotoImplementationResponse, GotoTypeDefinitionParams,
    GotoTypeDefinitionResponse,
};
use tower_lsp::lsp_types::*;
use tower_lsp::{Client, LanguageServer, LspService, Server};

use crate::project::ProjectConfig;

const SERVER_NAME: &str = "bynkc-lsp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// In-memory document state.
#[derive(Debug, Clone)]
struct DocumentState {
    text: String,
    version: i32,
}

/// v0.25 (ADR 0053): one analysis round's retained outputs — the binding
/// index plus the snapshots its spans are offsets into, and the open-doc
/// versions captured when the overlay was built (rename emits versioned
/// edits against exactly these versions).
#[derive(Debug)]
struct Analysis {
    /// Slice A: the canonicalised **project root** every path in this round
    /// resolves against. Was the single `src` directory; the round now covers
    /// every `include` tree, and ADR 0198 makes each file's path
    /// project-relative — so this is the one base that resolves all of them.
    project_root: PathBuf,
    index: bynk_check::index::ProjectIndex,
    /// Project-relative path → the analysed text.
    snapshots: std::collections::HashMap<PathBuf, String>,
    /// Project-relative path → the open document's version at analysis
    /// time (absent for files read from disk).
    versions: std::collections::HashMap<PathBuf, i32>,
    /// v0.26 (ADR 0054): project-relative path → the round's diagnostics,
    /// full `CompileError`s included — the suggestions `codeAction` serves
    /// ride on them. Every analysed file has an entry (clean files an empty
    /// one). Replaces the v0.25 categories-only field; the rename baseline
    /// derives from these via [`Self::diag_categories`].
    diagnostics: std::collections::HashMap<PathBuf, Vec<bynk_ide::Diagnostic>>,
    /// v0.27 (ADR 0056): project-relative path → the round's harvested
    /// inferred-type hints, spans against the analysed snapshots.
    hints: bynk_check::hints::FileHints,
    /// v0.99: project-relative path → the round's capability-requirement ledger,
    /// driving the materializable ghost `given` inlay hint, spans against the
    /// analysed snapshots.
    requirements: bynk_check::requirements::FileRequirements,
    /// v0.31 (ADR 0064): project-relative path → the round's local bindings
    /// with scope ranges, for locals navigation (references/definition/
    /// highlight), spans against the analysed snapshots.
    locals: bynk_check::locals::FileLocals,
    /// Slice 6: project-relative path → the round's expression types, spans
    /// against the analysed snapshots — backs go-to-type-definition.
    expr_types: bynk_check::expr_types::FileExprTypes,
    /// T3.6b (R4.1): the round's intern table — what every `TyId` in
    /// `expr_types` resolves against. One per analysis round, shared across
    /// every unit it checked.
    ty_intern: std::sync::Arc<bynk_check::checker::Types>,
    /// Slice 6b (ADR 0095): qualified unit name → its project source file(s),
    /// project-relative — backs document links (`uses`/`consumes` → source).
    unit_sources: std::collections::HashMap<String, Vec<PathBuf>>,
    /// #846: qualified context/adapter unit name → the cross-context/agent
    /// tables the `bynk/sequenceModel` request classifies handler calls
    /// against.
    sequence_info: std::collections::HashMap<String, bynk_ide::ContextSequenceInfo>,
    /// #855: qualified context/adapter unit name → the combined type table
    /// and service/agent tables the `bynk/wireContract` request resolves a
    /// handler's boundary shape and cross-context hash against.
    boundary_info: std::collections::HashMap<String, bynk_ide::ContextBoundaryInfo>,
    /// #848: qualified unit name → its doc-comment intra-doc-link search
    /// order — itself first, then its `uses` targets, then its `consumes`
    /// targets — backs intra-doc-link resolution in `document_link` and
    /// `hover`. See `bynk_ide::ProjectDiagnostics::doc_scope`.
    doc_scope: std::collections::HashMap<String, Vec<String>>,
}

impl Analysis {
    /// Per-file diagnostic categories — the rename validator's baseline,
    /// derived from the retained diagnostics.
    fn diag_categories(&self) -> Vec<(PathBuf, String)> {
        self.diagnostics
            .iter()
            .flat_map(|(path, diags)| {
                diags
                    .iter()
                    .map(|d| (path.clone(), d.error.category.to_string()))
            })
            .collect()
    }
}

/// One project's mutable state — the fields that were flat on `State` before
/// slice D, now one set per discovered project root. Every request routes by
/// URI (via `resolve_root`) to its owning entry, so two projects analyse,
/// version, and publish independently.
#[derive(Debug, Default)]
struct ProjectState {
    /// Parsed `bynk.toml` configuration for this root. Defaults for missing
    /// fields. Read live for the diagnostics mode/debounce and formatting;
    /// reloaded on a `bynk.toml` change (`did_change_watched_files`).
    config: ProjectConfig,
    /// v0.25: the latest analysis round's index + snapshots. References,
    /// rename, and the re-pointed definition/hover read this; positions
    /// convert against the analysed snapshots (v0.24 rule).
    analysis: Option<Arc<Analysis>>,
    /// v0.24: URIs that currently carry published project diagnostics — the
    /// previous round's dirty set, so newly-clean files get a clearing
    /// (empty) publish. Per-project (slice D): a round for this root must only
    /// clear its own files, never another project's.
    published: std::collections::HashSet<Url>,
    /// v0.24: debounce generation. Each change bumps it; a scheduled
    /// analysis runs only if it is still the latest when the delay elapses.
    /// Per-project: two projects debounce independently.
    analysis_generation: u64,
    /// Monotonic id handed to each analysis round as it *starts*. Together
    /// with `analysis_round_committed` this orders round completions: an old
    /// slow round must never overwrite a newer round's results (#513).
    /// Per-project (slice D): a global counter would let one project's round
    /// discard another's.
    analysis_round_started: u64,
    /// The id of the newest round whose results have been committed.
    analysis_round_committed: u64,
    /// #1667: the newest round panicked and no round has committed since.
    /// The client is told once per such streak, not on every keystroke.
    analysis_failed: bool,
    /// #1667: makes this project's next round panic, so a test can watch the
    /// failure reach the client. Per project, not a global, so parallel tests'
    /// rounds can't take it.
    #[cfg(test)]
    panic_next_round: bool,
    /// #2081: this project's `bynk.schema.lock`, read-only and cached by its
    /// `(mtime, len)` stamp, handed to every diagnostics round so the editor
    /// reports a non-additive event change as `bynk check` does. Cleared by
    /// the `**/bynk.schema.lock` watcher.
    schema_lock: schema_lock::SchemaLockCache,
}

/// #733: the client's `workspace/*/refresh` support, per pull-based decoration,
/// captured at `initialize`. Each flag gates the corresponding round-commit
/// nudge in [`Backend::run_project_diagnostics`].
#[derive(Debug, Clone, Copy, Default)]
struct RefreshSupport {
    semantic_tokens: bool,
    inlay_hints: bool,
    code_lens: bool,
}

/// Mutable server state. Slice D: a map of projects (was one flat project),
/// plus the open buffers (client-global) and the workspace-folder seeds.
#[derive(Debug, Default)]
struct State {
    /// Discovered projects, keyed by **canonical project root** (Q4: the
    /// directory a file's `resolve_root` walk lands on — a `bynk.toml`, else an
    /// implicit `src/` parent). Empty in single-file mode. A request routes to
    /// its entry by URI; the entry is created lazily on first touch (open or
    /// request) and pruned when no folder covers it and it holds no open buffer.
    projects: std::collections::HashMap<PathBuf, ProjectState>,
    /// The workspace-folder roots the client has open (slice D). **Discovery
    /// seeds, not routing owners** (Q4): they bound where
    /// `did_change_workspace_folders` prunes, but a URI routes by its nearest
    /// enclosing `bynk.toml`, which may sit above every folder.
    folders: Vec<PathBuf>,
    /// Open documents keyed by URI — a client-global set; each doc routes to
    /// its project via `resolve_root`.
    docs: std::collections::HashMap<Url, DocumentState>,
    /// Slice E: whether the client advertised `didChangeWatchedFiles`
    /// **dynamic registration** at `initialize`. When set, `initialized`
    /// registers the file watchers server-side (so any client is notified);
    /// when not, the client is expected to supply them itself (as VS Code did
    /// before the extension's client-side watchers were removed).
    supports_dynamic_watchers: bool,
    /// #733: whether the client advertised `refresh_support` for each pull-based
    /// decoration at `initialize`. When set, a committed round asks the client to
    /// re-pull that decoration (`workspace/*/refresh`) — the "revalidate" half of
    /// serving `committed_analysis` stale while typing. Only sent when advertised,
    /// so a client that never supported it is never spammed with unknown requests.
    supports_refresh: RefreshSupport,
    /// #2098: whether the client advertised
    /// `experimental.explainDiagnosticCommand` at `initialize` — it implements
    /// the client-side `bynk.explainDiagnostic` command, so `code_action`
    /// offers the "Explain …" action. Off for any other client.
    supports_explain_command: bool,
    /// #2100 slice 2: whether the client shows a disabled code action's
    /// reason (`codeAction.disabledSupport`) — extract-function tells the user
    /// why a capability-using selection can't be extracted that way.
    supports_code_action_disabled: bool,
    /// Slice F: debounce generation for **single-file** buffers (no project),
    /// keyed by URI. The project path holds its generation in `ProjectState`;
    /// this is the same coalescing for a buffer that has no entry — a burst runs
    /// one `diagnose`, not one per keystroke. Cleared on `did_close`.
    single_file_generations: std::collections::HashMap<Url, u64>,
    /// #682: memoised URI → canonical project root routing (`None` for
    /// single-file mode is itself a cached answer), so the hot request path
    /// stops re-walking the filesystem and `canonicalize()`ing on every call.
    /// For a URI whose own path is fixed, routing depends only on `bynk.toml`
    /// presence among its ancestors — `find_source_root`'s `src`-ancestor
    /// fallback is a pure string match against that fixed path, with no
    /// filesystem I/O of its own, so it can't drift independently. That makes
    /// a `bynk.toml` create/delete/change the only event that can move an
    /// already-cached URI's route, and this is invalidated wholesale on it
    /// (`did_change_watched_files`). A workspace-folder change also clears it
    /// (`did_change_workspace_folders`) even though `resolve_canonical` never
    /// consults `folders` today — a defensive, effectively-free no-op kept in
    /// case that ever changes, not a correctness requirement. Bounded entries
    /// are never individually evicted (e.g. on `did_close`); only ever
    /// wholesale-cleared, which is judged an acceptable tradeoff — bounded by
    /// the distinct files touched in a session. See [`Backend::root_for_uri`].
    root_cache: std::collections::HashMap<Url, Option<PathBuf>>,
    /// #682: bumped every time `root_cache` is wholesale-cleared. `root_for_uri`
    /// resolves a cache miss off the `state` lock (a filesystem walk must not
    /// run while holding it); this closes the race where an invalidating clear
    /// lands *during* that walk — the write-back re-checks the generation and
    /// drops a stale answer instead of resurrecting it into the freshly-cleared
    /// cache.
    root_cache_generation: u64,
}

#[derive(Clone)]
pub struct Backend {
    client: Client,
    state: Arc<RwLock<State>>,
    /// Slice B (the freshness contract): serialises request-driven refreshes so
    /// concurrent index-backed requests after one edit coalesce onto a single
    /// round instead of each spawning its own. Held only across `analysis_for`'s
    /// refresh; never across a `state` lock.
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    /// #1667: set when the client sends `shutdown`. [`run`] reads it once the
    /// session ends, to exit 0 after an orderly shutdown and 1 otherwise, as
    /// the LSP spec's `exit` notification prescribes.
    shutdown_requested: Arc<std::sync::atomic::AtomicBool>,
}

/// #1667: a failed `spawn_blocking` task, described for the log: the panic's
/// message when it panicked, else that it was cancelled.
fn describe_join_error(e: tokio::task::JoinError) -> String {
    if !e.is_panic() {
        return format!("cancelled ({e})");
    }
    let payload = e.into_panic();
    if let Some(s) = payload.downcast_ref::<&str>() {
        format!("panicked: {s}")
    } else if let Some(s) = payload.downcast_ref::<String>() {
        format!("panicked: {s}")
    } else {
        "panicked with a non-string payload".to_string()
    }
}

impl Backend {
    #[cfg(test)]
    fn new(client: Client) -> Self {
        Self::with_shutdown_flag(client, Arc::default())
    }

    fn with_shutdown_flag(
        client: Client,
        shutdown_requested: Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        Self {
            client,
            state: Arc::new(RwLock::new(State::default())),
            refresh_lock: Arc::new(tokio::sync::Mutex::new(())),
            shutdown_requested,
        }
    }

    /// Locate `bynk.toml` walking upward from the given path. Returns the
    /// project root (the directory containing `bynk.toml`) on success.
    fn find_project_root(start: &std::path::Path) -> Option<PathBuf> {
        let mut current = if start.is_file() {
            start.parent()?.to_path_buf()
        } else {
            start.to_path_buf()
        };
        loop {
            let candidate = current.join("bynk.toml");
            if candidate.is_file() {
                return Some(current);
            }
            current = current.parent()?.to_path_buf();
        }
    }

    /// Locate the nearest ancestor directory named `src`, walking upward from
    /// `start`. This is the implicit source root of a *rootless* tree — the
    /// same `src/`-without-`bynk.toml` layout `bynkc` compiles in its legacy
    /// single-tree mode (`bynkc/tests/e2e.rs` `compile_fixture`), which the
    /// compiler fixtures use. Returns that `src` directory.
    fn find_source_root(start: &std::path::Path) -> Option<PathBuf> {
        let mut current = if start.is_file() {
            start.parent()?.to_path_buf()
        } else {
            start.to_path_buf()
        };
        loop {
            if current.file_name().and_then(|n| n.to_str()) == Some("src") {
                return Some(current);
            }
            current = current.parent()?.to_path_buf();
        }
    }

    /// Resolve the analysis root for a path, with its config. A real
    /// `bynk.toml` project (config loaded from disk) takes precedence;
    /// otherwise (#485) fall back to the nearest enclosing `src/` as an
    /// implicit project so a multi-file commons in a rootless tree still
    /// analyses cross-file instead of dropping to sibling-blind single-file
    /// mode. `None` when neither is found — the caller stays single-file.
    fn resolve_root(start: &std::path::Path) -> Option<(PathBuf, project::ProjectConfig)> {
        if let Some(root) = Self::find_project_root(start) {
            let config = project::load_config(&root).unwrap_or_default();
            return Some((root, config));
        }
        // The implicit project root is the parent of `src`: with the default
        // `src_dir` ("src"), `run_project_diagnostics` re-derives exactly this
        // `src` tree as the analysis root, so every project-mode feature works
        // with no further plumbing.
        let src = Self::find_source_root(start)?;
        let root = src.parent()?.to_path_buf();
        Some((root, project::ProjectConfig::default()))
    }

    /// Slice D (Q4): the **canonical** project root that owns `uri`, with its
    /// config, or `None` for a file under no project (single-file mode). Routing
    /// is `resolve_root`'s walk-up — the same project `bynkc` attributes the file
    /// to — canonicalised so it matches the `projects` map key and every
    /// `Analysis.project_root`. Workspace folders do not enter here: a URI routes
    /// by its nearest enclosing `bynk.toml`, whatever folder it sits in.
    fn resolve_canonical(uri: &Url) -> Option<(PathBuf, project::ProjectConfig)> {
        let path = uri.to_file_path().ok()?;
        let (root, config) = Self::resolve_root(&path)?;
        Some((root.canonicalize().unwrap_or(root), config))
    }

    /// The canonical project root owning `uri`, or `None` in single-file mode.
    /// Uncached — walks the filesystem and `canonicalize()`s on every call.
    /// Kept for the one caller that must route off the `state` lock
    /// (`prune_orphaned_projects`, #682 DECISION B) and for tests exercising
    /// routing directly; every other caller wants the memoised
    /// [`Self::root_for_uri`].
    fn root_for_uri_uncached(uri: &Url) -> Option<PathBuf> {
        Self::resolve_canonical(uri).map(|(root, _)| root)
    }

    /// #682: the cached counterpart of `root_for_uri_uncached` — the canonical
    /// project root owning `uri`, memoised in `State.root_cache` so a repeated
    /// request for the same URI does not re-walk the filesystem. A miss runs
    /// the uncached walk and stores the result (`None` included — a file that
    /// routes to no project is itself a stable answer worth caching).
    ///
    /// The walk runs off the `state` lock (it is synchronous filesystem I/O),
    /// so a wholesale `root_cache.clear()` can land between the read that
    /// found the miss and the write that stores its answer — a `bynk.toml`
    /// created mid-walk would otherwise have this write resurrect the
    /// pre-creation (stale) route into the just-cleared cache, and unlike
    /// `prune_orphaned_projects`'s TOCTOU window this one would never
    /// self-heal. `root_cache_generation` closes it: the write-back only
    /// applies if no clear happened while the walk was in flight; otherwise
    /// the fresh answer is simply not cached (correct either way — just an
    /// uncached hit for that one request).
    async fn root_for_uri(&self, uri: &Url) -> Option<PathBuf> {
        let generation = {
            let state = self.state.read().await;
            if let Some(cached) = state.root_cache.get(uri) {
                return cached.clone();
            }
            state.root_cache_generation
        };
        let root = Self::root_for_uri_uncached(uri);
        let mut state = self.state.write().await;
        if Self::root_cache_write_is_current(generation, state.root_cache_generation) {
            state.root_cache.insert(uri.clone(), root.clone());
        }
        root
    }

    /// #682: whether a `root_for_uri` write-back computed while the cache was
    /// at `read_generation` should still be applied, given the cache is now at
    /// `current_generation` — `false` once an invalidating clear has bumped it
    /// past the read, meaning the walk's answer may already be stale. Pulled
    /// out of `root_for_uri` so the guard itself — the one thing standing
    /// between the fix and the TOCTOU it closes — is unit-testable without
    /// needing to actually win the race in real time.
    fn root_cache_write_is_current(read_generation: u64, current_generation: u64) -> bool {
        read_generation == current_generation
    }

    /// Slice E: every project root under `folder` — the folder's own
    /// `resolve_root` (a manifest at or above it, the folder-inside-a-project
    /// case) plus a bounded recursive walk collecting each directory that holds
    /// a `bynk.toml`. Roots are **canonical** (the `projects` map key). The walk
    /// skips the caches and heavy dirs it should never descend (`out`,
    /// `node_modules`, `target`, `.git`, and dot-dirs), and a **visited-set of
    /// canonicalised dirs** stops a symlink cycle (`ln -s . loop`) from recursing
    /// forever. Synchronous FS I/O — callers run it via `spawn_blocking`, off the
    /// executor. This is the "one tree-walk"
    /// [ADR 0204](../decisions/0204-per-workspace-project-state.md) §C named —
    /// shared by startup warming and added-folder warming.
    fn discover_projects_under(folder: &std::path::Path) -> Vec<PathBuf> {
        fn should_skip(name: &std::ffi::OsStr) -> bool {
            let name = name.to_string_lossy();
            matches!(name.as_ref(), "out" | "node_modules" | "target" | ".git")
                || name.starts_with('.')
        }
        fn walk(
            dir: &std::path::Path,
            out: &mut Vec<PathBuf>,
            visited: &mut std::collections::HashSet<PathBuf>,
        ) {
            // Guard against symlink cycles: a directory reached twice (by its
            // canonical path) is not descended again.
            let canon_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
            if !visited.insert(canon_dir.clone()) {
                return;
            }
            if dir.join("bynk.toml").is_file() && !out.contains(&canon_dir) {
                out.push(canon_dir);
            }
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && !should_skip(&entry.file_name()) {
                    walk(&path, out, visited);
                }
            }
        }
        let mut roots = Vec::new();
        // A manifest at or above the folder (the folder sits inside a project).
        if let Some((root, _)) = Self::resolve_root(folder) {
            let canon = root.canonicalize().unwrap_or(root);
            roots.push(canon);
        }
        // The implicit-`src/` shape (#485): a `src/` tree with no `bynk.toml`.
        // `resolve_root` only finds a `src/` *ancestor*, so the folder-is-the-root
        // case (folder holds `src/`, no manifest) needs an explicit check — else
        // a rootless project would warm only lazily on first open, not at startup.
        if folder.join("src").is_dir() && !folder.join("bynk.toml").is_file() {
            let canon = folder
                .canonicalize()
                .unwrap_or_else(|_| folder.to_path_buf());
            if !roots.contains(&canon) {
                roots.push(canon);
            }
        }
        let mut visited = std::collections::HashSet::new();
        walk(folder, &mut roots, &mut visited);
        // #2207: a workspace root isn't a project. Analysing it as one would
        // read every member's sources as a single tree; each member is found
        // by the walk on its own.
        roots.retain(|root| !bynk_ide::is_workspace_root(root));
        roots
    }

    /// Slice F: the single diagnostics-scheduler entry point. Route `uri` to its
    /// owning project (a debounced project round) or, if none, single-file mode
    /// (a debounced buffer `diagnose`). **One** generation-based debounce at the
    /// configured delay covers both — a burst coalesces to one analysis. Replaces
    /// `recompile_and_publish`, whose route + second hardcoded debounce stacked
    /// on `did_change`'s own sleep.
    async fn schedule_diagnostics(&self, uri: &Url) {
        // Slice D: route by URI to the owning project, creating its entry on
        // first touch (a file opened before any folder scan). Q4: the root is
        // the file's nearest enclosing `bynk.toml`, not its workspace folder.
        // #682: routing goes through the cache; the config is only loaded from
        // disk when the entry doesn't exist yet, not on every call.
        if let Some(root) = self.root_for_uri(uri).await {
            {
                let mut state = self.state.write().await;
                if !state.projects.contains_key(&root) {
                    let config = project::load_config(&root).unwrap_or_default();
                    state.projects.insert(
                        root.clone(),
                        ProjectState {
                            config,
                            ..Default::default()
                        },
                    );
                }
            }
            self.schedule_project_diagnostics(root).await;
        } else {
            self.schedule_single_file(uri.clone()).await;
        }
    }

    /// v0.24: debounce a project-wide analysis — each call bumps the project's
    /// generation; the spawned task runs only if still the latest after the
    /// delay, so a typing burst produces one analysis. Slice D: keyed on one
    /// project root, so two projects debounce independently. A no-op if the
    /// root's entry is gone (its folder was removed mid-debounce).
    ///
    /// Slice F: the delay is the project's **configured** `diagnostics_debounce_ms`
    /// (was a hardcoded 200 ms stacked on `did_change`'s own sleep — the two are
    /// now one debounce).
    async fn schedule_project_diagnostics(&self, root: PathBuf) {
        let (generation, debounce) = {
            let mut state = self.state.write().await;
            let Some(ps) = state.projects.get_mut(&root) else {
                return;
            };
            ps.analysis_generation += 1;
            (ps.analysis_generation, ps.config.diagnostics_debounce_ms)
        };
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(debounce)).await;
            let superseded = match this.state.read().await.projects.get(&root) {
                Some(ps) => ps.analysis_generation != generation,
                None => true, // entry pruned — nothing to analyse
            };
            if superseded {
                return;
            }
            this.run_project_diagnostics(root).await;
        });
    }

    /// Slice F: the single-file counterpart to `schedule_project_diagnostics` —
    /// a buffer with no project. Bump the URI's generation, sleep the (default)
    /// configured delay, and run one `diagnose` only if still latest, so a burst
    /// coalesces to one run (before slice F single-file had no generation and ran
    /// once per keystroke).
    async fn schedule_single_file(&self, uri: Url) {
        let debounce = ProjectConfig::default().diagnostics_debounce_ms;
        let generation = {
            let mut state = self.state.write().await;
            let g = state
                .single_file_generations
                .entry(uri.clone())
                .or_insert(0);
            *g += 1;
            *g
        };
        let this = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(debounce)).await;
            let current = this
                .state
                .read()
                .await
                .single_file_generations
                .get(&uri)
                .copied();
            if current != Some(generation) {
                return;
            }
            this.diagnose_single_file(&uri).await;
        });
    }

    /// Slice F: run `bynk_ide::diagnose` on one buffer and publish — the
    /// single-file leaf of the scheduler (extracted from `recompile_and_publish`).
    /// Best-effort: a malformed file produces diagnostics, not a hard failure.
    async fn diagnose_single_file(&self, uri: &Url) {
        let (text, version) = {
            let state = self.state.read().await;
            match state.docs.get(uri) {
                Some(d) => (d.text.clone(), d.version),
                None => return,
            }
        };
        let positions = crate::position::PositionMap::new(&text);
        let lsp_diags: Vec<Diagnostic> = bynk_ide::diagnose(&text)
            .into_iter()
            .map(|d| make_diagnostic(&d, &positions, uri, &ProjectFiles::new()))
            .collect();
        self.client
            .publish_diagnostics(uri.clone(), lsp_diags, Some(version))
            .await;
    }

    /// #1667: an analysis round for `root` failed (`reason` says how). Log
    /// it, and tell the client once per failure streak: the published
    /// diagnostics are the last good round's and may be stale. The streak ends
    /// at the next committed round, which clears the flag.
    async fn report_failed_round(&self, root: &std::path::Path, reason: String) {
        tracing::error!("analysis of {} {reason}", root.display());
        let first_failure = {
            let mut state = self.state.write().await;
            match state.projects.get_mut(root) {
                Some(ps) => !std::mem::replace(&mut ps.analysis_failed, true),
                None => false,
            }
        };
        if first_failure {
            self.client
                .show_message(
                    MessageType::ERROR,
                    format!(
                        "Bynk: analysing {} failed with an internal error, so its diagnostics may \
                         be out of date until a later edit analyses cleanly. Details are in \
                         ~/.bynk-lsp.log; please report it.",
                        root.display()
                    ),
                )
                .await;
        }
    }

    /// #2081: `root`'s `bynk.schema.lock`, through its project's cache. The
    /// cache is copied out, refreshed with no `state` lock held (a `stat`,
    /// and a read only when the file changed), then stored back. A root with
    /// no project entry gets a one-off read. Never writes the lock.
    async fn schema_lock_for(&self, root: &std::path::Path) -> bynk_ide::SchemaLock {
        let mut cache = self
            .state
            .read()
            .await
            .projects
            .get(root)
            .map(|ps| ps.schema_lock.clone())
            .unwrap_or_default();
        let lock = cache.current(root);
        if let Some(ps) = self.state.write().await.projects.get_mut(root) {
            ps.schema_lock = cache;
        }
        lock
    }

    /// v0.24 (ADR 0052): one project-wide diagnostics round — overlay the
    /// open buffers over disk, analyse off the async runtime, convert spans
    /// against the **analysed snapshots**, and publish via the pure
    /// publish-plan (clears included).
    async fn run_project_diagnostics(&self, root: PathBuf) {
        let (round, root, canonical_root, overlay, versions, previously_dirty) = {
            let mut state = self.state.write().await;
            // Slice D: the round is for one project's entry. If it was pruned
            // (its folder removed) between scheduling and now, there is nothing
            // to analyse — bail.
            let Some(ps) = state.projects.get_mut(&root) else {
                return;
            };
            ps.analysis_round_started += 1;
            let round = ps.analysis_round_started;
            // Slice A: the analysis is rooted at the *project*, not at one
            // `include` tree, and every path it returns is project-relative
            // (ADR 0198) — so this is the base the overlay keys against too.
            let canonical_root = root.canonicalize().unwrap_or_else(|_| root.clone());
            let previously_dirty = ps.published.clone();
            let mut overlay = std::collections::HashMap::new();
            let mut versions = std::collections::HashMap::new();
            // Every open buffer overlays disk. A buffer belonging to another
            // project keys to an absolute path outside this root, so it is inert
            // here — discovery never matches it — and its `versions` entry is
            // skipped by the `strip_prefix` guard. So the round stays scoped to
            // this project without filtering the doc set.
            for (uri, doc) in &state.docs {
                if let Ok(p) = uri.to_file_path() {
                    let canonical = p.canonicalize().unwrap_or(p);
                    // v0.25: capture the version the overlay snapshot came
                    // from, keyed project-relative like the analysis output.
                    if let Ok(rel) = canonical.strip_prefix(&canonical_root) {
                        versions.insert(rel.to_path_buf(), doc.version);
                    }
                    overlay.insert(canonical, doc.text.clone());
                }
            }
            (
                round,
                root,
                canonical_root,
                overlay,
                versions,
                previously_dirty,
            )
        };

        // Slice A: manifest-aware, multi-root — the same trees `bynkc` compiles.
        let roots = bynk_ide::AnalysisRoots::Project(root.clone());
        // Content-ownership track (#1086) slice 5: `overlay` above is only the
        // open buffers — with `discovery.rs`'s disk fallback gone, a project's
        // closed files need `sweep_project_content`'s full disk sweep too, or
        // every one of them fails `bynk.project.read_failed` on every round.
        #[cfg(test)]
        let inject_panic = self
            .state
            .write()
            .await
            .projects
            .get_mut(&root)
            .is_some_and(|ps| std::mem::take(&mut ps.panic_next_round));
        // #2081: the committed schema lock, so a non-additive event change is
        // diagnosed here as `bynk check` diagnoses it.
        let schema_lock = self.schema_lock_for(&root).await;
        let joined = tokio::task::spawn_blocking(move || {
            #[cfg(test)]
            if inject_panic {
                panic!("injected analysis panic");
            }
            let content = crate::content::sweep_project_content(&roots, &overlay);
            bynk_ide::diagnose_project_with(&roots, &content, &schema_lock)
        })
        .await;
        let result = match joined {
            Ok(result) => result,
            Err(e) => {
                self.report_failed_round(&root, describe_join_error(e))
                    .await;
                return;
            }
        };

        let mut new_by_uri: std::collections::HashMap<Url, Vec<Diagnostic>> =
            std::collections::HashMap::new();
        // Slice B (DECISION C): the document version each file was analysed at,
        // keyed by URI — so the publish can carry it and the client can drop a
        // range computed against a buffer it has already edited past. `None` for
        // a file read from disk (no open buffer, no version).
        let mut version_by_uri: std::collections::HashMap<Url, Option<i32>> =
            std::collections::HashMap::new();
        let mut snapshots = std::collections::HashMap::new();
        let mut diagnostics: std::collections::HashMap<PathBuf, Vec<bynk_ide::Diagnostic>> =
            std::collections::HashMap::new();
        // Spans convert against the snapshot the analysis saw — never a newer
        // buffer (Settled, v0.24 proposal). Built for every file up front: a
        // diagnostic's secondary label may lie in a sibling file.
        let files: ProjectFiles<'_> = result
            .files
            .iter()
            .filter_map(|file| {
                let abs = canonical_root.join(&file.source_path);
                let abs = abs.canonicalize().unwrap_or(abs);
                let uri = Url::from_file_path(&abs).ok()?;
                let positions = crate::position::PositionMap::new(&file.text);
                Some((file.source_path.clone(), (uri, positions)))
            })
            .collect();
        for file in &result.files {
            let Some((uri, positions)) = files.get(&file.source_path) else {
                continue;
            };
            let diags: Vec<Diagnostic> = file
                .diagnostics
                .iter()
                .map(|d| make_diagnostic(d, positions, uri, &files))
                .collect();
            version_by_uri.insert(uri.clone(), versions.get(&file.source_path).copied());
            new_by_uri.insert(uri.clone(), diags);
            diagnostics.insert(file.source_path.clone(), file.diagnostics.clone());
            snapshots.insert(file.source_path.clone(), file.text.clone());
        }
        // v0.25: retain the round's index + snapshots for references/rename
        // and the binding-correct definition/hover. v0.26: plus the raw
        // diagnostics, for `codeAction` (the suggestions ride on them).
        {
            let analysis = Arc::new(Analysis {
                project_root: canonical_root.clone(),
                index: result.index.clone(),
                snapshots,
                versions,
                diagnostics,
                hints: result.hints,
                requirements: result.requirements,
                locals: result.locals,
                expr_types: result.expr_types,
                ty_intern: result.ty_intern,
                unit_sources: result.unit_sources,
                sequence_info: result.sequence_info,
                boundary_info: result.boundary_info,
                doc_scope: result.doc_scope,
            });
            let mut state = self.state.write().await;
            let Some(ps) = state.projects.get_mut(&root) else {
                return; // pruned mid-round
            };
            // Completion order is not start order: a slow old round finishing
            // after a newer one must be dropped, not committed (#513).
            if ps.analysis_round_committed >= round {
                return;
            }
            ps.analysis_round_committed = round;
            ps.analysis_failed = false;
            ps.analysis = Some(analysis);
        }
        // Project-level diagnostics with no single owning file surface at
        // position 0:0 rather than vanishing — on `bynk.toml` when it exists,
        // else (#485, implicit `src/` mode has no manifest) on the first
        // analysed file, so they anchor to a real, openable document.
        let unattributed_anchor = {
            let toml = root.join("bynk.toml");
            if toml.is_file() {
                Url::from_file_path(toml).ok()
            } else {
                result.files.first().and_then(|f| {
                    let abs = canonical_root.join(&f.source_path);
                    let abs = abs.canonicalize().unwrap_or(abs);
                    Url::from_file_path(abs).ok()
                })
            }
        };
        if !result.unattributed.is_empty()
            && let Some(anchor_uri) = unattributed_anchor
        {
            let entry = new_by_uri.entry(anchor_uri).or_default();
            for d in &result.unattributed {
                entry.push(Diagnostic {
                    range: Default::default(),
                    severity: Some(match d.severity {
                        bynk_syntax::Severity::Error => DiagnosticSeverity::ERROR,
                        bynk_syntax::Severity::Warning => DiagnosticSeverity::WARNING,
                    }),
                    code: Some(tower_lsp::lsp_types::NumberOrString::String(
                        d.error.category.to_string(),
                    )),
                    message: d.error.message.clone(),
                    ..Default::default()
                });
            }
        }

        let (publishes, dirty) = publish::publish_plan(&previously_dirty, new_by_uri);
        for (uri, diags) in publishes {
            // Slice B (DECISION C): stamp the publish with the version the round
            // analysed this file at (was `None`), so a client can reject a range
            // its buffer has moved past. A clearing publish for a now-absent file
            // carries no version — it has no entry in `version_by_uri`.
            let version = version_by_uri.get(&uri).copied().flatten();
            self.client.publish_diagnostics(uri, diags, version).await;
        }
        let still_current = {
            let mut state = self.state.write().await;
            if let Some(ps) = state.projects.get_mut(&root)
                && ps.analysis_round_committed == round
            {
                ps.published = dirty;
                true
            } else {
                false
            }
        };
        // #733: revalidate. Pull-based decorations are served from the committed
        // round (`committed_analysis`) without a forced re-analysis, so a fresh
        // round is invisible to the client until it re-pulls. Nudge it to — but
        // only for this round if a newer one has not already superseded it (that
        // one sends its own nudge), and only for decorations the client can
        // refresh. Fired on a detached task: `run_project_diagnostics` also runs
        // on the *request* path (a cursor request's forced refresh), and a
        // `workspace/*/refresh` awaits a client round-trip — spawning keeps that
        // off the request's critical path. Best-effort: a failed nudge just
        // leaves the client on the previous pull until its next request.
        if still_current {
            let refresh = self.state.read().await.supports_refresh;
            if refresh.semantic_tokens || refresh.inlay_hints || refresh.code_lens {
                let client = self.client.clone();
                tokio::spawn(async move {
                    if refresh.semantic_tokens {
                        let _ = client.semantic_tokens_refresh().await;
                    }
                    if refresh.inlay_hints {
                        let _ = client.inlay_hint_refresh().await;
                    }
                    if refresh.code_lens {
                        let _ = client.code_lens_refresh().await;
                    }
                });
            }
        }
    }

    /// Slice A: the analysis roots for the project that owns `uri` — the
    /// manifest's, resolved by the compiler's own discovery. `None` in
    /// single-file mode (no project root), where cross-file lookups are skipped.
    /// Slice D: routes by URI (Q4), so a completion in project B enumerates B's
    /// units, not the first project's.
    ///
    /// Replaces `project_src_root`, which returned `root.join(config.src_dir)`:
    /// one tree, chosen by reducing `[paths] include` to its first entry and
    /// ignoring `exclude`. That reduction is the defect slice A removed.
    async fn analysis_roots_for(&self, uri: &Url) -> Option<bynk_ide::AnalysisRoots> {
        Some(bynk_ide::AnalysisRoots::Project(
            self.root_for_uri(uri).await?,
        ))
    }

    /// Content-ownership track (#1086): the owning project's `.bynk` files
    /// as a pre-read `(path, content)` map — every open buffer's **live**
    /// text, a real disk read for everything else `bynk_ide::discover_files`
    /// names. `None` in single-file mode. Backs completion, signature help
    /// (slice 0), and the cross-file symbol lookups (slice 1) — this is now
    /// the sole enumeration entry point; the bare-paths `project_files` it
    /// replaced (slice 0's `Backend::project_files`) was deleted once slice 1
    /// migrated its last two callers.
    ///
    /// Finding #62's exclusion carries over unchanged from `project_files`:
    /// the cursor's own file is filtered out here, once, for every caller —
    /// `bynk-ide`'s completion helpers already parse it fresh from the live
    /// buffer (`for_each_unit`'s `doc_text`), so leaving it in the map would
    /// offer a second, stale version of the same file alongside the live one.
    async fn project_content(
        &self,
        uri: &Url,
    ) -> Option<Arc<std::collections::HashMap<PathBuf, String>>> {
        let roots = self.analysis_roots_for(uri).await?;
        let current = uri
            .to_file_path()
            .ok()
            .and_then(|p| std::fs::canonicalize(&p).ok());
        let overlay = {
            let state = self.state.read().await;
            let mut ov = std::collections::HashMap::new();
            for (u, doc) in &state.docs {
                if let Ok(p) = u.to_file_path() {
                    let canonical = p.canonicalize().unwrap_or(p);
                    ov.insert(canonical, doc.text.clone());
                }
            }
            ov
        };
        tokio::task::spawn_blocking(move || {
            let mut content = crate::content::sweep_project_content(&roots, &overlay);
            if let Some(current) = current {
                content.remove(&current);
            }
            // `Arc`, not an owned map: a caller that needs to move this into
            // its own `spawn_blocking` closure (signature help fires on every
            // `(`/`,`) clones a refcount, not every project file's content.
            Arc::new(content)
        })
        .await
        .map_err(|e| tracing::error!("project content sweep {}", describe_join_error(e)))
        .ok()
    }

    /// v0.31: the def + use spans of the local under the cursor (def first), or
    /// `None` if the cursor is not on a local.
    fn local_sites(
        &self,
        analysis: &Analysis,
        rel: &std::path::Path,
        offset: usize,
    ) -> Option<Vec<bynk_syntax::span::Span>> {
        let text = analysis.snapshots.get(rel)?;
        let locals = analysis.locals.get(rel)?;
        crate::locals_nav::local_sites_at(locals, text, offset)
    }

    /// v0.31 (ADR 0064): the in-scope local bindings at the cursor, as
    /// `variable` completions, read from the **cached** analysis — so they
    /// survive the mid-edit buffer the current keystroke produced (the last
    /// good round's bindings around the cursor are what's wanted). Positions
    /// convert against the cached snapshot, like the other cached-round reads.
    async fn locals_completions(&self, uri: &Url, pos: Position) -> Vec<CompletionItem> {
        // Slice B: completion's locals sub-path resolves `pos` against the
        // round's snapshot (like `index_position`), so it refreshes too — the
        // one exposed reader the §4.2 table missed.
        let analysis = self.analysis_for(uri).await;
        let Some(analysis) = analysis else {
            return Vec::new();
        };
        let Some(rel) = Self::uri_to_rel(&analysis, uri) else {
            return Vec::new();
        };
        let (Some(text), Some(locals)) = (analysis.snapshots.get(&rel), analysis.locals.get(&rel))
        else {
            return Vec::new();
        };
        let Some(offset) = crate::position::position_to_offset(text, pos) else {
            return Vec::new();
        };
        bynk_check::locals::locals_at(locals, offset)
            .into_iter()
            .map(|b| CompletionItem {
                label: b.name.clone(),
                kind: Some(CompletionItemKind::VARIABLE),
                detail: Some(b.ty.clone()),
                ..Default::default()
            })
            .collect()
    }

    /// Convert same-file local spans to LSP `Location`s.
    fn local_locations(
        &self,
        analysis: &Analysis,
        rel: &std::path::Path,
        spans: &[bynk_syntax::span::Span],
    ) -> Vec<Location> {
        let Some(text) = analysis.snapshots.get(rel) else {
            return Vec::new();
        };
        let Ok(uri) = Url::from_file_path(analysis.project_root.join(rel)) else {
            return Vec::new();
        };
        spans
            .iter()
            .map(|s| Location {
                uri: uri.clone(),
                range: crate::position::span_to_range(text, *s),
            })
            .collect()
    }

    /// Slice 3 (ADR 0063): complete the members of a typed **value** receiver.
    /// Re-analyses the buffer rewritten so the receiver parses (the trailing
    /// `.partial` dropped), types the receiver via the retained `expr_types`,
    /// and maps its type to kernel methods + record fields. Silent (not
    /// necessarily empty — see below) when the receiver itself has no
    /// recorded type; an error elsewhere in the file no longer silences it
    /// (ADR 0094's partial types lifted the clean-file ceiling).
    ///
    /// #596: additionally merges a bare `store` field receiver's own
    /// vocabulary (entry ops, and for `Map` the `.entries`/`.keys`/`.values`
    /// accessors) — dispatched by receiver *provenance* in the checker, which
    /// the typed `ty` alone can't distinguish from an ordinary `Query`-typed
    /// local (a bare store `Map` widens to `Ty::Query` too, ADR 0120). This
    /// half runs **independently of whether `type_receiver` succeeded**: it
    /// re-parses the buffer itself and needs no typed `ty` at all, so a `store`
    /// field still offers its entry ops/accessors even when the receiver goes
    /// untyped. (Its original motivation — an unresolved name *elsewhere* in
    /// the file bailing the checker before it ran, the one clean-file-ceiling
    /// gap ADR 0094 didn't close — has gone since #1663 let the checker run
    /// past a resolve error.) A review on #812 flagged the earlier draft's
    /// single early return as undercutting that motivation.
    async fn value_member_completions(
        &self,
        uri: &Url,
        text: &str,
        offset: usize,
    ) -> Vec<CompletionItem> {
        let Some((rewritten, recv_offset)) = completion::value_receiver_rewrite(text, offset)
        else {
            return Vec::new();
        };
        let mut items: Vec<CompletionItem> = Vec::new();
        if let Some((ty, tys)) = self
            .type_receiver(uri, rewritten.clone(), recv_offset)
            .await
        {
            let files = self.project_content(uri).await;
            items.extend(
                completion::value_member_candidates(ty, &tys, text, files.as_deref())
                    .into_iter()
                    .map(to_completion_item),
            );
        }
        // #2138: `run.id`, when `run` is the test run (it has no type then).
        if items.is_empty() {
            items.extend(
                completion::run_id_member_candidates(&rewritten, recv_offset)
                    .into_iter()
                    .map(to_completion_item),
            );
        }
        let locals = self.fast_path_locals(uri, &rewritten).await;
        items.extend(
            completion::store_field_member_candidates(&rewritten, recv_offset, &locals)
                .into_iter()
                .map(to_completion_item),
        );
        items
    }

    /// #596: the current analysed round's locals for `uri`, only when its
    /// snapshot exactly matches `rewritten` — the same fast-path match
    /// [`Self::type_receiver`] uses. Empty (rather than forcing a synchronous
    /// re-analysis) when the round is stale or absent, so the store-field
    /// shadowing check degrades to "no local shadows the name".
    async fn fast_path_locals(
        &self,
        uri: &Url,
        rewritten: &str,
    ) -> Vec<bynk_check::locals::LocalBinding> {
        let Some(analysis) = self.project_analysis_for(uri).await else {
            return Vec::new();
        };
        let Some(rel) = Self::uri_to_rel(&analysis, uri) else {
            return Vec::new();
        };
        if analysis.snapshots.get(&rel).map(String::as_str) != Some(rewritten) {
            return Vec::new();
        }
        analysis.locals.get(&rel).cloned().unwrap_or_default()
    }

    /// v0.124 (slice 3): at `<expr> is <cursor>`, the scrutinee sum type's
    /// variants. The scrutinee is typed via `expr_types` (re-analysing through
    /// `type_receiver`, the value-member path), so it is subject to the clean-
    /// file ceiling and goes silent — never wrong — on a broken buffer.
    async fn is_pattern_completions(
        &self,
        uri: &Url,
        text: &str,
        offset: usize,
    ) -> Vec<CompletionItem> {
        let Some(scrut_off) = is_scrutinee_offset(text, offset) else {
            return Vec::new();
        };
        self.scrutinee_variant_completions(uri, text, scrut_off)
            .await
    }

    /// v0.128: at an arm-pattern-start inside a `match <expr> { … }`, the
    /// scrutinee sum type's variants — the deferred half of slice 3's
    /// `is`-pattern completion, sharing its scrutinee typing and candidate set.
    async fn match_arm_completions(
        &self,
        uri: &Url,
        text: &str,
        offset: usize,
    ) -> Vec<CompletionItem> {
        let Some(scrut_off) = match_scrutinee_offset(text, offset) else {
            return self.let_else_match_completions(uri, text, offset).await;
        };
        self.scrutinee_variant_completions(uri, text, scrut_off)
            .await
    }

    /// #2010: at an arm-pattern-start inside `let <pattern> (= | <-) <value>
    /// else match { … }`, the variants of the value's type (its `Effect`
    /// payload for `<-`) that the `let` pattern leaves for the arms to cover.
    async fn let_else_match_completions(
        &self,
        uri: &Url,
        text: &str,
        offset: usize,
    ) -> Vec<CompletionItem> {
        let Some(head) = let_else_match_head(text, offset) else {
            return Vec::new();
        };
        // Just past the value's last character: an offset *on* it would also
        // sit at the end of its innermost sub-expression (the `o` in
        // `Effect.pure(o)`), which `type_at_offset` prefers as the smaller span.
        let Some((ty, tys)) = self
            .type_receiver(uri, text.to_string(), head.value_end + 1)
            .await
        else {
            return Vec::new();
        };
        let ty = if head.effectful {
            match &*tys.get(ty) {
                bynk_check::checker::Ty::Effect(inner) => *inner,
                _ => return Vec::new(),
            }
        } else {
            ty
        };
        let files = self.project_content(uri).await;
        completion::variants_for_ty(ty, &tys, text, files.as_deref())
            .into_iter()
            .filter(|c| head.covered.as_deref() != Some(c.label.as_str()))
            .map(to_completion_item)
            .collect()
    }

    /// The variants of the scrutinee whose last character is at `scrut_off` — the
    /// shared tail of `is`/`match` pattern completion. Types the scrutinee via
    /// `expr_types` (ADR 0094's partial types on a broken buffer; silent when
    /// the scrutinee itself has no recorded type) and offers its variants;
    /// empty for a non-sum, non-`Result`/`Option`
    /// scrutinee. v0.145 (ADR 0169): `Result`/`Option` scrutinees now fire too
    /// (`variants_for_ty`), not only user-declared sums.
    async fn scrutinee_variant_completions(
        &self,
        uri: &Url,
        text: &str,
        scrut_off: usize,
    ) -> Vec<CompletionItem> {
        let Some((ty, tys)) = self.type_receiver(uri, text.to_string(), scrut_off).await else {
            return Vec::new();
        };
        let files = self.project_content(uri).await;
        completion::variants_for_ty(ty, &tys, text, files.as_deref())
            .into_iter()
            .map(to_completion_item)
            .collect()
    }

    /// v0.145 (ADR 0169): at `OuterVariant(‸` inside a match arm-pattern, the
    /// payload field type's variants — e.g. `Ok`/`Err` inside `Some(‸)` on an
    /// `Option[Result[…]]` scrutinee. `match_scrutinee_offset` deliberately bails
    /// on a nested constructor; `nested_pattern_offset` targets exactly it,
    /// yielding the scrutinee offset and the outer variant. Types the scrutinee
    /// via the same `type_receiver` path and resolves the payload type.
    async fn nested_pattern_completions(
        &self,
        uri: &Url,
        text: &str,
        offset: usize,
    ) -> Vec<CompletionItem> {
        let Some((scrut_off, variant)) = nested_pattern_offset(text, offset) else {
            return Vec::new();
        };
        let Some((ty, tys)) = self.type_receiver(uri, text.to_string(), scrut_off).await else {
            return Vec::new();
        };
        let files = self.project_content(uri).await;
        completion::nested_variant_completions(ty, &tys, &variant, text, files.as_deref())
            .into_iter()
            .map(to_completion_item)
            .collect()
    }

    /// v0.32 (ADR 0065): the type of a receiver expression at `recv_offset` in a
    /// buffer `rewritten` so it parses — re-analyse the overlay and query the
    /// retained `expr_types`. Shared by value-member completion and signature
    /// help; `None` when the receiver has no recorded type. A file with errors
    /// elsewhere still types it (ADR 0094's best-effort partial types).
    async fn type_receiver(
        &self,
        uri: &Url,
        rewritten: String,
        recv_offset: usize,
    ) -> Option<(
        bynk_check::checker::TyId,
        std::sync::Arc<bynk_check::checker::Types>,
    )> {
        // T3.6b (R4.1): the id is meaningless without the table it was minted
        // from, so both travel together — the round's own table on the fast
        // path, the fresh re-analysis's on the slow one.
        let roots = self.analysis_roots_for(uri).await?;
        let project_root = roots.project_root().to_path_buf();
        let canonical_root = project_root
            .canonicalize()
            .unwrap_or_else(|_| project_root.clone());
        let cur = uri.to_file_path().ok()?;
        let cur = cur.canonicalize().unwrap_or(cur);
        // Slice A: project-relative, matching the round's identity (ADR 0198).
        let rel = cur.strip_prefix(&canonical_root).ok()?.to_path_buf();
        // Overlay every open doc, with this one rewritten so it parses.
        let overlay = {
            let state = self.state.read().await;
            let mut ov = std::collections::HashMap::new();
            for (u, doc) in &state.docs {
                if let Ok(p) = u.to_file_path() {
                    let canonical = p.canonicalize().unwrap_or(p);
                    let t = if u == uri {
                        rewritten.clone()
                    } else {
                        doc.text.clone()
                    };
                    ov.insert(canonical, t);
                }
            }
            ov
        };
        // Fast path (#513): completion fires on every `.` keystroke, and the
        // rewritten buffer (the trailing `.`-segment removed so it parses) is
        // usually byte-identical to the snapshot the last debounced round
        // analysed. Reuse that round's expression types instead of running a
        // synchronous whole-project re-analysis on the request path.
        if let Some(analysis) = self.project_analysis_for(uri).await
            && analysis.snapshots.get(&rel).map(String::as_str) == Some(rewritten.as_str())
            && let Some((_, entries)) = analysis.expr_types.iter().find(|(p, _)| **p == rel)
        {
            return bynk_check::expr_types::type_at_offset(
                entries,
                recv_offset,
                &analysis.ty_intern,
            )
            .map(|t| (t, std::sync::Arc::clone(&analysis.ty_intern)));
        }
        // Content-ownership track (#1086) slice 5: same complete-content
        // requirement as `run_project_diagnostics` — `overlay` here is only
        // the open buffers, and `discovery.rs`'s disk fallback is gone.
        // #2081: `Off` — this re-analysis only reads expression types, which
        // the schema lock cannot change, so it skips the event sweep.
        let result = tokio::task::spawn_blocking(move || {
            let content = crate::content::sweep_project_content(&roots, &overlay);
            bynk_ide::diagnose_project_with(&roots, &content, &bynk_ide::SchemaLock::Off)
        })
        .await
        .map_err(|e| tracing::error!("receiver-typing analysis {}", describe_join_error(e)))
        .ok()?;
        let (_, entries) = result.expr_types.iter().find(|(p, _)| **p == rel)?;
        bynk_check::expr_types::type_at_offset(entries, recv_offset, &result.ty_intern)
            .map(|t| (t, std::sync::Arc::clone(&result.ty_intern)))
    }

    /// Slice D: the committed analysis for one project root, ungated — the raw
    /// last round, or `None` if the root has no entry or has not analysed yet.
    async fn project_analysis(&self, root: &std::path::Path) -> Option<Arc<Analysis>> {
        self.state.read().await.projects.get(root)?.analysis.clone()
    }

    /// The owning project's committed analysis for `uri`, ungated. For callers
    /// that reuse a round opportunistically (completion's receiver-typing fast
    /// path); the freshness gate is [`Self::analysis_for`].
    async fn project_analysis_for(&self, uri: &Url) -> Option<Arc<Analysis>> {
        let root = self.root_for_uri(uri).await?;
        self.project_analysis(&root).await
    }

    /// Ensure `root` has an entry (created with `config` if absent) and a
    /// committed analysis (one round run if none yet), and return it. For the
    /// cross-project workspace-symbol scan, which must answer over every project
    /// including ones no request has warmed. `None` if the round produced none.
    async fn ensure_project_analysed(
        &self,
        root: PathBuf,
        config: ProjectConfig,
    ) -> Option<Arc<Analysis>> {
        {
            let mut state = self.state.write().await;
            state
                .projects
                .entry(root.clone())
                .or_insert_with(|| ProjectState {
                    config,
                    ..Default::default()
                });
        }
        if let Some(a) = self.project_analysis(&root).await {
            return Some(a);
        }
        self.refresh_now(root.clone()).await;
        self.project_analysis(&root).await
    }

    /// Slice D (Q4 lifecycle): drop every project no longer reachable from a
    /// workspace folder **and** holding no open buffer, clearing its published
    /// diagnostics. A project is retained while some remaining folder relates to
    /// it (one is a path-prefix of the other — a file under that folder can still
    /// route to the root) or while any open buffer routes to it. Shared by the
    /// two events that can orphan a project: a folder leaving
    /// (`did_change_workspace_folders`) and its last buffer closing (`did_close`)
    /// — a project falls only when *both* its seed and its buffers are gone.
    /// Returns the URIs whose diagnostics were cleared so the caller can publish
    /// the clears (done outside the lock).
    async fn prune_orphaned_projects(&self) -> Vec<Url> {
        // #733: `root_for_uri_uncached` canonicalises and walks the filesystem
        // up to a `bynk.toml` for every open buffer — syscalls that must not run
        // while holding `state.write()`. Snapshot the inputs under a short read
        // lock, resolve the open roots off the lock, then take the write lock
        // only to mutate `projects`.
        //
        // #682 (DECISION B): this stays on the *uncached* router rather than
        // `root_for_uri` — pruning is not hot (it fires only on folder-removal
        // or close), and it runs inside a synchronous `filter_map` off the
        // lock, where an async, cache-consulting router can't be called inline
        // without either re-locking `state` here (defeating the point of
        // computing `open_roots` off-lock) or restructuring this into an async
        // stream. This opens a small TOCTOU window: `orphaned` is
        // computed against live `state.projects` under the write lock but against
        // the *snapshot's* `folders`/`open_roots`, so a `did_open` that lands in
        // between — newly covering a root — is not yet in `open_roots` and that
        // root could be pruned here. It is self-healing: the pruning callers
        // (`did_close`, `did_change_workspace_folders`) only ever *remove*
        // coverage, so a racing `did_open` re-creates the entry the moment that
        // buffer routes/analyses (`schedule_diagnostics` → a lazily-created
        // `ProjectState`) — its diagnostics clear-then-repopulate, never a
        // permanently-dropped project.
        let (folders, open_uris) = {
            let state = self.state.read().await;
            (
                state.folders.clone(),
                state.docs.keys().cloned().collect::<Vec<_>>(),
            )
        };
        let open_roots: std::collections::HashSet<PathBuf> = open_uris
            .iter()
            .filter_map(Self::root_for_uri_uncached)
            .collect();
        let covered = |root: &std::path::Path| {
            folders
                .iter()
                .any(|f| f.starts_with(root) || root.starts_with(f))
                || open_roots.contains(root)
        };
        let mut state = self.state.write().await;
        let orphaned: Vec<PathBuf> = state
            .projects
            .keys()
            .filter(|r| !covered(r))
            .cloned()
            .collect();
        let mut to_clear = Vec::new();
        for root in orphaned {
            if let Some(ps) = state.projects.remove(&root) {
                to_clear.extend(ps.published);
            }
        }
        to_clear
    }

    /// Slice E: discover and warm every project under `folders` — create each
    /// entry (idempotent, keyed by canonical root) and schedule its round — so a
    /// workspace shows diagnostics without a file being opened. Non-blocking:
    /// entries are created synchronously (routing is immediately correct) and the
    /// rounds run on the debounce path. Shared by `initialized` (all folders) and
    /// the `did_change_workspace_folders` added branch (the new folders).
    async fn warm_projects(&self, folders: &[PathBuf]) {
        if folders.is_empty() {
            return;
        }
        // Discover off the lock **and** off the executor: the walk is synchronous
        // FS I/O, so run it on a blocking thread rather than stalling an async
        // worker while a workspace tree is scanned.
        let folders = folders.to_vec();
        let roots = tokio::task::spawn_blocking(move || {
            let mut roots: Vec<PathBuf> = Vec::new();
            for folder in &folders {
                for root in Self::discover_projects_under(folder) {
                    if !roots.contains(&root) {
                        roots.push(root);
                    }
                }
            }
            roots
        })
        .await
        .unwrap_or_else(|e| {
            tracing::error!("project discovery {}", describe_join_error(e));
            Vec::new()
        });
        for root in roots {
            let config = project::load_config(&root).unwrap_or_default();
            {
                let mut state = self.state.write().await;
                state
                    .projects
                    .entry(root.clone())
                    .or_insert_with(|| ProjectState {
                        config,
                        ..Default::default()
                    });
            }
            self.schedule_project_diagnostics(root).await;
        }
    }

    /// Slice E: register the `workspace/didChangeWatchedFiles` capability with
    /// the client — once, with folder-independent globs (`**/*.bynk`,
    /// `**/bynk.toml`, and since #2081 `**/bynk.schema.lock`), per Q4 (ADR
    /// 0204 §D). So a client that supports dynamic
    /// registration is notified of source and manifest changes without watching
    /// files itself. Best-effort: a registration failure is logged, not fatal.
    async fn register_file_watchers(&self) {
        use tower_lsp::lsp_types::{
            DidChangeWatchedFilesRegistrationOptions, FileSystemWatcher, GlobPattern, Registration,
        };
        let watchers = ["**/*.bynk", "**/bynk.toml", "**/bynk.schema.lock"]
            .into_iter()
            .map(|g| FileSystemWatcher {
                glob_pattern: GlobPattern::String(g.to_string()),
                kind: None, // create | change | delete
            })
            .collect();
        let registration = Registration {
            id: "bynk-watched-files".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                watchers,
            })
            .ok(),
        };
        if let Err(e) = self.client.register_capability(vec![registration]).await {
            self.client
                .log_message(
                    MessageType::WARNING,
                    format!("bynkc-lsp: file-watcher registration failed: {e}"),
                )
                .await;
        }
    }

    /// The `bynk.toml` config governing `uri` — its project's, or the default
    /// (single-file mode). Backs the per-file diagnostics mode/debounce and the
    /// formatting options, which now differ by project.
    async fn config_for(&self, uri: &Url) -> ProjectConfig {
        let Some(root) = self.root_for_uri(uri).await else {
            return ProjectConfig::default();
        };
        self.state
            .read()
            .await
            .projects
            .get(&root)
            .map(|p| p.config.clone())
            .unwrap_or_default()
    }

    /// Slice B — the freshness contract (Q3, settled #663). The analysis a
    /// request must answer from, **current for `uri`**: cold start triggers a
    /// round; a round that predates `uri`'s buffer triggers a refresh.
    ///
    /// The client's request position refers to `uri`'s current document
    /// version — messages are ordered, so `docs[uri].version` reflects every
    /// `didChange` sent before the request. The returned analysis is guaranteed
    /// to have analysed *that* version of `uri`, so `position_to_offset` against
    /// its snapshot is never resolved against text the user edited past.
    ///
    /// Slice D: routes to the project that owns `uri` (Q4) before gating, so the
    /// freshness check is against *that* project's round. A file under no
    /// project (single-file mode) is never index-answerable — decline.
    ///
    /// Returns `None` — decline, per Q3 — only when the request cannot be
    /// answered at the version the client holds: single-file mode (no project),
    /// a file outside every `include` root (never a snapshot key), or a
    /// concurrent edit that moved past the refresh (rare; the next request is
    /// current). Never returns an analysis whose snapshot for `uri` is stale.
    async fn analysis_for(&self, uri: &Url) -> Option<Arc<Analysis>> {
        let root = self.root_for_uri(uri).await?;
        // The version the request's position is stated against. `None` when the
        // file is not an open buffer — then any round is as authoritative as it
        // gets (nothing newer to be stale against), so the freshness gate is a
        // no-op and only cold start matters.
        let want = self.state.read().await.docs.get(uri).map(|d| d.version);
        let current = |a: &Arc<Analysis>| {
            let Some(rel) = Self::uri_to_rel(a, uri) else {
                return false; // unmappable URI — cannot be answered
            };
            // The file must actually be *analysed* (a snapshot key), not merely
            // have a version entry: `versions` is built from open docs, so a
            // file open but outside every `include` root has a version and no
            // snapshot. Such a file is never answerable — decline.
            if !a.snapshots.contains_key(&rel) {
                return false;
            }
            match want {
                // Open buffer: the analysed snapshot must be at the client's
                // version, or the position resolves against text edited past.
                Some(v) => a.versions.get(&rel) == Some(&v),
                // Not an open buffer (a closed/disk file, e.g. a goto target):
                // the analysed round is authoritative — nothing newer to lag.
                None => true,
            }
        };

        if let Some(a) = self.project_analysis(&root).await
            && current(&a)
        {
            return Some(a);
        }

        // Refresh. The lock serialises concurrent requests: the first runs the
        // round, the rest wait and then find it already current below — so N
        // requests after one edit share one round, not N. (One lock across all
        // projects is fine — a refresh holds it only across its own round.)
        let _guard = self.refresh_lock.lock().await;
        if let Some(a) = self.project_analysis(&root).await
            && current(&a)
        {
            return Some(a);
        }
        self.refresh_now(root.clone()).await;
        let a = self.project_analysis(&root).await?;
        // Strict: only answer if the fresh round is actually current for `uri`.
        // An edit that landed during the round leaves us behind — decline, and
        // the next request refreshes again. Never a position against stale text.
        current(&a).then_some(a)
    }

    /// #733 — the non-refreshing gate for **pull-based decoration requests**
    /// (`semanticTokens`, `inlayHint`, `codeLens`, `documentLink`, `codeAction`).
    /// Returns the last committed round for `uri`'s project **as-is**, without
    /// forcing a synchronous re-analysis on the request path.
    ///
    /// Why this is safe where [`Self::analysis_for`] is not: these handlers
    /// resolve nothing against the client's *live* cursor — every range and span
    /// they emit converts against the round's own `snapshots` (or, for
    /// `document_link`, against live text plus the project-level `unit_sources`
    /// map). So a committed round lagging the buffer by at most one debounce
    /// cycle is internally consistent; the strict version match `analysis_for`
    /// demands is stronger than a decoration needs. The editor auto-fires these
    /// on every `didChange`, so forcing a whole-project round here is exactly
    /// what defeated the debounce (#733).
    ///
    /// This is stale-while-revalidate: serve the committed round now; the
    /// already-scheduled debounce round is the revalidation, and on its commit
    /// [`Self::run_project_diagnostics`] nudges the client to re-pull via
    /// `workspace/*/refresh`. Cursor requests keep the strict gate.
    ///
    /// `None` — the handler returns empty — when the file is under no project, is
    /// outside every `include` root (never a snapshot key), or no round has
    /// committed yet (cold start; the scheduled round will produce one and the
    /// client re-pulls on the refresh nudge).
    async fn committed_analysis(&self, uri: &Url) -> Option<Arc<Analysis>> {
        let root = self.root_for_uri(uri).await?;
        let a = self.project_analysis(&root).await?;
        // Must actually be analysed (a snapshot key), not merely version-tracked
        // — the handler converts its spans against this snapshot.
        let rel = Self::uri_to_rel(&a, uri)?;
        a.snapshots.contains_key(&rel).then_some(a)
    }

    /// #2100 (DECISION B): the organise-imports action for `rel`, offered
    /// only if its edit re-checks clean. The full organise (drops included)
    /// is tried first; if a drop would introduce a diagnostic, the sort-and-
    /// dedupe-only edit is tried instead, so one wrongly-judged drop doesn't
    /// withhold the whole action.
    async fn organise_action(
        &self,
        analysis: &Arc<Analysis>,
        rel: &std::path::Path,
        uri: &Url,
        version: Option<i32>,
    ) -> Option<CodeActionOrCommand> {
        use crate::organise::{Mode, Project};
        let text = analysis.snapshots.get(rel)?;
        let project = Project {
            index: &analysis.index,
            snapshots: &analysis.snapshots,
            unit_sources: &analysis.unit_sources,
        };
        let mut tried: Option<String> = None;
        for mode in [Mode::Full, Mode::SortOnly] {
            let Some(edit) = crate::organise::organise(text, rel, mode, &project) else {
                continue;
            };
            let edited = crate::organise::apply(text, &edit);
            if tried.as_deref() == Some(edited.as_str()) {
                continue;
            }
            if self.rechecks_clean(analysis, rel, edited.clone()).await {
                return Some(crate::organise::action(text, edit, uri, version));
            }
            tried = Some(edited);
        }
        None
    }

    /// #2100 slice 1: resolve a listed inline action — re-plan it against the
    /// committed round (which must still be the version it was listed from)
    /// and offer the edit only if the inlined project re-checks clean.
    async fn resolve_inline(
        &self,
        action: CodeAction,
        target: crate::inline::Target,
    ) -> JsonRpcResult<CodeAction> {
        let refused = |msg: &str| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::InvalidParams,
            message: msg.to_string().into(),
            data: None,
        };
        let stale = "the document changed since the inline was offered";
        let analysis = self
            .committed_analysis(&target.uri)
            .await
            .ok_or_else(|| refused(stale))?;
        let rel = Self::uri_to_rel(&analysis, &target.uri).ok_or_else(|| refused(stale))?;
        let (Some(text), Some(locals)) = (analysis.snapshots.get(&rel), analysis.locals.get(&rel))
        else {
            return Err(refused(stale));
        };
        let version = analysis.versions.get(&rel).copied();
        if version != target.version {
            return Err(refused(stale));
        }
        let plan = crate::inline::plan(text, target.def, locals)
            .ok_or_else(|| refused("this binding can't be inlined"))?;
        let inlined = crate::fix_all::apply(text, &plan.edits);
        if !self.rechecks_clean(&analysis, &rel, inlined).await {
            return Err(refused(&format!(
                "inlining `{}` would introduce a new problem",
                plan.name
            )));
        }
        Ok(crate::fill_arms::resolve(
            action,
            text,
            &plan.edits,
            &target.uri,
            version,
        ))
    }

    /// #2100 slice 4: resolve a listed provider-stub action — re-plan it
    /// against the committed round (which must still be the version it was
    /// listed from) and offer the edit only if the project re-checks clean.
    async fn resolve_provider_stub(
        &self,
        action: CodeAction,
        target: crate::provider_stub::Target,
    ) -> JsonRpcResult<CodeAction> {
        let refused = |msg: &str| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::InvalidParams,
            message: msg.to_string().into(),
            data: None,
        };
        let stale = "the document changed since the provider stub was offered";
        let analysis = self
            .committed_analysis(&target.uri)
            .await
            .ok_or_else(|| refused(stale))?;
        let rel = Self::uri_to_rel(&analysis, &target.uri).ok_or_else(|| refused(stale))?;
        let Some(text) = analysis.snapshots.get(&rel) else {
            return Err(refused(stale));
        };
        let version = analysis.versions.get(&rel).copied();
        if version != target.version {
            return Err(refused(stale));
        }
        let plan = crate::provider_stub::plan(text, &target.capability, &analysis.index)
            .ok_or_else(|| refused("a provider stub can't be generated here"))?;
        let stubbed = crate::fix_all::apply(text, &plan.edits);
        if !self.rechecks_clean(&analysis, &rel, stubbed).await {
            return Err(refused(&format!(
                "a provider stub for `{}` would introduce a new problem",
                plan.capability
            )));
        }
        Ok(crate::fill_arms::resolve(
            action,
            text,
            &plan.edits,
            &target.uri,
            version,
        ))
    }

    /// #2100 slice 2: resolve a listed extract-function action — re-plan it
    /// against the committed round (which must still be the version it was
    /// listed from) and offer the edit only if the project re-checks clean.
    async fn resolve_extract_function(
        &self,
        action: CodeAction,
        target: crate::extract::FunctionTarget,
    ) -> JsonRpcResult<CodeAction> {
        let refused = |msg: &str| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::InvalidParams,
            message: msg.to_string().into(),
            data: None,
        };
        let stale = "the document changed since the extraction was offered";
        let analysis = self
            .committed_analysis(&target.uri)
            .await
            .ok_or_else(|| refused(stale))?;
        let rel = Self::uri_to_rel(&analysis, &target.uri).ok_or_else(|| refused(stale))?;
        let Some(text) = analysis.snapshots.get(&rel) else {
            return Err(refused(stale));
        };
        let version = analysis.versions.get(&rel).copied();
        if version != target.version {
            return Err(refused(stale));
        }
        let empty_reqs = Vec::new();
        let empty_locals = Vec::new();
        let empty_types = Vec::new();
        let plan = crate::extract::plan_function(
            text,
            target.requested,
            analysis.requirements.get(&rel).unwrap_or(&empty_reqs),
            analysis.locals.get(&rel).unwrap_or(&empty_locals),
            analysis.expr_types.get(&rel).unwrap_or(&empty_types),
            &analysis.ty_intern,
        )
        .map_err(|r| {
            refused(
                r.reason
                    .as_deref()
                    .unwrap_or("the selection can't be extracted into a function"),
            )
        })?;
        let extracted = crate::fix_all::apply(text, &plan.edits);
        if !self.rechecks_clean(&analysis, &rel, extracted).await {
            return Err(refused(&format!(
                "extracting `{}` would introduce a new problem",
                plan.name
            )));
        }
        Ok(crate::fill_arms::resolve(
            action,
            text,
            &plan.edits,
            &target.uri,
            version,
        ))
    }

    /// #2100 (DECISION B): re-analyse the round's project with `rel`'s text
    /// replaced by `edited`, and report whether that introduces no diagnostic
    /// the round didn't already have (rename's validator 1, for one file).
    /// Every other file is pinned to its round snapshot, so the re-analysis
    /// differs from the round only by the edit.
    async fn rechecks_clean(
        &self,
        analysis: &Arc<Analysis>,
        rel: &std::path::Path,
        edited: String,
    ) -> bool {
        let mut overlay = std::collections::HashMap::new();
        for (rel_path, text) in &analysis.snapshots {
            let abs = analysis.project_root.join(rel_path);
            let abs = abs.canonicalize().unwrap_or(abs);
            let text = if rel_path == rel {
                edited.clone()
            } else {
                text.clone()
            };
            overlay.insert(abs, text);
        }
        let roots = bynk_ide::AnalysisRoots::Project(analysis.project_root.clone());
        // #2081: `Off`, as rename's re-check: schema-lock diagnostics are
        // project-level and never compared here.
        let Ok(post) = tokio::task::spawn_blocking(move || {
            bynk_ide::diagnose_project_with(&roots, &overlay, &bynk_ide::SchemaLock::Off)
        })
        .await
        else {
            return false;
        };
        let post_diags: Vec<(PathBuf, String)> = post
            .files
            .iter()
            .flat_map(|f| {
                f.diagnostics
                    .iter()
                    .map(|d| (f.source_path.clone(), d.error.category.to_string()))
            })
            .collect();
        crate::index_queries::no_new_diagnostics(&analysis.diag_categories(), &post_diags).is_ok()
    }

    /// Slice B: the analysis for a handler that emits **multi-file versioned
    /// edits** — today, `rename`. Per-URI freshness ([`Self::analysis_for`]) is
    /// not enough here: a rename touches every file that references the symbol,
    /// and each edit is stamped with *that* file's analysed version, so the
    /// round must be current for **every open buffer**, not just the cursor's.
    ///
    /// Without this, a buffer edited since the last round but not under the
    /// cursor keeps its stale version in the round; `rename`'s edit for it is
    /// then stamped with that old version and the client rejects the whole
    /// operation (VS Code: "document changed since the refactoring was
    /// requested"). This restores the whole-project guarantee the pre-v0.179
    /// `fresh_analysis` gave — as a version-aware refresh, not an unconditional
    /// one. Returns `None` on the same terms as `analysis_for` (no project, or a
    /// concurrent edit that raced the refresh).
    ///
    /// Slice D: takes the rename's project `root` — a rename spans one project
    /// (the symbol and its references live under one root), so the round must
    /// cover *that* project's open buffers. A buffer in another project strips
    /// against a different `project_root`, so `uri_to_rel` returns `None` for it
    /// and it does not gate this rename.
    async fn analysis_covering_open_buffers(
        &self,
        root: &std::path::Path,
    ) -> Option<Arc<Analysis>> {
        // Every open buffer that maps into the project must be analysed at its
        // current version. A buffer outside the project (no snapshot key) is not
        // part of a project rename and does not gate it.
        let all_current =
            |a: &Arc<Analysis>, docs: &std::collections::HashMap<Url, DocumentState>| {
                docs.iter()
                    .all(|(uri, doc)| match Self::uri_to_rel(a, uri) {
                        Some(rel) if a.snapshots.contains_key(&rel) => {
                            a.versions.get(&rel) == Some(&doc.version)
                        }
                        _ => true,
                    })
            };

        {
            let state = self.state.read().await;
            if let Some(a) = state.projects.get(root).and_then(|p| p.analysis.clone())
                && all_current(&a, &state.docs)
            {
                return Some(a);
            }
        }
        let _guard = self.refresh_lock.lock().await;
        {
            let state = self.state.read().await;
            if let Some(a) = state.projects.get(root).and_then(|p| p.analysis.clone())
                && all_current(&a, &state.docs)
            {
                return Some(a);
            }
        }
        self.refresh_now(root.to_path_buf()).await;
        let state = self.state.read().await;
        let a = state.projects.get(root).and_then(|p| p.analysis.clone())?;
        all_current(&a, &state.docs).then_some(a)
    }

    /// Run a round now for one project, superseding any pending debounced one.
    /// Bumping the project's generation makes a scheduled round (which checks it
    /// before running) bail, so a request-driven refresh does not race a
    /// redundant debounce round that would produce the same result 200 ms later.
    async fn refresh_now(&self, root: PathBuf) {
        if let Some(ps) = self.state.write().await.projects.get_mut(&root) {
            ps.analysis_generation += 1;
        }
        self.run_project_diagnostics(root).await;
    }

    /// Map a request URI to the analysis' project-relative path.
    fn uri_to_rel(analysis: &Analysis, uri: &Url) -> Option<PathBuf> {
        let p = uri.to_file_path().ok()?;
        let canonical = p.canonicalize().unwrap_or(p);
        // Slice A: one `strip_prefix` still, but against the *project* root —
        // which is total across `include` trees, where the old `src` base could
        // only ever name files in one of them. A file under no root strips fine
        // and simply misses every lookup, which is correct: it was not analysed.
        canonical
            .strip_prefix(&analysis.project_root)
            .ok()
            .map(|r| r.to_path_buf())
    }

    /// #302: like [`Self::uri_to_rel`], but for a URI whose file does not
    /// exist yet — `willRenameFiles`' `new_uri`, named before the physical
    /// move happens. `Path::canonicalize` requires the path to exist, so
    /// `uri_to_rel`'s fallback (`unwrap_or(p)`, dead code for every other
    /// caller, which only ever resolves existing files) would silently keep
    /// the client's raw, non-canonical path — mismatching `project_root`
    /// (always canonical) whenever the workspace sits behind a symlink (macOS
    /// `/tmp` → `/private/tmp` being the common case), and the rename would
    /// quietly produce no edit. Canonicalizing the *parent* directory
    /// instead — it does exist — and rejoining the file name sidesteps that.
    fn uri_to_rel_for_new_path(analysis: &Analysis, uri: &Url) -> Option<PathBuf> {
        let p = uri.to_file_path().ok()?;
        let file_name = p.file_name()?;
        let parent = p.parent()?;
        let canonical_parent = parent
            .canonicalize()
            .unwrap_or_else(|_| parent.to_path_buf());
        canonical_parent
            .join(file_name)
            .strip_prefix(&analysis.project_root)
            .ok()
            .map(|r| r.to_path_buf())
    }

    /// #2105: the per-file renames one `FileRename` stands for, as
    /// project-relative `(old, new)` paths. A path the round has a snapshot
    /// for is a file rename. Anything else is treated as a folder: every
    /// snapshot under it (a component-wise prefix match, so `src/bill` never
    /// matches `src/billing/…`), each mapped to the same relative position
    /// under the new folder. A folder with no project file under it, or an
    /// old/new path outside the project, yields nothing. Sorted, so the edit
    /// set is deterministic.
    fn expand_rename(analysis: &Analysis, old_uri: &Url, new_uri: &Url) -> Vec<(PathBuf, PathBuf)> {
        let (Some(old_rel), Some(new_rel)) = (
            Self::uri_to_rel(analysis, old_uri),
            Self::uri_to_rel_for_new_path(analysis, new_uri),
        ) else {
            return Vec::new();
        };
        if analysis.snapshots.contains_key(&old_rel) {
            return vec![(old_rel, new_rel)];
        }
        let mut pairs: Vec<(PathBuf, PathBuf)> = analysis
            .snapshots
            .keys()
            .filter_map(|rel| {
                let suffix = rel.strip_prefix(&old_rel).ok()?;
                (!suffix.as_os_str().is_empty()).then(|| (rel.clone(), new_rel.join(suffix)))
            })
            .collect();
        pairs.sort();
        pairs
    }

    /// #2105 (DECISION B): the canonical project root a rename's destination
    /// would belong to. The destination does not exist yet (`willRenameFiles`
    /// fires before the move), so its existing parent is canonicalized and the
    /// last segment rejoined, as [`Self::uri_to_rel_for_new_path`] does, before
    /// the usual walk up to a `bynk.toml` (or implicit `src/` parent).
    fn new_path_root(new_uri: &Url) -> Option<PathBuf> {
        let p = new_uri.to_file_path().ok()?;
        let parent = p.parent()?;
        let canonical = parent
            .canonicalize()
            .unwrap_or_else(|_| parent.to_path_buf())
            .join(p.file_name()?);
        let (root, _) = Self::resolve_root(&canonical)?;
        Some(root.canonicalize().unwrap_or(root))
    }

    /// Slice 6a follow-up (ADR 0095): if `pos` sits on a `uses`/`consumes` unit
    /// name, the location of that unit's source (its first file, at the top —
    /// units aren't index symbols, so there is no finer def span to land on).
    /// Spans come from the live buffer; the target from the round's unit→source
    /// map. `None` for a first-party/unresolved unit or a non-unit position.
    async fn unit_reference_definition(&self, uri: &Url, pos: Position) -> Option<Location> {
        // Slice B: the position is resolved against *live* text (no stale-offset
        // risk), but the `uses`/`consumes` → source lookup reads the round's
        // `unit_sources`, so route that through the gate — fresh or decline,
        // never a stale unit map. Cheap here: `goto_definition` already
        // refreshed via `index_position`, so this hits the current-round path.
        let analysis = self.analysis_for(uri).await;
        let text = self
            .state
            .read()
            .await
            .docs
            .get(uri)
            .map(|d| d.text.clone());
        let (text, analysis) = (text?, analysis?);
        let offset = cursor_offset(&text, pos);
        for (unit, span) in crate::symbols::unit_reference_spans(&text) {
            if span.start <= offset && offset <= span.end {
                // #2223: a dependency unit is keyed by its canonical name,
                // resolved against the live content map (an unsaved alias
                // edit counts, as it does for the analysed round).
                let view = if analysis.unit_sources.contains_key(&unit) {
                    None
                } else {
                    let files = self.project_content(uri).await;
                    let empty = std::collections::HashMap::new();
                    bynk_ide::packages::DependencyView::for_root(
                        &analysis.project_root,
                        files.as_deref().unwrap_or(&empty),
                    )
                };
                let rel = bynk_ide::packages::unit_sources_for(
                    &analysis.unit_sources,
                    view.as_ref(),
                    &unit,
                )?
                .first()?;
                let target = Url::from_file_path(analysis.project_root.join(rel)).ok()?;
                return Some(Location {
                    uri: target,
                    range: Range::default(),
                });
            }
        }
        None
    }

    /// Convert an index site to an LSP location, spans against the analysed
    /// snapshot (v0.24 rule).
    fn site_to_location(
        analysis: &Analysis,
        site: &bynk_check::index::SiteRef,
    ) -> Option<Location> {
        let text = analysis.snapshots.get(&site.path)?;
        let abs = analysis.project_root.join(&site.path);
        let uri = Url::from_file_path(abs).ok()?;
        Some(Location {
            uri,
            range: crate::position::span_to_range(text, site.span),
        })
    }

    /// v0.34 (ADR 0067): build a `CallHierarchyItem` for an index symbol from
    /// its key + definition site. The key is round-tripped through `data` so
    /// the incoming/outgoing follow-ups resolve straight off it, never
    /// re-inferring from a position.
    fn call_hierarchy_item(
        analysis: &Analysis,
        key: &bynk_check::index::SymbolKey,
        def: &bynk_check::index::SiteRef,
    ) -> Option<CallHierarchyItem> {
        let location = Self::site_to_location(analysis, def)?;
        Some(CallHierarchyItem {
            name: key.name.clone(),
            kind: lsp_symbol_kind(key.kind),
            tags: None,
            detail: Some(key.unit.clone()),
            uri: location.uri,
            range: location.range,
            selection_range: location.range,
            data: serde_json::to_value(SerKey::from(key)).ok(),
        })
    }

    /// The call-site ranges (`fromRanges`) for a call relation, each converted
    /// against its file's analysed snapshot.
    fn call_ranges(analysis: &Analysis, sites: &[&bynk_check::index::SiteRef]) -> Vec<Range> {
        sites
            .iter()
            .filter_map(|s| {
                let text = analysis.snapshots.get(&s.path)?;
                Some(crate::position::span_to_range(text, s.span))
            })
            .collect()
    }

    /// v0.28 (ADR 0057): the shared body of both semantic-tokens requests —
    /// resolve the cached round, convert the optional range against the
    /// analysed snapshot, and run the pure producer. Empty when no round is
    /// cached or the file is outside the project.
    async fn semantic_tokens_for(&self, uri: &Url, range: Option<Range>) -> Vec<SemanticToken> {
        // #733: serve the last committed round without forcing a re-analysis —
        // tokens convert against the round's own snapshot, so a one-cycle lag is
        // consistent, and the client re-pulls on the round-commit refresh nudge.
        let analysis = self.committed_analysis(uri).await;
        let Some(analysis) = analysis else {
            return Vec::new();
        };
        let Some(rel) = Self::uri_to_rel(&analysis, uri) else {
            return Vec::new();
        };
        let Some(text) = analysis.snapshots.get(&rel) else {
            return Vec::new();
        };
        let span = match range {
            None => None,
            // The requested range converts against the analysed snapshot,
            // like the spans it is intersected with.
            Some(r) => {
                let (Some(start), Some(end)) = (
                    crate::position::position_to_offset(text, r.start),
                    crate::position::position_to_offset(text, r.end),
                ) else {
                    return Vec::new();
                };
                Some(bynk_syntax::span::Span::new(start, end))
            }
        };
        let lt = analysis
            .locals
            .get(&rel)
            .map(|l| crate::locals_nav::local_token_sites(l, text))
            .unwrap_or_default();
        // v0.140 (ADR 0163): handler-annotation spans (`@cache` name + argument
        // labels), classified as `decorator`. Parsed from the snapshot here, off
        // the index-read path (mirroring how locals are precomputed).
        let dt = crate::symbols::handler_annotation_token_spans(text);
        crate::index_queries::semantic_tokens(&analysis.index, &lt, &dt, &rel, text, span)
    }

    /// The (analysis, rel-path, snapshot byte offset) for a request
    /// position — the shared front half of every index-backed handler.
    async fn index_position(
        &self,
        uri: &Url,
        position: Position,
    ) -> Option<(Arc<Analysis>, PathBuf, usize)> {
        // Slice B: `analysis_for` guarantees the round analysed `uri`'s current
        // version, so `position_to_offset` resolves against the same text the
        // client's position refers to — the `fresh` flag every caller used to
        // pass is gone (freshness is the contract now, not a per-call choice).
        let analysis = self.analysis_for(uri).await?;
        let rel = Self::uri_to_rel(&analysis, uri)?;
        let text = analysis.snapshots.get(&rel)?;
        let offset = crate::position::position_to_offset(text, position)?;
        Some((analysis, rel, offset))
    }

    /// Locate the AST node at the given cursor position by re-parsing the
    /// document. Returns the textual identifier (if any) and its span.
    /// Used by hover and definition handlers.
    async fn identifier_at(
        &self,
        uri: &Url,
        position: Position,
    ) -> Option<(String, bynk_syntax::span::Span, String)> {
        let text = {
            let state = self.state.read().await;
            state.docs.get(uri)?.text.clone()
        };
        let offset = crate::position::position_to_offset(&text, position)?;
        // Hole-aware (issue #473): interpolation holes are expanded so a cursor
        // inside `"… \(name) …"` lands on the hole's identifier token, not the
        // opaque `InterpStr` token.
        let tokens = bynk_syntax::lexer::tokenize_expanding_holes(&text).ok()?;
        // Find the token whose span covers `offset`.
        for t in &tokens {
            if t.span.start <= offset
                && offset < t.span.end
                && matches!(
                    t.kind,
                    bynk_syntax::lexer::TokenKind::Ident
                        | bynk_syntax::lexer::TokenKind::Int
                        | bynk_syntax::lexer::TokenKind::String
                        | bynk_syntax::lexer::TokenKind::Bool
                        | bynk_syntax::lexer::TokenKind::Float
                        | bynk_syntax::lexer::TokenKind::Result
                        | bynk_syntax::lexer::TokenKind::Option
                        | bynk_syntax::lexer::TokenKind::Effect
                )
            {
                let name = text[t.span.start..t.span.end].to_string();
                return Some((name, t.span, text));
            }
        }
        None
    }

    /// #846: `bynk/sequenceModel` — the sequence-diagram query for the
    /// handler under the cursor. This server's first custom (non-standard)
    /// request, registered via `custom_method` in [`run`] rather than a
    /// `LanguageServer` trait slot. Served from the committed round (#733),
    /// like `code_lens`; no refresh nudge (see the `sequence_request` module
    /// doc for why one isn't needed).
    async fn sequence_model(
        &self,
        params: sequence_request::SequenceModelParams,
    ) -> JsonRpcResult<Option<sequence_request::WireSequenceModel>> {
        let uri = params.text_document.uri;
        let Some(analysis) = self.committed_analysis(&uri).await else {
            return Ok(None);
        };
        let Some(rel) = Self::uri_to_rel(&analysis, &uri) else {
            return Ok(None);
        };
        let Some(text) = analysis.snapshots.get(&rel) else {
            return Ok(None);
        };
        let Some(offset) = crate::position::position_to_offset(text, params.position) else {
            return Ok(None);
        };
        let info = bynk_ide::symbols::own_declaration_name(text)
            .and_then(|(name, _)| analysis.sequence_info.get(&name));
        let model = sequence_request::sequence_model_at(text, offset, info);
        Ok(model.map(|m| sequence_request::to_wire(&m, text)))
    }

    /// #2196: `bynk/agentStateModel` — the agent state view's query for the
    /// agent enclosing the cursor: its state machines, derived from its
    /// handlers. Agent-scoped (the cursor anywhere in the `agent`), served from
    /// the committed round, no refresh nudge — like `sequence_model`. A
    /// non-project file, no committed round, an offset outside every agent, or
    /// a unit `boundary_info` has no entry for all answer `None`.
    async fn agent_state_model(
        &self,
        params: agent_state_request::AgentStateModelParams,
    ) -> JsonRpcResult<Option<agent_state_request::WireAgentStateModel>> {
        let uri = params.text_document.uri;
        let Some(analysis) = self.committed_analysis(&uri).await else {
            return Ok(None);
        };
        let Some(rel) = Self::uri_to_rel(&analysis, &uri) else {
            return Ok(None);
        };
        let Some(text) = analysis.snapshots.get(&rel) else {
            return Ok(None);
        };
        let Some(offset) = crate::position::position_to_offset(text, params.position) else {
            return Ok(None);
        };
        let Some((unit, _)) = bynk_ide::symbols::own_declaration_name(text) else {
            return Ok(None);
        };
        let Some(info) = analysis.boundary_info.get(&unit) else {
            return Ok(None);
        };
        // The agents a handler can call: the context's own, across its files.
        let agents: std::collections::HashSet<String> = analysis
            .sequence_info
            .get(&unit)
            .map(|s| s.agents.keys().cloned().collect())
            .unwrap_or_default();
        Ok(
            agent_state_request::agent_state_at(text, offset, &info.types).map(|(agent, model)| {
                let effects = agent_state_request::effects_of(&agent, &agents);
                agent_state_request::to_wire(&agent, &model, &effects, text)
            }),
        )
    }

    /// #847: `bynk/documentationModel` — the documentation-view query for the
    /// unit under the request. This server's second custom request,
    /// registered via `custom_method` in [`run`] (like `sequence_model`).
    /// Served from the committed round (#733), on-demand: no cursor position
    /// (the page is the whole unit — every file of a multi-file unit since
    /// #1885) and no refresh nudge (Decision D — see the
    /// `documentation_request` module doc, and #846's for why a custom method
    /// needs none). A non-project file / no committed round → `None` (empty
    /// page).
    async fn documentation_model(
        &self,
        params: documentation_request::DocumentationModelParams,
    ) -> JsonRpcResult<Option<documentation_request::WireDocModel>> {
        let uri = params.text_document.uri;
        let Some(analysis) = self.committed_analysis(&uri).await else {
            return Ok(None);
        };
        let Some(rel) = Self::uri_to_rel(&analysis, &uri) else {
            return Ok(None);
        };
        Ok(documentation_request::documentation_model_for(
            &rel,
            &analysis.snapshots,
            &analysis.unit_sources,
            &analysis.project_root,
        ))
    }

    /// #2098: `bynk/explain` — the curated, offline explanation of a diagnostic
    /// code, from the toolchain's own compiled-in table (so it always matches
    /// the installed compiler). Not document-scoped and reads no analysis
    /// round: the code alone is the key. `None` for an uncurated or unknown
    /// code. Registered via `custom_method` in [`run`].
    async fn explain(
        &self,
        params: explain_request::ExplainParams,
    ) -> JsonRpcResult<Option<explain_request::WireExplanation>> {
        Ok(explain_request::explanation_for(&params.code))
    }

    /// #2471: `bynk/emittedOutput` — the generated TypeScript for the file,
    /// with its source map decoded. Compiles the committed round's snapshots in
    /// memory, off the async runtime (it re-runs the checker), and only when
    /// the client asks. A non-project file, no committed round, or a project
    /// that does not compile answers `None`.
    async fn emitted_output(
        &self,
        params: emitted_request::EmittedOutputParams,
    ) -> JsonRpcResult<Option<emitted_request::WireEmittedOutput>> {
        let uri = params.text_document.uri;
        let Some(analysis) = self.committed_analysis(&uri).await else {
            return Ok(None);
        };
        let Some(rel) = Self::uri_to_rel(&analysis, &uri) else {
            return Ok(None);
        };
        Ok(tokio::task::spawn_blocking(move || {
            emitted_request::emitted_output_for(&analysis.project_root, &analysis.snapshots, &rel)
        })
        .await
        .unwrap_or(None))
    }

    /// #851: `bynk/architectureModel` — the whole-project architecture-map
    /// query. This server's third custom request, registered via
    /// `custom_method` in [`run`] (like `sequence_model`/`documentation_model`).
    /// Served from the committed round (#733); no refresh nudge, for the same
    /// reason neither sibling needs one. Unlike both siblings this is
    /// **project-scoped** — `params.text_document` only resolves which
    /// project's round to read (via `committed_analysis`); the result covers
    /// every context/adapter unit in that round, not just the request's own
    /// file. A non-project file / no committed round → `None` (empty map).
    async fn architecture_model(
        &self,
        params: architecture_request::ArchitectureModelParams,
    ) -> JsonRpcResult<Option<architecture_request::WireArchModel>> {
        let uri = params.text_document.uri;
        let Some(analysis) = self.committed_analysis(&uri).await else {
            return Ok(None);
        };
        let model = architecture_request::architecture_model_for(
            &analysis.unit_sources,
            &analysis.snapshots,
            &analysis.sequence_info,
        );
        Ok(Some(architecture_request::to_wire(
            &model,
            &analysis.project_root,
            &analysis.snapshots,
        )))
    }

    /// #855: `bynk/wireContract` — the wire-contract peek for the handler
    /// under the cursor. This server's fourth custom request, modelled on
    /// `sequence_model` (file-scoped + position, not project-scoped like
    /// `architecture_model` — the panel is per-handler). Served from the
    /// committed round; no refresh nudge, for the same reason no custom
    /// request needs one. A non-project file, no committed round, an offset
    /// outside any handler, or a unit `boundary_info` has no entry for (the
    /// pipeline bailed before the checker) all answer `None` (`null` on the
    /// wire).
    async fn wire_contract(
        &self,
        params: wire_contract_request::WireContractParams,
    ) -> JsonRpcResult<Option<wire_contract_request::WcModel>> {
        let uri = params.text_document.uri;
        let Some(analysis) = self.committed_analysis(&uri).await else {
            return Ok(None);
        };
        let tys = &analysis.ty_intern;
        let Some(rel) = Self::uri_to_rel(&analysis, &uri) else {
            return Ok(None);
        };
        let Some(text) = analysis.snapshots.get(&rel) else {
            return Ok(None);
        };
        let Some(offset) = crate::position::position_to_offset(text, params.position) else {
            return Ok(None);
        };
        // The owning unit — same `own_declaration_name` convention
        // `sequence_model` uses to key `sequence_info`.
        let Some((unit, _)) = bynk_ide::symbols::own_declaration_name(text) else {
            return Ok(None);
        };
        let Some(info) = analysis.boundary_info.get(&unit) else {
            return Ok(None);
        };
        let expr_types = analysis
            .expr_types
            .get(&rel)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let context_count = bynk_ide::wire_contract::real_context_count(
            &analysis.boundary_info,
            &analysis.unit_sources,
        );
        let Some(model) = bynk_ide::wire_contract::wire_contract_at(
            &unit,
            text,
            offset,
            info,
            expr_types,
            tys,
            context_count,
        ) else {
            return Ok(None);
        };
        // #848's own search order (self, then `uses`, then `consumes`) —
        // reused rather than re-derived, since it is exactly the priority a
        // boundary type name resolves through in `ContextBoundaryInfo::types`.
        let search_order = analysis
            .doc_scope
            .get(&unit)
            .cloned()
            .unwrap_or_else(|| vec![unit.clone()]);
        Ok(Some(wire_contract_request::to_wire(
            &model,
            &analysis.project_root,
            text,
            &info.types,
            &analysis.index,
            &analysis.snapshots,
            &search_order,
        )))
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> JsonRpcResult<InitializeResult> {
        // Slice D (Q4): record **every** workspace folder as a discovery seed
        // (was `folders.first()` only). Folders do not own URIs — a request
        // routes by its nearest enclosing `bynk.toml` (`resolve_root`) — so this
        // seeds where `did_change_workspace_folders` prunes and where slice E's
        // startup scan looks. Slice E: also capture whether the client accepts a
        // server-side `didChangeWatchedFiles` registration, used in `initialized`.
        let dynamic_watchers = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.did_change_watched_files.as_ref())
            .and_then(|d| d.dynamic_registration)
            .unwrap_or(false);
        // #733: whether the client can be nudged to re-pull each pull-based
        // decoration after a round commits (the "revalidate" of stale-while-
        // revalidate). Absent → the flag stays false and no nudge is sent.
        let ws = params.capabilities.workspace.as_ref();
        let supports_refresh = RefreshSupport {
            semantic_tokens: ws
                .and_then(|w| w.semantic_tokens.as_ref())
                .and_then(|s| s.refresh_support)
                .unwrap_or(false),
            inlay_hints: ws
                .and_then(|w| w.inlay_hint.as_ref())
                .and_then(|i| i.refresh_support)
                .unwrap_or(false),
            code_lens: ws
                .and_then(|w| w.code_lens.as_ref())
                .and_then(|c| c.refresh_support)
                .unwrap_or(false),
        };
        {
            let mut state = self.state.write().await;
            state.supports_dynamic_watchers = dynamic_watchers;
            state.supports_refresh = supports_refresh;
            state.supports_explain_command = explain_request::client_supports_explain_command(
                params.capabilities.experimental.as_ref(),
            );
            state.supports_code_action_disabled = params
                .capabilities
                .text_document
                .as_ref()
                .and_then(|t| t.code_action.as_ref())
                .and_then(|c| c.disabled_support)
                .unwrap_or(false);
            if let Some(folders) = &params.workspace_folders {
                state.folders = folders
                    .iter()
                    .filter_map(|f| f.uri.to_file_path().ok())
                    .map(|p| p.canonicalize().unwrap_or(p))
                    .collect();
            }
        }
        Ok(InitializeResult {
            capabilities: server_capabilities(),
            server_info: Some(ServerInfo {
                name: SERVER_NAME.into(),
                version: Some(SERVER_VERSION.into()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {
        let (folders, dynamic) = {
            let s = self.state.read().await;
            (s.folders.clone(), s.supports_dynamic_watchers)
        };
        // Slice E (Q4/ADR 0204 §D): register the file watchers server-side, once,
        // with folder-independent globs — so any client is notified, and the VS
        // Code extension no longer supplies them (avoiding a double
        // notification). Only when the client accepts dynamic registration;
        // otherwise it is expected to watch files itself.
        if dynamic {
            self.register_file_watchers().await;
        }
        // Slice E: warm every project under the workspace folders, so diagnostics
        // appear at activation without a file being opened (spec §2.3).
        self.warm_projects(&folders).await;
        let msg = if folders.is_empty() {
            "bynkc-lsp: no workspace folders; single-file mode".to_string()
        } else {
            format!(
                "bynkc-lsp: {} workspace folder(s); projects resolved per file",
                folders.len()
            )
        };
        self.client.log_message(MessageType::INFO, msg).await;
    }

    async fn shutdown(&self) -> JsonRpcResult<()> {
        self.shutdown_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        {
            let mut state = self.state.write().await;
            state.docs.insert(
                uri.clone(),
                DocumentState {
                    text: params.text_document.text,
                    version: params.text_document.version,
                },
            );
        }
        // Slice D/F: `schedule_diagnostics` routes the URI to its project and
        // creates the entry on first touch — no separate root-setting step.
        self.schedule_diagnostics(&uri).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri.clone();
        {
            let mut state = self.state.write().await;
            if let Some(doc) = state.docs.get_mut(&uri)
                && let Some(change) = params.content_changes.into_iter().next_back()
            {
                doc.text = change.text;
                doc.version = params.text_document.version;
            }
        }
        // `[lsp] diagnostics_mode = "on_save"`: no per-keystroke rounds — the
        // buffer state is updated above and diagnosis waits for `didSave`.
        // Slice D: the mode is the *owning project's* (config differs per
        // project); a single-file buffer uses the defaults.
        if self.config_for(&uri).await.diagnostics_mode == crate::project::DiagnosticsMode::OnSave {
            return;
        }
        // Slice F: hand off to the one scheduler — it debounces once, at the
        // configured delay (no manual pre-sleep stacked on the round's own
        // debounce), and coalesces a burst to a single analysis.
        self.schedule_diagnostics(&uri).await;
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        // The live path already diagnosed on change; this matters for
        // `diagnostics_mode = "on_save"`, where saves are the only trigger.
        self.schedule_diagnostics(&params.text_document.uri).await;
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        {
            let mut state = self.state.write().await;
            state.docs.remove(&uri);
            // Slice F: drop the buffer's single-file debounce generation (a no-op
            // for a project file, which never had one).
            state.single_file_generations.remove(&uri);
        }
        // Slice D (Q4 §C): closing the last buffer can orphan a project whose
        // folder was already removed — it was retained *because* a buffer held
        // it. Prune it now and clear its diagnostics, the mirror of the folder
        // path, so a fully-orphaned project never lingers with stale squiggles.
        for cleared in self.prune_orphaned_projects().await {
            self.client
                .publish_diagnostics(cleared, Vec::new(), None)
                .await;
        }
    }

    /// Transport only: resolve the position, gather the round's tables and the
    /// live buffer, and package the result. The resolution *order* — which is the
    /// behaviour — lives in [`crate::hover::hover_content`], so it has one
    /// definition a test can pin (ADR 0190; #611's gap B was a fall-through bug).
    async fn hover(&self, params: HoverParams) -> JsonRpcResult<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        // The analysed round, positioned — absent for a file outside it.
        let positioned = self.index_position(&uri, pos).await;
        // The live buffer — absent when the document is not open. Distinct from
        // the snapshot above, which lags while the user types.
        let doc_text = {
            let state = self.state.read().await;
            state.docs.get(&uri).map(|d| d.text.clone())
        };
        let doc = doc_text
            .as_deref()
            .and_then(|t| Some((t, crate::position::position_to_offset(t, pos)?)));
        let files = self.project_content(&uri).await;
        let analysis = positioned
            .as_ref()
            .map(|(a, rel, offset)| crate::hover::HoverAnalysis {
                index: &a.index,
                snapshots: &a.snapshots,
                locals: &a.locals,
                expr_types: &a.expr_types,
                tys: &a.ty_intern,
                rel,
                offset: *offset,
                project_root: &a.project_root,
                doc_scope: &a.doc_scope,
                boundary_info: &a.boundary_info,
                // #855: computed once per round, not re-derived as a bare
                // `boundary_info.len()` — see `real_context_count`'s doc.
                context_count: bynk_ide::wire_contract::real_context_count(
                    &a.boundary_info,
                    &a.unit_sources,
                ),
            });
        let content = crate::hover::hover_content(&crate::hover::HoverInput {
            analysis,
            doc,
            uri: &uri,
            files: files.as_deref(),
        });
        Ok(content.map(|value| Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            }),
            range: None,
        }))
    }

    /// v0.32 (ADR 0065): signature help for the call under the cursor.
    async fn signature_help(
        &self,
        params: SignatureHelpParams,
    ) -> JsonRpcResult<Option<SignatureHelp>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let text = {
            let s = self.state.read().await;
            s.docs.get(&uri).map(|d| d.text.clone())
        };
        let Some(text) = text else { return Ok(None) };
        let offset = cursor_offset(&text, pos);
        let Some(ctx) = crate::signature_help::call_context(&text, offset) else {
            return Ok(None);
        };
        let files = self.project_content(&uri).await;
        // Name callees (free fns, statics, capability ops, of/unsafe) — lexical.
        // #733: `resolve_label` enumerates the project's units (file stats +
        // recovery parse of the cache-missed ones), so run it on the blocking
        // pool — signature help fires on every `(`/`,` while typing a call.
        let resolved_label = {
            let callee = ctx.callee.clone();
            let text = text.clone();
            let files = files.clone();
            match tokio::task::spawn_blocking(move || {
                crate::signature_help::resolve_label(&callee, &text, files.as_deref())
            })
            .await
            {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("signature-help label task failed: {e}");
                    None
                }
            }
        };
        let label = match resolved_label {
            Some(l) => Some(l),
            // v0.32 slice 2: a value-receiver method (`xs.fold(`) — type the
            // receiver via the rewrite + re-analyse, then the kernel signature.
            None => match crate::signature_help::value_receiver_method(&ctx.callee) {
                Some((_, method)) => {
                    if let Some((rewritten, recv_offset)) =
                        crate::signature_help::value_receiver_rewrite(
                            &text,
                            &ctx.callee,
                            ctx.open_paren,
                            offset,
                        )
                        && let Some((ty, tys)) =
                            self.type_receiver(&uri, rewritten, recv_offset).await
                    {
                        crate::signature_help::kernel_method_signature(ty, &tys, method)
                    } else {
                        None
                    }
                }
                None => None,
            },
        };
        let Some(label) = label else { return Ok(None) };
        let active = ctx.active_param as u32;
        let parameters: Vec<ParameterInformation> = crate::signature_help::param_ranges(&label)
            .into_iter()
            .map(|(s, e)| ParameterInformation {
                label: ParameterLabel::LabelOffsets([s as u32, e as u32]),
                documentation: None,
            })
            .collect();
        Ok(Some(SignatureHelp {
            signatures: vec![SignatureInformation {
                label,
                documentation: None,
                parameters: Some(parameters),
                active_parameter: Some(active),
            }],
            active_signature: Some(0),
            active_parameter: Some(active),
        }))
    }

    /// v0.33 (ADR 0066): a reference-count lens above each top-level definition,
    /// clickable to peek the references. Served from the cached round.
    async fn code_lens(&self, params: CodeLensParams) -> JsonRpcResult<Option<Vec<CodeLens>>> {
        let uri = params.text_document.uri;
        // #733: committed round, no forced re-analysis (see `committed_analysis`).
        let analysis = self.committed_analysis(&uri).await;
        let Some(analysis) = analysis else {
            return Ok(Some(Vec::new()));
        };
        let Some(rel) = Self::uri_to_rel(&analysis, &uri) else {
            return Ok(Some(Vec::new()));
        };
        let Some(text) = analysis.snapshots.get(&rel) else {
            return Ok(Some(Vec::new()));
        };
        // Peek the references/providers on click — a standard client command,
        // so no extension support is required (the client middleware hydrates the
        // three-argument shape). Shared by both the reference and provider lenses.
        let show_references = |range: Range, locations: Vec<Location>, title: String| CodeLens {
            range,
            command: Some(Command {
                title,
                command: "editor.action.showReferences".to_string(),
                arguments: Some(vec![
                    serde_json::to_value(&uri).unwrap_or_default(),
                    serde_json::to_value(range.start).unwrap_or_default(),
                    serde_json::to_value(&locations).unwrap_or_default(),
                ]),
            }),
            data: None,
        };
        let mut lenses: Vec<CodeLens> = crate::index_queries::code_lenses(&analysis.index, &rel)
            .into_iter()
            .map(|(def, refs)| {
                let range = crate::position::span_to_range(text, def.span);
                let locations: Vec<Location> = refs
                    .iter()
                    .filter_map(|r| Self::site_to_location(&analysis, r))
                    .collect();
                let n = refs.len();
                show_references(
                    range,
                    locations,
                    format!("{n} reference{}", if n == 1 { "" } else { "s" }),
                )
            })
            .collect();
        // v0.127 (editor-currency slice 6): a `N provider(s)` lens on each
        // capability, listing the services that `provides` it. Stacks below the
        // reference lens, as a referenced test stacks a reference + test lens.
        lenses.extend(
            crate::index_queries::capability_provider_lenses(&analysis.index, &rel)
                .into_iter()
                .map(|(def, providers)| {
                    let range = crate::position::span_to_range(text, def.span);
                    let locations: Vec<Location> = providers
                        .iter()
                        .filter_map(|r| Self::site_to_location(&analysis, r))
                        .collect();
                    let n = providers.len();
                    show_references(
                        range,
                        locations,
                        format!("{n} provider{}", if n == 1 { "" } else { "s" }),
                    )
                }),
        );
        // v0.129 (#259): a `N refinements of <Base>` lens on each refined/opaque
        // type, listing its family — every type over the same builtin base. Stacks
        // below the reference lens, like the provider lens on a capability.
        lenses.extend(
            crate::index_queries::refinement_family_lenses(&analysis.index, &rel)
                .into_iter()
                .map(|(def, base, family)| {
                    let range = crate::position::span_to_range(text, def.span);
                    let locations: Vec<Location> = family
                        .iter()
                        .filter_map(|r| Self::site_to_location(&analysis, r))
                        .collect();
                    let n = family.len();
                    show_references(
                        range,
                        locations,
                        format!("{n} refinements of {}", base.name()),
                    )
                }),
        );
        // #846: a "Show Sequence" lens above every handler declaration —
        // `bynk.showSequenceDiagram` is a plain extension command (not a
        // built-in VS Code command), so its arguments travel as plain JSON
        // with no `codelens.ts` hydration needed, unlike `show_references`
        // above. A direct AST walk (`handler_lens_sites`), not
        // `index_queries::code_lenses` — that only indexes agent handlers
        // (`SymbolKind::Handler`; service handlers have no per-handler name)
        // and would silently drop the lens for every service handler.
        lenses.extend(
            crate::sequence_request::handler_lens_sites(text)
                .into_iter()
                .map(|span| {
                    let range = crate::position::span_to_range(text, span);
                    CodeLens {
                        range,
                        command: Some(Command {
                            title: "Show Sequence".to_string(),
                            command: "bynk.showSequenceDiagram".to_string(),
                            arguments: Some(vec![
                                serde_json::to_value(&uri).unwrap_or_default(),
                                serde_json::to_value(range.start).unwrap_or_default(),
                            ]),
                        }),
                        data: None,
                    }
                }),
        );
        // #2196: a "Show State" lens on every agent's name — the state view is
        // agent-scoped, so one lens per agent, not per handler.
        lenses.extend(
            crate::agent_state_request::agent_lens_sites(text)
                .into_iter()
                .map(|span| {
                    let range = crate::position::span_to_range(text, span);
                    CodeLens {
                        range,
                        command: Some(Command {
                            title: "Show State".to_string(),
                            command: "bynk.showAgentState".to_string(),
                            arguments: Some(vec![
                                serde_json::to_value(&uri).unwrap_or_default(),
                                serde_json::to_value(range.start).unwrap_or_default(),
                            ]),
                        }),
                        data: None,
                    }
                }),
        );
        Ok(Some(lenses))
    }

    async fn prepare_call_hierarchy(
        &self,
        params: CallHierarchyPrepareParams,
    ) -> JsonRpcResult<Option<Vec<CallHierarchyItem>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some((analysis, rel, offset)) = self.index_position(&uri, pos).await else {
            return Ok(None);
        };
        let Some((key, def)) =
            crate::index_queries::prepare_call_hierarchy(&analysis.index, &rel, offset)
        else {
            return Ok(None);
        };
        Ok(Self::call_hierarchy_item(&analysis, key, def).map(|item| vec![item]))
    }

    async fn incoming_calls(
        &self,
        params: CallHierarchyIncomingCallsParams,
    ) -> JsonRpcResult<Option<Vec<CallHierarchyIncomingCall>>> {
        let analysis = self.analysis_for(&params.item.uri).await;
        let Some(analysis) = analysis else {
            return Ok(Some(Vec::new()));
        };
        let Some(key) = SerKey::read(&params.item.data) else {
            return Ok(Some(Vec::new()));
        };
        let calls = crate::index_queries::incoming_calls(&analysis.index, &key)
            .into_iter()
            .filter_map(|rel| {
                let from = Self::call_hierarchy_item(&analysis, rel.key, rel.def)?;
                let from_ranges = Self::call_ranges(&analysis, &rel.sites);
                Some(CallHierarchyIncomingCall { from, from_ranges })
            })
            .collect();
        Ok(Some(calls))
    }

    async fn outgoing_calls(
        &self,
        params: CallHierarchyOutgoingCallsParams,
    ) -> JsonRpcResult<Option<Vec<CallHierarchyOutgoingCall>>> {
        let analysis = self.analysis_for(&params.item.uri).await;
        let Some(analysis) = analysis else {
            return Ok(Some(Vec::new()));
        };
        let Some(key) = SerKey::read(&params.item.data) else {
            return Ok(Some(Vec::new()));
        };
        let calls = crate::index_queries::outgoing_calls(&analysis.index, &key)
            .into_iter()
            .filter_map(|rel| {
                let to = Self::call_hierarchy_item(&analysis, rel.key, rel.def)?;
                let from_ranges = Self::call_ranges(&analysis, &rel.sites);
                Some(CallHierarchyOutgoingCall { to, from_ranges })
            })
            .collect();
        Ok(Some(calls))
    }

    /// v0.35 (ADR 0068): `textDocument/implementation` — on a capability
    /// symbol (its declaration, a `given Cap` use, or a `provides Cap` use),
    /// the providers that implement it. `None` for any other symbol (the
    /// reverse, provider → capability, is served by goto-definition).
    async fn goto_implementation(
        &self,
        params: GotoImplementationParams,
    ) -> JsonRpcResult<Option<GotoImplementationResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some((analysis, rel, offset)) = self.index_position(&uri, pos).await else {
            return Ok(None);
        };
        let Some((key, _)) = analysis.index.symbol_at(&rel, offset) else {
            return Ok(None);
        };
        if key.kind != bynk_check::index::SymbolKind::Capability {
            return Ok(None);
        }
        let locations: Vec<Location> = crate::index_queries::implementations(&analysis.index, key)
            .into_iter()
            .filter_map(|d| Self::site_to_location(&analysis, d))
            .collect();
        if locations.is_empty() {
            return Ok(None);
        }
        Ok(Some(GotoDefinitionResponse::Array(locations)))
    }

    /// Slice 6: `textDocument/typeDefinition` — from a value at the cursor to the
    /// definition of its (user-declared) type. Reads the value's type from the
    /// round's `expr_types`, unwraps it to a `Named` target, and returns that
    /// type's definition site(s). `None` for a built-in/function/actor type, or
    /// a cursor not on a typed expression in a clean round.
    async fn goto_type_definition(
        &self,
        params: GotoTypeDefinitionParams,
    ) -> JsonRpcResult<Option<GotoTypeDefinitionResponse>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some((analysis, rel, offset)) = self.index_position(&uri, pos).await else {
            return Ok(None);
        };
        let tys = &analysis.ty_intern;
        let Some(entries) = analysis.expr_types.get(&rel) else {
            return Ok(None);
        };
        let Some(ty) = bynk_check::expr_types::type_at_offset(entries, offset, tys) else {
            return Ok(None);
        };
        let Some(name) = crate::index_queries::named_type_target(ty, tys) else {
            return Ok(None);
        };
        let locations: Vec<Location> =
            crate::index_queries::type_definitions_named(&analysis.index, &name)
                .into_iter()
                .filter_map(|d| Self::site_to_location(&analysis, d))
                .collect();
        if locations.is_empty() {
            return Ok(None);
        }
        Ok(Some(GotoDefinitionResponse::Array(locations)))
    }

    /// Slice 6b (ADR 0095): `textDocument/documentLink` — `uses`/`consumes` unit
    /// names are clickable to the unit's source. Spans come from parsing the live
    /// buffer; the target is the unit's first source file from the round's
    /// unit→source map. A first-party `uses` (embedded, no on-disk file) or an
    /// unresolved unit yields no link.
    ///
    /// #848: plus intra-doc links inside the file's own `--- … ---` doc
    /// comments — `[Name]`/`[Owner.member]` resolved against the declaring
    /// unit's `doc_scope`. Resolves against the full `analysis.index` under
    /// the same `committed_analysis` gate as the unit-reference links above;
    /// consistent with `code_lens`/`capability_provider_lenses`, which
    /// already resolve full-index cross-references under this gate.
    async fn document_link(
        &self,
        params: DocumentLinkParams,
    ) -> JsonRpcResult<Option<Vec<DocumentLink>>> {
        let uri = params.text_document.uri;
        // #733: committed round. Link ranges convert against live `text` here and
        // the round only supplies the project-level `unit_sources` map (it changes
        // only on a `uses`/`consumes` edit), so a committed round is safe.
        let analysis = self.committed_analysis(&uri).await;
        let text = self
            .state
            .read()
            .await
            .docs
            .get(&uri)
            .map(|d| d.text.clone());
        let (Some(text), Some(analysis)) = (text, analysis) else {
            return Ok(None);
        };
        let mut links: Vec<DocumentLink> = crate::symbols::unit_reference_spans(&text)
            .into_iter()
            .filter_map(|(unit, span)| {
                let rel = analysis.unit_sources.get(&unit)?.first()?;
                let target = Url::from_file_path(analysis.project_root.join(rel)).ok()?;
                Some(DocumentLink {
                    range: crate::position::span_to_range(&text, span),
                    target: Some(target),
                    tooltip: Some(format!("Open unit `{unit}`")),
                    data: None,
                })
            })
            .collect();
        // #848: a suite file's own doc comments are out of scope this
        // increment (own_declaration_name returns None for a suite; its
        // uses-clause links above are unaffected).
        if let Some((owner_unit, _)) = crate::symbols::own_declaration_name(&text) {
            for (name, span) in crate::symbols::doc_link_spans(&text) {
                let Some(def) = crate::index_queries::resolve_doc_link(
                    &analysis.index,
                    &analysis.doc_scope,
                    &owner_unit,
                    &name,
                ) else {
                    continue;
                };
                let Some(target) = crate::symbols::doc_link_target(
                    &analysis.project_root,
                    def,
                    &analysis.snapshots,
                ) else {
                    continue;
                };
                links.push(DocumentLink {
                    range: crate::position::span_to_range(&text, span),
                    target: Some(target),
                    tooltip: Some(format!("Go to `{name}`")),
                    data: None,
                });
            }
        }
        Ok((!links.is_empty()).then_some(links))
    }

    async fn completion(
        &self,
        params: CompletionParams,
    ) -> JsonRpcResult<Option<CompletionResponse>> {
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let text = {
            let s = self.state.read().await;
            s.docs.get(&uri).map(|d| d.text.clone())
        };
        let Some(text) = text else { return Ok(None) };
        let offset = cursor_offset(&text, pos);
        // The line up to the cursor — the context the completion keys off.
        // Derived from the converted offset (always a char boundary), not by
        // slicing the line at `pos.character` bytes.
        let line_prefix = text[..offset].rsplit('\n').next().unwrap_or("").to_string();
        let files = self.project_content(&uri).await;
        // #2134: inside a suite's or case's `env { }` block, at a name position,
        // offer the secret names actors declare and the `Secrets.get` literals.
        if completion::in_env_name_position(&text, offset) {
            let items = completion::env_name_candidates(&text, files.as_deref())
                .into_iter()
                .map(to_completion_item)
                .collect();
            return Ok(Some(CompletionResponse::Array(items)));
        }
        // `complete()` enumerates the project's units — file stats and CPU-bound
        // recovery parsing (of the buffer, and any project file whose parse cache
        // missed). Run it on the blocking pool so a keystroke on a large project
        // never stalls the async runtime (#733).
        let candidates = {
            let line_prefix = line_prefix.clone();
            let text = text.clone();
            let root = self.root_for_uri(&uri).await;
            match tokio::task::spawn_blocking(move || {
                completion::complete_in(root.as_deref(), &line_prefix, &text, files.as_deref())
            })
            .await
            {
                Ok(c) => c,
                // A panic (or cancellation) inside `complete()` degrades to empty
                // completions rather than a failed request — but log the
                // `JoinError` so the underlying bug is not silently swallowed
                // (#776 review).
                Err(e) => {
                    tracing::error!("completion enumeration task failed: {e}");
                    Vec::new()
                }
            }
        };
        let mut items: Vec<CompletionItem> =
            candidates.into_iter().map(to_completion_item).collect();
        // ADR 0064/0093 D3: offer in-scope locals/params at keyword position
        // (alongside keywords) and at expression position (alongside the
        // constructors + type names `complete()` now yields there). Both are
        // places a value or name can begin; the two positions are disjoint.
        if completion::is_keyword_position(&line_prefix)
            || completion::is_expression_position(&line_prefix)
        {
            items.extend(self.locals_completions(&uri, pos).await);
        }
        // v0.124 (slice 3): inside a `requires`/`ensures` predicate, offer the
        // enclosing function's parameters (and `result` in an `ensures`),
        // merged with whatever the lexical cell yields there — the same
        // append-in-scope-names posture as locals above.
        items.extend(contract_param_completions(&text, offset, &line_prefix));
        // v0.131: inside a `cors { }` block, offer the policy field names; at a
        // service-body item start, offer the `cors` section keyword alongside the
        // handler-kind keywords the keyword-position cell already yields.
        items.extend(cors_completions(&text, offset, &line_prefix));
        // v0.141 (ADR 0164): inside a `security { }` block, offer the policy field
        // names; at a service-body item start, offer the `security` section keyword.
        items.extend(security_completions(&text, offset, &line_prefix));
        // v0.140 (ADR 0163): inside `@cache( … )`, offer the annotation argument
        // names; at a service-body item start, offer the `@cache` snippet alongside
        // the `cors` keyword and handler kinds.
        items.extend(cache_completions(&text, offset, &line_prefix));
        // v0.142 (ADR 0165): inside a `limits { }` block, offer the policy field
        // names; at a service-body item start, offer the `limits` section keyword.
        items.extend(limits_completions(&text, offset, &line_prefix));
        // v0.142 (ADR 0165): inside `@limit( … )`, offer the annotation argument
        // names; at a service-body item start, offer the `@limit` snippet.
        items.extend(limit_completions(&text, offset, &line_prefix));
        // Inside `@raw( … )`, offer the `contentType` argument; at a service-body
        // item start, offer the `@raw` snippet.
        items.extend(raw_completions(&text, offset, &line_prefix));
        // P3 (#2342): on a `capability X = …` line, the instantiable bases and
        // then a base's required options.
        items.extend(
            completion::instance_declaration_completions(&line_prefix)
                .into_iter()
                .map(|(label, detail, insert)| CompletionItem {
                    label,
                    kind: Some(CompletionItemKind::SNIPPET),
                    detail: Some(detail),
                    insert_text: Some(insert),
                    insert_text_format: Some(InsertTextFormat::SNIPPET),
                    ..Default::default()
                }),
        );
        // P3 (#2342): inside `capability X = RateLimit @period(`, offer the
        // platform's two windows.
        items.extend(
            completion::instance_option_values(&text, offset)
                .iter()
                .map(|(value, doc)| CompletionItem {
                    label: value.to_string(),
                    kind: Some(CompletionItemKind::VALUE),
                    detail: Some((*doc).to_string()),
                    ..Default::default()
                }),
        );
        // v0.128: at a `match` arm-pattern-start, prepend the scrutinee's
        // variants — the most relevant candidate there. Unlike an `is` position, a
        // fresh-line or after-comma arm already looks like a keyword/expression
        // position (so `items` is non-empty and the `is_empty` path below never
        // fires), hence the merge. The expensive scrutinee typing is gated behind
        // the cheap lexical `match_scrutinee_offset` check inside, so ordinary
        // keyword-position completion pays only a string scan.
        // v0.145 (ADR 0169): a nested constructor position (`Some(‸`) offers the
        // payload type's variants; it and the arm-start position are mutually
        // exclusive (one is inside a `(`, the other before any), so the two lists
        // never overlap. Nested is the more specific position, so it leads.
        let mut pattern_items = self.nested_pattern_completions(&uri, &text, offset).await;
        pattern_items.extend(self.match_arm_completions(&uri, &text, offset).await);
        if !pattern_items.is_empty() {
            let mut merged = pattern_items;
            merged.extend(items);
            stamp_resolve_data(&mut merged, &uri);
            return Ok(Some(CompletionResponse::Array(merged)));
        }
        if items.is_empty() {
            // Slice 3: `<expr> is <cursor>` — offer the scrutinee sum type's
            // variants, resolved from `expr_types` (the ADR 0063 ceiling).
            let is_items = self.is_pattern_completions(&uri, &text, offset).await;
            if !is_items.is_empty() {
                return Ok(Some(CompletionResponse::Array(is_items)));
            }
            // A lowercase `receiver.` is a value receiver — type it by
            // re-analysing the rewritten buffer and offer its members. (Value
            // members name no declared symbol, so they carry no resolve data.)
            let value_items = self.value_member_completions(&uri, &text, offset).await;
            return Ok((!value_items.is_empty()).then_some(CompletionResponse::Array(value_items)));
        }
        // Slice 5: stash the doc URI so `completion_resolve` can attach lazy docs.
        stamp_resolve_data(&mut items, &uri);
        Ok(Some(CompletionResponse::Array(items)))
    }

    /// Slice 5: fill in hover-quality `documentation` for the focused completion
    /// item, reusing the hover renderer (`symbols::describe_symbol`, local then
    /// cross-file — §3.4). The originating doc URI is read from the item's
    /// `data` (a resolve request carries only the item, not a position). A no-op
    /// for an item that names no declared symbol (a keyword, kernel method, or
    /// local) — its one-line `detail` already suffices.
    async fn completion_resolve(&self, mut item: CompletionItem) -> JsonRpcResult<CompletionItem> {
        if item.documentation.is_some() {
            return Ok(item);
        }
        let Some(uri) = item
            .data
            .as_ref()
            .and_then(|d| d.get("uri"))
            .and_then(serde_json::Value::as_str)
            .and_then(|s| Url::parse(s).ok())
        else {
            return Ok(item);
        };
        let local = {
            let s = self.state.read().await;
            s.docs.get(&uri).map(|d| d.text.clone())
        };
        let doc = match local
            .as_deref()
            .and_then(|t| crate::symbols::describe_symbol(t, &item.label))
        {
            Some(md) => Some(md),
            // #733: the cross-file fallback enumerates the project's units (file
            // stats + recovery parse of the cache-missed ones); the firstparty
            // fallback parses the embedded surface. Both read/parse off the
            // blocking pool — completion-item resolve fires as the user arrows
            // through the completion list.
            None => {
                let files = self.project_content(&uri).await;
                let uri = uri.clone();
                let label = item.label.clone();
                match tokio::task::spawn_blocking(move || {
                    files
                        .and_then(|files| {
                            crate::symbols::describe_symbol_cross_file(&files, &uri, &label)
                        })
                        .map(|(_uri, md)| md)
                        // Slice 9: stdlib/surface symbols (e.g. a `uses bynk.list`
                        // combinator) live in the embedded first-party sources,
                        // not the project's files.
                        .or_else(|| crate::symbols::describe_firstparty_symbol(&label))
                })
                .await
                {
                    Ok(md) => md,
                    Err(e) => {
                        tracing::error!("completion-resolve describe task failed: {e}");
                        None
                    }
                }
            }
        };
        if let Some(md) = doc {
            item.documentation = Some(Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }));
        }
        Ok(item)
    }

    async fn goto_definition(
        &self,
        params: GotoDefinitionParams,
    ) -> JsonRpcResult<Option<GotoDefinitionResponse>> {
        let uri = params
            .text_document_position_params
            .text_document
            .uri
            .clone();
        let pos = params.text_document_position_params.position;
        // v0.25 rider: binding-correct definition via the index (fixes the
        // name-collision mis-navigation of the string-matching path). The
        // legacy path remains as fallback for not-yet-indexed symbol kinds
        // (locals, methods, fields, ops).
        if let Some((analysis, rel, offset)) = self.index_position(&uri, pos).await {
            if let Some((_, def)) =
                crate::index_queries::definition_at(&analysis.index, &rel, offset)
                && let Some(location) = Self::site_to_location(&analysis, def)
            {
                return Ok(Some(GotoDefinitionResponse::Scalar(location)));
            }
            // v0.31: a local binding — scope-correct definition (before the
            // string-matching fallback, which can't tell scopes apart).
            if let Some(text) = analysis.snapshots.get(&rel)
                && let Some(locals) = analysis.locals.get(&rel)
                && let Some(def) = crate::locals_nav::local_definition_at(locals, text, offset)
                && let Some(location) = self
                    .local_locations(&analysis, &rel, &[def])
                    .into_iter()
                    .next()
            {
                return Ok(Some(GotoDefinitionResponse::Scalar(location)));
            }
        }
        // Slice 6a follow-up (ADR 0095): the cursor on a `uses`/`consumes` unit
        // name jumps to that unit's source. Units aren't index symbols, so the
        // unit→source map resolves them; runs before the name-matching path so a
        // unit segment can't be mistaken for a like-named type.
        if let Some(location) = self.unit_reference_definition(&uri, pos).await {
            return Ok(Some(GotoDefinitionResponse::Scalar(location)));
        }
        let Some((name, _span, text)) = self.identifier_at(&uri, pos).await else {
            return Ok(None);
        };
        if let Some(decl_span) = crate::symbols::find_declaration_span(&text, &name) {
            let range = crate::position::span_to_range(&text, decl_span);
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri,
                range,
            })));
        }
        // Cross-file fallback (v1.1; LSP spec §3.4).
        if let Some(files) = self.project_content(&uri).await
            && let Some(found) = crate::symbols::find_declaration_cross_file(&files, &uri, &name)
        {
            let range = crate::position::span_to_range(&found.source, found.span);
            return Ok(Some(GotoDefinitionResponse::Scalar(Location {
                uri: found.uri,
                range,
            })));
        }
        Ok(None)
    }

    async fn formatting(
        &self,
        params: DocumentFormattingParams,
    ) -> JsonRpcResult<Option<Vec<TextEdit>>> {
        let uri = params.text_document.uri;
        let text = {
            let s = self.state.read().await;
            s.docs.get(&uri).map(|d| d.text.clone())
        };
        let Some(text) = text else { return Ok(None) };
        // Slice D: the format options are the owning project's (or the defaults
        // in single-file mode).
        let opts = self.config_for(&uri).await.format_options();
        match bynk_fmt::format_source(&text, &opts) {
            Ok(formatted) => {
                // #1763: a CRLF buffer whose LF form is canonical needs no edit,
                // as on the command line; otherwise every format-on-save would
                // rewrite an already-canonical Windows file.
                if formatted == bynk_fmt::normalize_line_endings(&text) {
                    Ok(Some(Vec::new()))
                } else {
                    // Replace the entire document.
                    let end_pos = crate::position::end_position(&text);
                    Ok(Some(vec![TextEdit {
                        range: Range {
                            start: Position::new(0, 0),
                            end: end_pos,
                        },
                        new_text: formatted,
                    }]))
                }
            }
            Err(_) => {
                // Formatting failed (parse error). Return no edits; the
                // diagnostics flow will surface the parse error.
                Ok(Some(Vec::new()))
            }
        }
    }

    async fn range_formatting(
        &self,
        params: DocumentRangeFormattingParams,
    ) -> JsonRpcResult<Option<Vec<TextEdit>>> {
        // Best-effort: format the whole document. Per spec, range
        // formatting may return edits wider than the requested range.
        self.formatting(DocumentFormattingParams {
            text_document: params.text_document,
            options: params.options,
            work_done_progress_params: params.work_done_progress_params,
        })
        .await
    }

    async fn document_symbol(
        &self,
        params: DocumentSymbolParams,
    ) -> JsonRpcResult<Option<DocumentSymbolResponse>> {
        // v1.1 — outline view + Cmd-Shift-O. See `design/bynk-lsp-spec.md` §3.7.
        let uri = params.text_document.uri;
        let text = {
            let s = self.state.read().await;
            s.docs.get(&uri).map(|d| d.text.clone())
        };
        let Some(text) = text else { return Ok(None) };
        let syms = crate::document_symbols::outline(&text);
        if syms.is_empty() {
            return Ok(None);
        }
        Ok(Some(DocumentSymbolResponse::Nested(syms)))
    }

    /// v0.37 (ADR 0070): `textDocument/foldingRange` — structural folds + comment
    /// runs from the recovered AST (no analysis round).
    async fn folding_range(
        &self,
        params: FoldingRangeParams,
    ) -> JsonRpcResult<Option<Vec<FoldingRange>>> {
        let uri = params.text_document.uri;
        let text = {
            let s = self.state.read().await;
            s.docs.get(&uri).map(|d| d.text.clone())
        };
        let Some(text) = text else { return Ok(None) };
        Ok(Some(crate::structure::folding_ranges(&text)))
    }

    /// v0.37 (ADR 0070): `textDocument/selectionRange` — the enclosing-node
    /// chain (innermost first) for each requested position.
    async fn selection_range(
        &self,
        params: SelectionRangeParams,
    ) -> JsonRpcResult<Option<Vec<SelectionRange>>> {
        let uri = params.text_document.uri;
        let text = {
            let s = self.state.read().await;
            s.docs.get(&uri).map(|d| d.text.clone())
        };
        let Some(text) = text else { return Ok(None) };
        Ok(Some(crate::structure::selection_ranges(
            &text,
            &params.positions,
        )))
    }

    async fn references(&self, params: ReferenceParams) -> JsonRpcResult<Option<Vec<Location>>> {
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let Some((analysis, rel, offset)) = self.index_position(&uri, pos).await else {
            return Ok(None);
        };
        let include_decl = params.context.include_declaration;
        if let Some(sites) =
            crate::index_queries::sites_for(&analysis.index, &rel, offset, include_decl)
        {
            let locations: Vec<Location> = sites
                .into_iter()
                .filter_map(|site| Self::site_to_location(&analysis, site))
                .collect();
            return Ok(Some(locations));
        }
        // v0.31: a local binding — its def + uses, resolved from the snapshot.
        if let Some(spans) = self.local_sites(&analysis, &rel, offset) {
            let spans = if include_decl {
                &spans[..]
            } else {
                &spans[1..]
            }; // def first
            let locations = self.local_locations(&analysis, &rel, spans);
            return Ok(Some(locations));
        }
        Ok(None)
    }

    /// v0.26 (ADR 0054): quick-fixes from structured suggestions. v0.213
    /// (ADR 0239) adds the extract-variable refactor
    /// (`CodeActionKind::REFACTOR_EXTRACT`), computed from the same snapshot.
    /// Track #800 adds the sibling extract-function refactor, additionally
    /// fed the round's `requirements`/`locals`/`expr_types` (the
    /// capability-free-only gate and the parameter/return type synthesis).
    /// Served from the **cached** analysis round only (never a fresh run —
    /// slow, and it could disagree with the squiggles the client is
    /// showing): a request before the first round, or for a file outside
    /// the project, returns the empty list. #804: the combined list is then
    /// filtered against `params.context.only`, if the client set it.
    async fn code_action(
        &self,
        params: CodeActionParams,
    ) -> JsonRpcResult<Option<CodeActionResponse>> {
        let uri = params.text_document.uri;
        // #733: committed round. The request range and the diagnostics the fixes
        // ride on both convert against the round's snapshot, and the emitted edits
        // carry the round's version, so a committed round is self-consistent.
        let analysis = self.committed_analysis(&uri).await;
        let Some(analysis) = analysis else {
            return Ok(Some(Vec::new()));
        };
        let Some(rel) = Self::uri_to_rel(&analysis, &uri) else {
            return Ok(Some(Vec::new()));
        };
        let (Some(text), Some(diags)) =
            (analysis.snapshots.get(&rel), analysis.diagnostics.get(&rel))
        else {
            return Ok(Some(Vec::new()));
        };
        // The request range converts against the analysed snapshot (the
        // v0.24 rule), like the spans it is intersected with.
        let (Some(start), Some(end)) = (
            crate::position::position_to_offset(text, params.range.start),
            crate::position::position_to_offset(text, params.range.end),
        ) else {
            return Ok(Some(Vec::new()));
        };
        let version = analysis.versions.get(&rel).copied();
        let span = bynk_syntax::span::Span::new(start, end);
        let mut actions = crate::code_actions::quick_fixes(text, diags, span, &uri, version);
        // #852: capability-aware header fixes (`add consumes`, auto-`uses`/
        // `consumes`), computed from the committed index + a fresh reparse.
        actions.extend(crate::capability_fixes::header_quick_fixes(
            text,
            diags,
            span,
            &uri,
            version,
            &analysis.index,
        ));
        // #2100 slice 3: fill-match-arms, listed without its edit; the edit
        // and its re-check run in `code_action_resolve`.
        for match_span in crate::fill_arms::fillable(diags, span) {
            actions.push(crate::fill_arms::listed(diags, match_span, &uri, version));
        }
        // #2100 slice 1: inline the local binding under the cursor, listed
        // without its edit (resolved and re-checked lazily, like fill-arms).
        if let Some(plan) = analysis
            .locals
            .get(&rel)
            .and_then(|locals| crate::inline::offerable(text, span.start, locals))
        {
            actions.push(crate::inline::listed(&plan, &uri, version));
        }
        // #2100 slice 4: a provider stub for a capability with none, listed
        // without its edit (resolved and re-checked lazily).
        for request in crate::provider_stub::requested(text, diags, span) {
            if let Some(plan) =
                crate::provider_stub::plan(text, &request.capability, &analysis.index)
            {
                actions.push(crate::provider_stub::listed(
                    &plan,
                    request.quick_fix,
                    &uri,
                    version,
                ));
            }
        }
        actions.extend(crate::extract::extract_variable(text, span, &uri, version));
        let empty_reqs = Vec::new();
        let empty_locals = Vec::new();
        let empty_types = Vec::new();
        // #2100 slice 2: extract-function is listed without its edit and
        // re-checked in `code_action_resolve`; a capability-using (or `?`-
        // containing) selection gets a disabled action saying why, for a
        // client that shows those.
        let planned = crate::extract::plan_function(
            text,
            span,
            analysis.requirements.get(&rel).unwrap_or(&empty_reqs),
            analysis.locals.get(&rel).unwrap_or(&empty_locals),
            analysis.expr_types.get(&rel).unwrap_or(&empty_types),
            &analysis.ty_intern,
        );
        let disabled_support = self.state.read().await.supports_code_action_disabled;
        actions.extend(crate::extract::function_listed(
            &planned,
            span,
            &uri,
            version,
            disabled_support,
        ));
        // #2100: source actions are computed only when the client asks for
        // their kind (the Source Action menu, the organise-imports command,
        // `editor.codeActionsOnSave`), never for the unfiltered lightbulb
        // request every cursor move sends: each one re-analyses the project
        // before it is offered (DECISION B).
        let only = params.context.only.as_deref();
        let wants = |kind: &CodeActionKind| {
            only.is_some_and(|only| {
                only.iter()
                    .any(|requested| crate::code_actions::kind_matches(kind, requested))
            })
        };
        if wants(&CodeActionKind::SOURCE_ORGANIZE_IMPORTS)
            && let Some(action) = self.organise_action(&analysis, &rel, &uri, version).await
        {
            actions.push(action);
        }
        if wants(&crate::fix_all::kind()) {
            let edits = crate::fix_all::edits(text, diags);
            if !edits.is_empty()
                && self
                    .rechecks_clean(&analysis, &rel, crate::fix_all::apply(text, &edits))
                    .await
            {
                actions.push(crate::fix_all::action(text, &edits, &uri, version));
            }
        }
        // #2098: "Explain `code`" for curated codes in range — last, so it never
        // preempts a real fix, and only for a client that runs the command.
        if self.state.read().await.supports_explain_command {
            actions.extend(crate::explain_request::explain_actions(diags, span));
        }
        // #804: honour the client's requested action kinds, if any.
        let actions = crate::code_actions::filter_by_only(actions, only);
        Ok(Some(actions))
    }

    /// #2100 slice 3: resolve a listed fill-match-arms action — compute its
    /// edit from the committed round and offer it only if the edited project
    /// re-checks clean (DECISION B). The round must still be the one the
    /// action was listed from (same document version); otherwise, or when
    /// the fill doesn't re-check, the resolve is refused with a message the
    /// client shows. Any other action resolves to itself.
    async fn code_action_resolve(&self, action: CodeAction) -> JsonRpcResult<CodeAction> {
        if let Some(target) = crate::inline::Target::of(&action) {
            return self.resolve_inline(action, target).await;
        }
        if let Some(target) = crate::provider_stub::Target::of(&action) {
            return self.resolve_provider_stub(action, target).await;
        }
        if let Some(target) = crate::extract::FunctionTarget::of(&action) {
            return self.resolve_extract_function(action, target).await;
        }
        let Some(target) = crate::fill_arms::Target::of(&action) else {
            return Ok(action);
        };
        let refused = |msg: &str| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::InvalidParams,
            message: msg.to_string().into(),
            data: None,
        };
        let stale = "the document changed since the match arms were offered";
        let analysis = self
            .committed_analysis(&target.uri)
            .await
            .ok_or_else(|| refused(stale))?;
        let rel = Self::uri_to_rel(&analysis, &target.uri).ok_or_else(|| refused(stale))?;
        let (Some(text), Some(diags)) =
            (analysis.snapshots.get(&rel), analysis.diagnostics.get(&rel))
        else {
            return Err(refused(stale));
        };
        let version = analysis.versions.get(&rel).copied();
        if version != target.version {
            return Err(refused(stale));
        }
        let edits = crate::fill_arms::edits(text, diags, target.match_span)
            .ok_or_else(|| refused("the missing match arms can't be filled here"))?;
        let filled = crate::fix_all::apply(text, &edits);
        if !self.rechecks_clean(&analysis, &rel, filled).await {
            return Err(refused(
                "filling the missing match arms would introduce a new problem",
            ));
        }
        Ok(crate::fill_arms::resolve(
            action,
            text,
            &edits,
            &target.uri,
            version,
        ))
    }

    /// v0.27 (ADR 0056): inferred-type inlay hints for the visible range,
    /// served from the cached round only — no cached round (pre-first-
    /// analysis, non-project file) returns the empty list. Positions
    /// convert against the analysed snapshot (the v0.24 rule).
    async fn inlay_hint(&self, params: InlayHintParams) -> JsonRpcResult<Option<Vec<InlayHint>>> {
        let uri = params.text_document.uri;
        // #733: committed round, no forced re-analysis (see `committed_analysis`).
        let analysis = self.committed_analysis(&uri).await;
        let Some(analysis) = analysis else {
            return Ok(Some(Vec::new()));
        };
        let Some(rel) = Self::uri_to_rel(&analysis, &uri) else {
            return Ok(Some(Vec::new()));
        };
        let Some(text) = analysis.snapshots.get(&rel) else {
            return Ok(Some(Vec::new()));
        };
        // The visible range converts against the analysed snapshot, like
        // the hint spans it is intersected with.
        let (Some(start), Some(end)) = (
            crate::position::position_to_offset(text, params.range.start),
            crate::position::position_to_offset(text, params.range.end),
        ) else {
            return Ok(Some(Vec::new()));
        };
        let visible = bynk_syntax::span::Span::new(start, end);
        // v0.27: inferred-type hints. v0.99: plus the materializable ghost
        // `given` hints for uncovered capability requirements. A file may carry
        // one set without the other, so each defaults to empty independently.
        let mut hints = analysis
            .hints
            .get(&rel)
            .map(|h| crate::inlay_hints::inlay_hints(text, h, visible))
            .unwrap_or_default();
        if let Some(reqs) = analysis.requirements.get(&rel) {
            hints.extend(crate::inlay_hints::given_hints(text, reqs, visible));
        }
        Ok(Some(hints))
    }

    /// v0.28 (ADR 0057): semantic tokens for the whole document, served
    /// from the cached round only (no cached round / non-project file →
    /// empty), positions against the analysed snapshot (the v0.24 rule).
    async fn semantic_tokens_full(
        &self,
        params: SemanticTokensParams,
    ) -> JsonRpcResult<Option<SemanticTokensResult>> {
        let data = self
            .semantic_tokens_for(&params.text_document.uri, None)
            .await;
        Ok(Some(SemanticTokensResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    /// v0.28 (ADR 0057): the `…/range` variant — the same pure read,
    /// filtered to tokens overlapping the requested range.
    async fn semantic_tokens_range(
        &self,
        params: SemanticTokensRangeParams,
    ) -> JsonRpcResult<Option<SemanticTokensRangeResult>> {
        let data = self
            .semantic_tokens_for(&params.text_document.uri, Some(params.range))
            .await;
        Ok(Some(SemanticTokensRangeResult::Tokens(SemanticTokens {
            result_id: None,
            data,
        })))
    }

    /// v0.26 rider (ADR 0055): workspace-wide symbol search — the index's
    /// definitions, filtered by the query. Slice D (Q4): one server, many
    /// projects — aggregate across **every** project. Candidates are the
    /// **already-warmed** projects (slice E warms every project under the folders
    /// at `initialized`, and the watcher warms one created later), plus each
    /// folder's own `resolve_root` — a cheap bounded walk-*up*, the pre-slice-E
    /// seeding. No full tree-walk on this request path: a `workspace/symbol`
    /// query can fire per keystroke, and the warmed set already holds the nested
    /// monorepo projects a walk would rediscover.
    async fn symbol(
        &self,
        params: WorkspaceSymbolParams,
    ) -> JsonRpcResult<Option<Vec<SymbolInformation>>> {
        let candidates: Vec<(PathBuf, ProjectConfig)> = {
            // Snapshot warmed projects + folders under the lock; resolve the
            // folders' own roots off it (a bounded walk-up, but still FS I/O).
            let (mut set, folders) = {
                let state = self.state.read().await;
                let known: std::collections::HashMap<PathBuf, ProjectConfig> = state
                    .projects
                    .iter()
                    .map(|(r, p)| (r.clone(), p.config.clone()))
                    .collect();
                (known, state.folders.clone())
            };
            for folder in &folders {
                if let Some((root, config)) = Self::resolve_root(folder) {
                    let root = root.canonicalize().unwrap_or(root);
                    set.entry(root).or_insert(config);
                }
            }
            set.into_iter().collect()
        };
        let mut symbols: Vec<SymbolInformation> = Vec::new();
        for (root, config) in candidates {
            let Some(analysis) = self.ensure_project_analysed(root, config).await else {
                continue;
            };
            for (key, def) in
                crate::index_queries::workspace_symbols(&analysis.index, &params.query)
            {
                let Some(location) = Self::site_to_location(&analysis, def) else {
                    continue;
                };
                #[allow(deprecated)]
                symbols.push(SymbolInformation {
                    name: key.name.clone(),
                    kind: lsp_symbol_kind(key.kind),
                    tags: None,
                    deprecated: None,
                    location,
                    container_name: Some(key.unit.clone()),
                });
            }
        }
        // Aggregating across projects (a `HashMap`-derived candidate list) groups
        // matches by project in arbitrary order; the spec (§3.11) promises a
        // stable `(name, unit)` ordering, so sort the merged result. `unit` is
        // the container name.
        symbols.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| a.container_name.cmp(&b.container_name))
        });
        Ok(Some(symbols))
    }

    /// v0.26 rider (ADR 0055): the symbol-at-cursor's occurrences in the
    /// active file. `kind` is omitted — the index does not distinguish read
    /// from write references.
    async fn document_highlight(
        &self,
        params: DocumentHighlightParams,
    ) -> JsonRpcResult<Option<Vec<DocumentHighlight>>> {
        let uri = params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;
        let Some((analysis, rel, offset)) = self.index_position(&uri, pos).await else {
            return Ok(None);
        };
        let Some(text) = analysis.snapshots.get(&rel) else {
            return Ok(None);
        };
        if let Some(sites) =
            crate::index_queries::document_highlights(&analysis.index, &rel, offset)
        {
            let highlights: Vec<DocumentHighlight> = sites
                .into_iter()
                .map(|s| DocumentHighlight {
                    range: crate::position::span_to_range(text, s.span),
                    kind: None,
                })
                .collect();
            return Ok(Some(highlights));
        }
        // v0.31: a local binding's occurrences (def + uses) in the file.
        if let Some(spans) = self.local_sites(&analysis, &rel, offset) {
            let highlights = spans
                .iter()
                .map(|s| DocumentHighlight {
                    range: crate::position::span_to_range(text, *s),
                    kind: None,
                })
                .collect();
            return Ok(Some(highlights));
        }
        Ok(None)
    }

    async fn prepare_rename(
        &self,
        params: TextDocumentPositionParams,
    ) -> JsonRpcResult<Option<PrepareRenameResponse>> {
        let uri = params.text_document.uri;
        let pos = params.position;
        // Refuse (None) for anything the index does not cover — locals,
        // methods, record fields, capability ops, unit names — rather than
        // falling through to a partial or name-matched rename.
        let Some((analysis, rel, offset)) = self.index_position(&uri, pos).await else {
            return Ok(None);
        };
        let Some((key, site)) = crate::index_queries::prepare_rename(&analysis.index, &rel, offset)
        else {
            return Ok(None);
        };
        let Some(text) = analysis.snapshots.get(&rel) else {
            return Ok(None);
        };
        Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: crate::position::span_to_range(text, site.span),
            placeholder: key.name.clone(),
        }))
    }

    async fn rename(&self, params: RenameParams) -> JsonRpcResult<Option<WorkspaceEdit>> {
        let uri = params.text_document_position.text_document.uri;
        let pos = params.text_document_position.position;
        let new_name = params.new_name;
        let refused = |msg: String| tower_lsp::jsonrpc::Error {
            code: tower_lsp::jsonrpc::ErrorCode::InvalidParams,
            message: msg.into(),
            data: None,
        };
        // Slice B: rename emits versioned edits across *every* file that
        // references the symbol, so it needs the round current for **all** open
        // buffers, not just the cursor's (`analysis_for` would leave a dirty
        // non-cursor file stale and its edit would be stamped with an old
        // version, which the client rejects). `analysis_covering_open_buffers`
        // restores the whole-project freshness the pre-v0.179 `fresh_analysis`
        // gave. The cursor's file is one of those buffers, so it is current too;
        // resolve `rel`/`offset` against it here (what `index_position` did).
        // Slice D (Q4): route by the cursor's project — a rename spans one
        // project, so the round need only cover *that* project's buffers.
        let Some(root) = self.root_for_uri(&uri).await else {
            return Err(refused("rename requires a project (bynk.toml)".into()));
        };
        let Some(analysis) = self.analysis_covering_open_buffers(&root).await else {
            return Err(refused("rename requires a project (bynk.toml)".into()));
        };
        let Some(rel) = Self::uri_to_rel(&analysis, &uri) else {
            return Ok(None);
        };
        let Some(text) = analysis.snapshots.get(&rel) else {
            return Ok(None);
        };
        let Some(offset) = crate::position::position_to_offset(text, pos) else {
            return Ok(None);
        };
        let plan = crate::index_queries::plan_rename(&analysis.index, &rel, offset, &new_name)
            .map_err(refused)?;

        // Validator 1 + 2 input: re-analyse with the edits applied. Every
        // snapshot is pinned via the overlay so the re-analysis differs from
        // the plan's baseline only by the edits themselves.
        //
        // Slice A: this must re-analyse over the **same roots** the baseline
        // round used (`AnalysisRoots::Project`, manifest-aware), not the
        // single-tree `diagnose_project`. `diagnose_project(project_root)`
        // resolves to `Roots::Single`, which walks the whole tree with **no
        // `exclude`** and no `out`/`node_modules` skip — so `post` would cover a
        // superset of the baseline's files, and validators 1 and 2 (which
        // compare `post` against baselines from the manifest-aware round) would
        // read a diagnostic or index site in an excluded tree as *new* and
        // refuse a valid rename.
        let mut overlay = std::collections::HashMap::new();
        for (rel_path, text) in &analysis.snapshots {
            let edited = match plan.edits.get(rel_path) {
                Some(spans) => crate::index_queries::apply_edits(text, spans, &plan.new_name),
                None => text.clone(),
            };
            let abs = analysis.project_root.join(rel_path);
            let abs = abs.canonicalize().unwrap_or(abs);
            overlay.insert(abs, edited);
        }
        let roots = bynk_ide::AnalysisRoots::Project(analysis.project_root.clone());
        // #2081: `Off`. Schema-lock diagnostics are project-level: they land
        // in `unattributed`, which neither validator compares (both read
        // `post.files`), so reading the lock here would cost a `stat` and an
        // event sweep for nothing. A rename that changes an event
        // non-additively is not refused; the next diagnostics round reports
        // it on `bynk.toml`.
        let Ok(post) = tokio::task::spawn_blocking(move || {
            bynk_ide::diagnose_project_with(&roots, &overlay, &bynk_ide::SchemaLock::Off)
        })
        .await
        else {
            return Err(refused("rename validation failed to run".into()));
        };

        // Validator 1 — collisions: refuse on any new diagnostic.
        let post_diags: Vec<(PathBuf, String)> = post
            .files
            .iter()
            .flat_map(|f| {
                f.diagnostics
                    .iter()
                    .map(|d| (f.source_path.clone(), d.error.category.to_string()))
            })
            .collect();
        crate::index_queries::no_new_diagnostics(&analysis.diag_categories(), &post_diags)
            .map_err(refused)?;

        // Validator 2 — capture/escape: the re-built index must be the old
        // index modulo the rename; a silent re-binding has no diagnostic.
        if !crate::index_queries::index_unchanged_modulo_rename(&analysis.index, &post.index, &plan)
        {
            return Err(refused(format!(
                "renaming `{}` to `{new_name}` would silently re-bind another name — refused",
                plan.key.name
            )));
        }

        // Versioned edits: the client rejects the rename if a buffer drifted
        // past the analysed version rather than mis-applying it.
        let mut document_edits: Vec<TextDocumentEdit> = Vec::new();
        for (rel_path, spans) in &plan.edits {
            let Some(text) = analysis.snapshots.get(rel_path) else {
                continue;
            };
            let abs = analysis.project_root.join(rel_path);
            let Ok(file_uri) = Url::from_file_path(&abs) else {
                continue;
            };
            let edits: Vec<OneOf<TextEdit, AnnotatedTextEdit>> = spans
                .iter()
                .map(|span| {
                    OneOf::Left(TextEdit {
                        range: crate::position::span_to_range(text, *span),
                        new_text: plan.new_name.clone(),
                    })
                })
                .collect();
            document_edits.push(TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier {
                    uri: file_uri,
                    version: analysis.versions.get(rel_path).copied(),
                },
                edits,
            });
        }
        Ok(Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(document_edits)),
            change_annotations: None,
        }))
    }

    /// #302: `workspace/willRenameFiles` — when a `.bynk` file is renamed or
    /// moved, keep `uses`/`consumes` references pointing at its unit in sync.
    /// Uses `analysis_covering_open_buffers`, the same gate `rename`
    /// uses: this handler emits multi-file **versioned** edits too, so a
    /// stale open buffer must be refreshed first or the client rejects the
    /// whole edit — unlike `documentLink`'s read-only decoration, which
    /// tolerates a round lagging by one debounce cycle.
    ///
    /// Never refuses: a filesystem rename isn't something this soft,
    /// edit-only hook can block (the response is just an optional edit), so
    /// anything this can't confidently resolve — an unparseable file, a
    /// `suite` (addressed by no one), a rename that preserves the unit's
    /// arrangement, a cross-project move, a name collision with an existing
    /// unit — is simply skipped rather than erroring the whole batch. The
    /// collision check is a lightweight `unit_sources` lookup, not `rename`'s
    /// full re-analysis: good enough to avoid handing back an edit that is
    /// *known in advance* to break the build, without paying for a second
    /// analysis round on every file move.
    ///
    /// Edits for the moved file's own declaration target `old_uri`, not
    /// `new_uri`: the client applies the returned edit against files at
    /// their current (pre-move) locations, then performs the actual rename,
    /// so the file lands at its new path already carrying the new name.
    ///
    /// #2105: a `FileRename` may name a **folder** (the capability registers a
    /// folder filter too). A folder rename is the same operation as one file
    /// rename per `.bynk` file under it, so it is expanded server-side from
    /// the round's snapshots (`Self::expand_rename`) and each file goes
    /// through the single-file computation. A folder holding no project file
    /// expands to nothing. Across the whole request, a file is handled once
    /// (a request naming a file *and* its enclosing folder is deduplicated by
    /// resolved path), and each renamed unit's references are rewritten once
    /// (the files of a multi-file unit all map to the same new name). A move
    /// whose destination belongs to a different project root is skipped with
    /// a log line (DECISION B): the units' identity changes beyond a rename.
    async fn will_rename_files(
        &self,
        params: RenameFilesParams,
    ) -> JsonRpcResult<Option<WorkspaceEdit>> {
        let mut combined: std::collections::HashMap<Url, (Option<i32>, Vec<TextEdit>)> =
            std::collections::HashMap::new();
        // #2105: (project root, project-relative path) of every file handled,
        // and (project root, old unit name) of every unit whose references
        // were rewritten, across the whole request.
        let mut handled_files: std::collections::HashSet<(PathBuf, PathBuf)> =
            std::collections::HashSet::new();
        let mut renamed_units: std::collections::HashSet<(PathBuf, String)> =
            std::collections::HashSet::new();
        for fr in &params.files {
            let (Ok(old_uri), Ok(new_uri)) = (Url::parse(&fr.old_uri), Url::parse(&fr.new_uri))
            else {
                continue;
            };
            let Some(root) = self.root_for_uri(&old_uri).await else {
                continue;
            };
            // DECISION B: a move into another project changes the units'
            // identity beyond a rename; leave its references to the user.
            if Self::new_path_root(&new_uri).as_ref() != Some(&root) {
                tracing::info!(
                    "willRenameFiles: {} -> {} leaves project {}; no reference edits",
                    fr.old_uri,
                    fr.new_uri,
                    root.display()
                );
                continue;
            }
            let Some(analysis) = self.analysis_covering_open_buffers(&root).await else {
                continue;
            };
            for (old_rel, new_rel) in Self::expand_rename(&analysis, &old_uri, &new_uri) {
                if !handled_files.insert((root.clone(), old_rel.clone())) {
                    continue;
                }
                let Some(text) = analysis.snapshots.get(&old_rel) else {
                    continue;
                };
                let Some((old_name, name_span)) = crate::symbols::own_declaration_name(text) else {
                    continue;
                };
                let Some(new_name) = bynk_ide::renamed_unit_name(&old_rel, &old_name, &new_rel)
                else {
                    // #2105: logged, since inside a folder rename the other
                    // units are still edited and this one is moved unchanged.
                    tracing::info!(
                        "willRenameFiles: {} -> {}: no unit name follows from the new path \
                         for `{old_name}`; left unchanged",
                        old_rel.display(),
                        new_rel.display()
                    );
                    continue;
                };
                if new_name == old_name {
                    continue;
                }
                // Refuse to hand back an edit that would create a duplicate unit
                // name — some other file already declares `new_name`.
                if analysis.unit_sources.contains_key(&new_name) {
                    tracing::info!(
                        "willRenameFiles: {} -> {}: `{new_name}` is already declared; \
                         `{old_name}` left unchanged",
                        old_rel.display(),
                        new_rel.display()
                    );
                    continue;
                }
                // Every file's Url is reconstructed the same way (never the
                // client's raw `old_uri`/`new_uri` strings) so the moved file's
                // own edit and a referencer's edit merge into the same
                // `TextDocumentEdit` when they're the same file — a raw client
                // string and a `from_file_path` reconstruction aren't guaranteed
                // byte-identical (percent-encoding, trailing slashes).
                let Ok(old_file_uri) = Url::from_file_path(analysis.project_root.join(&old_rel))
                else {
                    continue;
                };
                // The moved file's own declaration header — edited at its old
                // (still current) location.
                combined
                    .entry(old_file_uri)
                    .or_insert_with(|| (analysis.versions.get(&old_rel).copied(), Vec::new()))
                    .1
                    .push(TextEdit {
                        range: crate::position::span_to_range(text, name_span),
                        new_text: new_name.clone(),
                    });
                // #2105: the files of a multi-file unit share its name, so its
                // references are rewritten for the first of them only — a
                // second pass would emit the same, overlapping, edits again.
                if !renamed_units.insert((root.clone(), old_name.clone())) {
                    continue;
                }
                // Every other file's `uses`/`consumes` references to the old name.
                for (rel, snap_text) in &analysis.snapshots {
                    if *rel == old_rel {
                        continue;
                    }
                    let edits: Vec<TextEdit> = crate::symbols::unit_reference_spans(snap_text)
                        .into_iter()
                        .filter(|(unit, _)| *unit == old_name)
                        .map(|(_, span)| TextEdit {
                            range: crate::position::span_to_range(snap_text, span),
                            new_text: new_name.clone(),
                        })
                        .collect();
                    if edits.is_empty() {
                        continue;
                    }
                    let Ok(file_uri) = Url::from_file_path(analysis.project_root.join(rel)) else {
                        continue;
                    };
                    combined
                        .entry(file_uri)
                        .or_insert_with(|| (analysis.versions.get(rel).copied(), Vec::new()))
                        .1
                        .extend(edits);
                }
            }
        }
        if combined.is_empty() {
            return Ok(None);
        }
        let document_edits: Vec<TextDocumentEdit> = combined
            .into_iter()
            .map(|(uri, (version, edits))| TextDocumentEdit {
                text_document: OptionalVersionedTextDocumentIdentifier { uri, version },
                edits: edits.into_iter().map(OneOf::Left).collect(),
            })
            .collect();
        Ok(Some(WorkspaceEdit {
            changes: None,
            document_changes: Some(DocumentChanges::Edits(document_edits)),
            change_annotations: None,
        }))
    }

    async fn did_change_watched_files(&self, params: DidChangeWatchedFilesParams) {
        // #682: a `bynk.toml` create/delete/change is the one event that can
        // move an already-cached URI's route (see `State.root_cache`'s doc) —
        // invalidate the whole cache before this batch's lookups consult it,
        // so a manifest that just appeared/vanished is reflected within the
        // same round rather than one event late. The generation bump closes
        // `root_for_uri`'s TOCTOU window against a walk already in flight.
        if params.changes.iter().any(|ev| is_bynk_toml(&ev.uri)) {
            let mut state = self.state.write().await;
            state.root_cache.clear();
            state.root_cache_generation += 1;
        }
        // For every changed `.bynk` file we have open, refresh diagnostics.
        // Changes to files we do *not* have open (a git checkout, an external
        // edit) still invalidate the project index — schedule a project round
        // so cross-file state doesn't go stale (#513).
        let mut uris_to_refresh = Vec::new();
        // Slice D: route each change to its owning project root, so a change in
        // project A never re-analyses project B.
        let mut roots_to_reanalyse: std::collections::HashSet<PathBuf> =
            std::collections::HashSet::new();
        // A `bynk.toml` edit changes the formatting style, the diagnostics
        // mode/debounce, and the source root — none of which were re-read after
        // the initial load, so the settings only took effect on an LSP restart.
        // Detect the change here and reload that project's config before
        // re-analysing it.
        let mut config_changed_roots: std::collections::HashSet<PathBuf> =
            std::collections::HashSet::new();
        // #682: snapshotted off the lock — the loop below calls the
        // cache-consulting `root_for_uri`, which itself locks `state`, so it
        // must not run while a read lock from this function is still held.
        let open_docs: std::collections::HashSet<Url> =
            self.state.read().await.docs.keys().cloned().collect();
        for ev in &params.changes {
            if is_bynk_toml(&ev.uri) {
                // The manifest's own directory is the project root.
                if let Ok(p) = ev.uri.to_file_path()
                    && let Some(dir) = p.parent()
                {
                    let root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
                    config_changed_roots.insert(root.clone());
                    roots_to_reanalyse.insert(root);
                }
            } else if schema_lock::is_schema_lock(&ev.uri) {
                // #2081: the lock's own directory is the project root. Drop
                // its cached content and re-analyse, so the editor shows the
                // new lock's verdict without waiting for an edit.
                if let Ok(p) = ev.uri.to_file_path()
                    && let Some(dir) = p.parent()
                {
                    let root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
                    if let Some(ps) = self.state.write().await.projects.get_mut(&root) {
                        ps.schema_lock.invalidate();
                    }
                    roots_to_reanalyse.insert(root);
                }
            } else if open_docs.contains(&ev.uri) {
                uris_to_refresh.push(ev.uri.clone());
            } else if ev.uri.path().ends_with(".bynk")
                && let Some(root) = self.root_for_uri(&ev.uri).await
            {
                roots_to_reanalyse.insert(root);
            }
        }
        // A `bynk.toml` change reloads its project's config — and, if the
        // manifest was just *created*, warms the new project (create the entry).
        // Slice E: this is how a project added after startup is picked up now
        // that `workspace/symbol` no longer walks the tree per query.
        for root in &config_changed_roots {
            let config = project::load_config(root).unwrap_or_default();
            let mut state = self.state.write().await;
            state
                .projects
                .entry(root.clone())
                .and_modify(|ps| ps.config = config.clone())
                .or_insert_with(|| ProjectState {
                    config,
                    ..Default::default()
                });
        }
        for uri in uris_to_refresh {
            self.schedule_diagnostics(&uri).await;
        }
        // A reloaded config re-derives the diagnostics behaviour, so re-analyse
        // each affected project against it — the same debounced round a non-open
        // `.bynk` change schedules. A no-op for a root with no entry (a project
        // no file has opened): nothing is published there to go stale.
        for root in roots_to_reanalyse {
            self.schedule_project_diagnostics(root).await;
        }
    }

    async fn did_change_workspace_folders(&self, params: DidChangeWorkspaceFoldersParams) {
        // Slice D (Q4): folders are discovery seeds, not routing owners.
        // Added folders extend the seed set; removed folders shrink it, then any
        // project a removed folder orphaned — no remaining folder, no open
        // buffer — is pruned.
        let added_dirs: Vec<PathBuf> = {
            let mut state = self.state.write().await;
            let mut added = Vec::new();
            for a in &params.event.added {
                if let Ok(p) = a.uri.to_file_path() {
                    let dir = p.canonicalize().unwrap_or(p);
                    if !state.folders.contains(&dir) {
                        state.folders.push(dir.clone());
                        added.push(dir);
                    }
                }
            }
            for removed in &params.event.removed {
                if let Ok(p) = removed.uri.to_file_path() {
                    let dir = p.canonicalize().unwrap_or(p);
                    state.folders.retain(|f| f != &dir);
                }
            }
            // #682: `resolve_canonical` never consults `folders` — a folder
            // change cannot actually move any URI's route today — but clear
            // (and bump the generation, same as the `bynk.toml` case) as a
            // defensive, effectively-free no-op against that ever changing,
            // rather than relying on routing's independence from folders
            // staying true forever.
            state.root_cache.clear();
            state.root_cache_generation += 1;
            added
        };
        // Slice E: warm the added folders proactively — the analysis D deferred
        // to here, using the same discovery walk as startup.
        self.warm_projects(&added_dirs).await;
        // Clear the dropped projects' diagnostics so the client does not keep
        // showing stale squiggles for a folder that is gone.
        for uri in self.prune_orphaned_projects().await {
            self.client.publish_diagnostics(uri, Vec::new(), None).await;
        }
    }
}

/// The advertised capability set — `design/bynk-lsp-spec.md` §4.3. Split out
/// of `initialize` so the advertisement is unit-testable without transport.
fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        // Full-text sync, with save notifications explicitly opted in — the
        // `on_save` diagnostics mode is driven by `didSave` (#513).
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::FULL),
                save: Some(TextDocumentSyncSaveOptions::Supported(true)),
                ..Default::default()
            },
        )),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        // v0.17: completion for `consumes` units and `given` /
        // `consumes U { … }` capabilities. Trigger on the space after a
        // keyword, the `{` of a selected-capability list, and `,`. The `.`
        // auto-fires the name- and value-receiver member contexts (ADR 0093 D1).
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![
                " ".to_string(),
                "{".to_string(),
                ",".to_string(),
                ".".to_string(),
            ]),
            // Slice 5: resolve fills in hover-quality `documentation` lazily, on
            // the focused item only, so the initial list stays cheap.
            resolve_provider: Some(true),
            ..Default::default()
        }),
        // v0.32 (ADR 0065): signature help while typing a call's arguments.
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
            retrigger_characters: Some(vec![",".to_string()]),
            ..Default::default()
        }),
        // v0.33 (ADR 0066): reference-count lenses above top-level definitions.
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(false),
        }),
        // v0.34 (ADR 0067): call hierarchy over the binding index's call graph.
        call_hierarchy_provider: Some(CallHierarchyServerCapability::Simple(true)),
        // v0.35 (ADR 0068): implementation nav — capability → its providers.
        implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
        // Slice 6: go-to-type-definition (value → its type's declaration).
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        // Slice 6b: `uses`/`consumes` unit names link to their source.
        document_link_provider: Some(DocumentLinkOptions {
            resolve_provider: Some(false),
            work_done_progress_options: Default::default(),
        }),
        document_formatting_provider: Some(OneOf::Left(true)),
        document_range_formatting_provider: Some(OneOf::Left(true)),
        document_symbol_provider: Some(OneOf::Left(true)),
        // v0.37 (ADR 0070): structural folding + selection ranges (AST-driven).
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        selection_range_provider: Some(SelectionRangeProviderCapability::Simple(true)),
        // v0.25 (ADR 0053): references + rename over the binding
        // index; prepareRename refuses out-of-scope symbols.
        references_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        // v0.26 (ADR 0054): quick-fixes from the diagnostics' structured
        // suggestions. v0.213 (ADR 0239) adds the extract-variable refactor.
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::REFACTOR,
                CodeActionKind::REFACTOR_EXTRACT,
                CodeActionKind::REFACTOR_INLINE,
                CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                crate::fix_all::kind(),
            ]),
            // #2100 slice 3: fill-match-arms computes its edit lazily.
            resolve_provider: Some(true),
            ..Default::default()
        })),
        // v0.27 (ADR 0056): inferred-type inlay hints from the retained
        // analysis round's harvested hint set.
        inlay_hint_provider: Some(OneOf::Left(true)),
        // v0.28 (ADR 0057): semantic tokens over the frozen legend — a
        // pure read of the cached index (`symbols` + `foreign_refs`),
        // additive over the client's syntactic layer. `delta` deferred.
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: crate::index_queries::semantic_tokens_legend(),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                range: Some(true),
                ..Default::default()
            },
        )),
        // v0.26 riders (ADR 0055): both are `ProjectIndex` queries.
        workspace_symbol_provider: Some(OneOf::Left(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        workspace: Some(WorkspaceServerCapabilities {
            workspace_folders: Some(WorkspaceFoldersServerCapabilities {
                supported: Some(true),
                change_notifications: Some(OneOf::Left(true)),
            }),
            // #302: `willRenameFiles` over `.bynk` files keeps `uses`/`consumes`
            // references in sync on a file rename/move. #2105: a second filter
            // matches every folder (a folder's name can't say whether it holds
            // `.bynk` files); the handler expands it from the round's snapshots,
            // and one holding no project file produces no edit.
            file_operations: Some(WorkspaceFileOperationsServerCapabilities {
                will_rename: Some(FileOperationRegistrationOptions {
                    filters: vec![
                        FileOperationFilter {
                            scheme: Some("file".to_string()),
                            pattern: FileOperationPattern {
                                glob: "**/*.bynk".to_string(),
                                matches: Some(FileOperationPatternKind::File),
                                options: None,
                            },
                        },
                        FileOperationFilter {
                            scheme: Some("file".to_string()),
                            pattern: FileOperationPattern {
                                glob: "**".to_string(),
                                matches: Some(FileOperationPatternKind::Folder),
                                options: None,
                            },
                        },
                    ],
                }),
                ..Default::default()
            }),
        }),
        // #846/#847: no standard `ServerCapabilities` field exists for a custom
        // request — `experimental` is the only feature-detection surface a
        // client has for `bynk/sequenceModel`, `bynk/documentationModel`, and
        // `bynk/architectureModel`.
        experimental: Some(serde_json::json!({
            "sequenceModel": true,
            "agentStateModel": true,
            "documentationModel": true,
            "architectureModel": true,
            "wireContract": true,
            "explain": true,
            "emittedOutput": true,
        })),
        ..Default::default()
    }
}

/// Index symbol kind → LSP symbol kind, aligned with the document-symbol
/// outline's choices (capability=INTERFACE, service/agent=CLASS,
/// provider=OBJECT). The index does not distinguish type shapes, so every
/// type maps to STRUCT.
/// Map a `completion::Completion` to an LSP `CompletionItem`.
/// Stash the document URI in each item's `data` so `completion_resolve` can look
/// the symbol up — a resolve request carries only the item, not a position.
fn stamp_resolve_data(items: &mut [CompletionItem], uri: &Url) {
    let data = serde_json::json!({ "uri": uri.to_string() });
    for item in items.iter_mut() {
        item.data = Some(data.clone());
    }
}

/// v0.124 (slice 3): the enclosing function's parameters (and `result` for an
/// `ensures`) as completions, when `offset` sits in a `requires`/`ensures`
/// predicate. Empty when not in a contract clause or no enclosing `fn` is
/// found. A pure parse — the params are read straight off the recovered AST.
/// v0.131: the CORS completion cells. Inside a `cors { }` block at a field-name
/// position, offer the closed field set; at a service-body item start, offer the
/// `cors` section keyword. Both are lexical (offset-based), matching the
/// `contract_param_completions` posture.
fn cors_completions(text: &str, offset: usize, line: &str) -> Vec<CompletionItem> {
    if completion::in_cors_field_position(text, offset) {
        return completion::CORS_FIELDS
            .iter()
            .map(|(name, doc)| CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some((*doc).to_string()),
                insert_text: Some(format!("{name}: ")),
                ..Default::default()
            })
            .collect();
    }
    if completion::in_service_body_item_position(text, offset, line) {
        return vec![CompletionItem {
            label: "cors".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("a cross-origin (CORS) policy for this HTTP service".to_string()),
            insert_text: Some("cors {\n\torigins: [$0],\n}".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        }];
    }
    Vec::new()
}

/// v0.141 (ADR 0164): the security-headers completion cells. Inside a
/// `security { }` block at a field-name position, offer the closed field set
/// (`nosniff`/`hsts`); at a service-body item start, offer the `security` section
/// keyword. Both are lexical (offset-based), mirroring `cors_completions`.
fn security_completions(text: &str, offset: usize, line: &str) -> Vec<CompletionItem> {
    if completion::in_security_field_position(text, offset) {
        return completion::SECURITY_FIELDS
            .iter()
            .map(|(name, doc)| CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some((*doc).to_string()),
                insert_text: Some(format!("{name}: ")),
                ..Default::default()
            })
            .collect();
    }
    if completion::in_service_body_item_position(text, offset, line) {
        return vec![CompletionItem {
            label: "security".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("security response headers for this HTTP service".to_string()),
            insert_text: Some("security {\n\tnosniff: $0,\n}".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        }];
    }
    Vec::new()
}

/// v0.140 (ADR 0163): the `@cache` completion cells. Inside `@cache( … )` at an
/// argument-name position, offer the closed argument set (`maxAge`/`scope`/`immutable`); at a
/// service-body item start, offer the `@cache` annotation snippet. Both are lexical
/// (offset-based), mirroring `cors_completions`.
fn cache_completions(text: &str, offset: usize, line: &str) -> Vec<CompletionItem> {
    if completion::in_cache_arg_position(text, offset) {
        return completion::CACHE_ARGS
            .iter()
            .map(|(name, doc)| CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some((*doc).to_string()),
                insert_text: Some(format!("{name}: ")),
                ..Default::default()
            })
            .collect();
    }
    if completion::in_service_body_item_position(text, offset, line) {
        return vec![CompletionItem {
            label: "@cache".to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(
                "cache a GET read — a synthesised ETag/304 revalidation with a freshness window"
                    .to_string(),
            ),
            insert_text: Some("@cache(maxAge: ${1:5.minutes})".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        }];
    }
    Vec::new()
}

/// v0.142 (ADR 0165): the request-limits completion cells. Inside a `limits { }`
/// block at a field-name position, offer the closed field set (`maxBody`); at a
/// service-body item start, offer the `limits` section keyword. Both are lexical
/// (offset-based), mirroring `security_completions`.
fn limits_completions(text: &str, offset: usize, line: &str) -> Vec<CompletionItem> {
    if completion::in_limits_field_position(text, offset) {
        return completion::LIMITS_FIELDS
            .iter()
            .map(|(name, doc)| CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some((*doc).to_string()),
                insert_text: Some(format!("{name}: ")),
                ..Default::default()
            })
            .collect();
    }
    if completion::in_service_body_item_position(text, offset, line) {
        return vec![CompletionItem {
            label: "limits".to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            detail: Some("request limits for this HTTP service".to_string()),
            insert_text: Some("limits {\n\tmaxBody: $0,\n}".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        }];
    }
    Vec::new()
}

/// v0.142 (ADR 0165): the `@limit` completion cells. Inside `@limit( … )` at an
/// argument-name position, offer the closed argument set (`maxBody`); at a
/// service-body item start, offer the `@limit` annotation snippet. Both are lexical
/// (offset-based), mirroring `cache_completions`.
fn limit_completions(text: &str, offset: usize, line: &str) -> Vec<CompletionItem> {
    if completion::in_limit_arg_position(text, offset) {
        return completion::LIMIT_ARGS
            .iter()
            .map(|(name, doc)| CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some((*doc).to_string()),
                insert_text: Some(format!("{name}: ")),
                ..Default::default()
            })
            .collect();
    }
    if completion::in_service_body_item_position(text, offset, line) {
        return vec![CompletionItem {
            label: "@limit".to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(
                "cap the request body size — a `413` synthesised before the body is read"
                    .to_string(),
            ),
            insert_text: Some("@limit(maxBody: ${1:1048576})".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        }];
    }
    Vec::new()
}

/// The `@raw` completion cells, mirroring `limit_completions`: inside `@raw( … )` at
/// an argument-name position, the closed argument set (`contentType`); at a
/// service-body item start, the `@raw` annotation snippet.
fn raw_completions(text: &str, offset: usize, line: &str) -> Vec<CompletionItem> {
    if completion::in_raw_arg_position(text, offset) {
        return completion::RAW_ARGS
            .iter()
            .map(|(name, doc)| CompletionItem {
                label: name.to_string(),
                kind: Some(CompletionItemKind::FIELD),
                detail: Some((*doc).to_string()),
                insert_text: Some(format!("{name}: ")),
                ..Default::default()
            })
            .collect();
    }
    if completion::in_service_body_item_position(text, offset, line) {
        return vec![CompletionItem {
            label: "@raw".to_string(),
            kind: Some(CompletionItemKind::SNIPPET),
            detail: Some(
                "take `body: Bytes` as the raw request body — a `415` for any other content type"
                    .to_string(),
            ),
            insert_text: Some("@raw(contentType: \"${1:application/octet-stream}\")".to_string()),
            insert_text_format: Some(InsertTextFormat::SNIPPET),
            ..Default::default()
        }];
    }
    Vec::new()
}

fn contract_param_completions(text: &str, offset: usize, line: &str) -> Vec<CompletionItem> {
    use bynk_syntax::ast::{CommonsItem, SourceUnit};
    let Some(is_ensures) = completion::contract_clause_kind(line) else {
        return Vec::new();
    };
    let Ok(tokens) = bynk_syntax::lexer::tokenize(text) else {
        return Vec::new();
    };
    let (Some(unit), _) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, text) else {
        return Vec::new();
    };
    let items = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        _ => return Vec::new(),
    };
    for item in items {
        // The cursor sits in a fn's signature/contract region: between the fn's
        // start and the `{` that opens its body.
        if let CommonsItem::Fn(f) = item
            && f.span.start <= offset
            && offset <= f.body.span.start
        {
            // Built directly as VARIABLE items, matching `locals_completions`
            // (in-scope names carry no resolve data).
            let mut out: Vec<CompletionItem> = f
                .params
                .iter()
                .filter(|p| p.name.name != "_")
                .map(|p| CompletionItem {
                    label: p.name.name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(format!(
                        "parameter: {}",
                        crate::symbols::type_ref_str(&p.type_ref)
                    )),
                    ..Default::default()
                })
                .collect();
            if is_ensures {
                out.push(CompletionItem {
                    label: "result".to_string(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some("the function's return value".to_string()),
                    ..Default::default()
                });
            }
            return out;
        }
    }
    Vec::new()
}

/// v0.124 (slice 3): the byte offset of the scrutinee's last character in
/// `<scrutinee> is <partial>` ending at `cursor`, or `None` if the cursor is
/// not at an `is`-pattern position. `is` must be a standalone word (so `basis`
/// does not trigger it).
fn is_scrutinee_offset(text: &str, cursor: usize) -> Option<usize> {
    let before = text.get(..cursor)?;
    // Drop the partial variant being typed, then the whitespace before it.
    let before = before
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .trim_end();
    let before = before.strip_suffix("is")?;
    if !before.ends_with(char::is_whitespace) {
        return None;
    }
    let before = before.trim_end();
    (!before.is_empty()).then(|| before.len() - 1)
}

/// v0.128: the byte offset of the scrutinee's last character in a
/// `match <scrutinee> { … <partial>` whose cursor sits at an **arm-pattern-start**
/// position, or `None` otherwise — the deferred half of slice 3's `is`-pattern
/// completion. Conservative: it fires only at the *start* of an arm's pattern
/// (after the `{` or a top-level `,`, before any `=>`), never inside an arm body
/// or a nested constructor pattern, so it stays honest mid-edit.
fn match_scrutinee_offset(text: &str, cursor: usize) -> Option<usize> {
    let before = text.get(..cursor)?;
    // The innermost `{` still open at the cursor — the block the cursor is in.
    let brace = innermost_open_brace(before)?;
    // The current arm: from the last top-level `,` after the brace (or the brace
    // itself) to the cursor. A `=>` in it means the cursor is in the arm body.
    let arm_start = arm_start_offset(before, brace);
    let arm = before.get(arm_start..)?;
    if arm.contains("=>") {
        return None;
    }
    // Only at the pattern's *start*: nothing but the partial pattern being typed
    // sits between the arm boundary and the cursor.
    if !arm
        .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
        .trim()
        .is_empty()
    {
        return None;
    }
    match_head_scrutinee_offset(before, brace)
}

/// v0.145 (ADR 0169): the scrutinee offset and outer variant name at a
/// `match <scrutinee> { … OuterVariant(<partial>` position — the cursor inside a
/// variant's payload parens within an arm-pattern (before `=>`), the one place
/// `match_scrutinee_offset` bails. Conservative: the payload `(` must be still
/// open, the token before it an uppercase-led variant constructor, and only the
/// partial nested pattern may sit between the `(` and the cursor.
fn nested_pattern_offset(text: &str, cursor: usize) -> Option<(usize, String)> {
    let before = text.get(..cursor)?;
    let brace = innermost_open_brace(before)?;
    let arm_start = arm_start_offset(before, brace);
    let arm = before.get(arm_start..)?;
    if arm.contains("=>") {
        return None; // in the arm body, not its pattern
    }
    // The innermost `(` still open in the arm — the outer variant's payload.
    let paren = innermost_open_paren(arm)?;
    // The identifier immediately before that `(` is the outer variant; only an
    // uppercase-led constructor opens a nested pattern (a binding never does).
    let head = arm.get(..paren)?.trim_end();
    let variant: String = head
        .chars()
        .rev()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if !variant.chars().next().is_some_and(char::is_uppercase) {
        return None;
    }
    // Between the payload `(` and the cursor, only the partial nested pattern
    // being typed (an identifier, optionally a `Type.` qualifier) may sit.
    let after = arm.get(paren + 1..)?;
    if !after
        .trim_start_matches(|c: char| c.is_alphanumeric() || c == '_' || c == '.')
        .trim()
        .is_empty()
    {
        return None;
    }
    let scrut_off = match_head_scrutinee_offset(before, brace)?;
    Some((scrut_off, variant))
}

/// The byte offset of the scrutinee's last character for the `match <scrutinee>`
/// whose body brace is at `brace`, or `None` if `brace` does not head a
/// `match`: a standalone `match` keyword, then a scrutinee expression with no
/// nested block or arrow between it and the brace. Shared by
/// `match_scrutinee_offset` and `nested_pattern_offset`.
fn match_head_scrutinee_offset(before: &str, brace: usize) -> Option<usize> {
    let head = before.get(..brace)?.trim_end();
    let m = head.rfind("match")?;
    if head[..m]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
    {
        return None; // part of a longer identifier (`rematch`), not the keyword
    }
    let after = head.get(m + "match".len()..)?;
    if !after.starts_with(char::is_whitespace) {
        return None;
    }
    let scrut = after.trim();
    if scrut.is_empty() || scrut.contains(['{', '}']) || scrut.contains("=>") {
        return None;
    }
    // `head` was trimmed to end at the scrutinee's last char (the brace followed).
    Some(head.len() - 1)
}

/// #2010: what `let_else_match_completions` needs from the head of a
/// `let <pattern> (= | <-) <value> else match {` — read from text, like the
/// other pattern-position scanners here, so it works on a half-typed buffer.
#[derive(Debug, PartialEq, Eq)]
struct LetElseMatchHead {
    /// The offset of the value expression's last character.
    value_end: usize,
    /// `<-`: the value is an `Effect`, whose payload the arms match.
    effectful: bool,
    /// The variant the `let` pattern matches outright (`Ok(x)`, `Some(_)`,
    /// `P.Late`), which the arms never need; `None` when the pattern only
    /// covers part of a variant (`Err(Missing)`) or is not a variant.
    covered: Option<String>,
}

/// #2010: the [`LetElseMatchHead`] when the cursor is at an arm-pattern-start
/// inside a `let … else match { … }`.
fn let_else_match_head(text: &str, cursor: usize) -> Option<LetElseMatchHead> {
    let before = text.get(..cursor)?;
    let brace = innermost_open_brace(before)?;
    let arm = before.get(arm_start_offset(before, brace)..)?;
    if arm.contains("=>")
        || !arm
            .trim_end_matches(|c: char| c.is_alphanumeric() || c == '_')
            .trim()
            .is_empty()
    {
        return None;
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let strip_word = |s: &'_ str, word: &str| -> Option<usize> {
        let rest = s.strip_suffix(word)?;
        (!rest.ends_with(is_ident)).then_some(rest.trim_end().len())
    };
    let head = before.get(..brace)?.trim_end();
    let head = &head[..strip_word(head, "match")?];
    let head = &head[..strip_word(head, "else")?];
    let value_end = head.len().checked_sub(1)?;
    // The statement's own `let`: the last one at the start of a word.
    let let_at = head
        .match_indices("let")
        .map(|(i, _)| i)
        .filter(|&i| {
            !head[..i].ends_with(is_ident) && head[i + 3..].starts_with(char::is_whitespace)
        })
        .last()?;
    let rest = &head[let_at + 3..];
    // The first `=` (not `==`/`=>`) or `<-` outside brackets ends the pattern.
    let mut depth = 0i32;
    let mut split = None;
    let bytes = rest.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'<' if depth == 0 && bytes.get(i + 1) == Some(&b'-') => {
                split = Some((i, true));
                break;
            }
            b'=' if depth == 0
                && !matches!(bytes.get(i + 1), Some(b'=' | b'>'))
                && !matches!(
                    i.checked_sub(1).map(|j| bytes[j]),
                    Some(b'=' | b'!' | b'<' | b'>')
                ) =>
            {
                split = Some((i, false));
                break;
            }
            _ => {}
        }
    }
    let (at, effectful) = split?;
    let pattern = rest[..at].trim();
    let (name, payload) = match pattern.split_once('(') {
        Some((name, payload)) => (name, Some(payload.trim_end_matches(')'))),
        None => (pattern, None),
    };
    let name = name.rsplit('.').next()?.trim();
    let whole_variant = name.starts_with(char::is_uppercase)
        && payload.is_none_or(|p| {
            p.split(',').all(|part| {
                let binder = part.rsplit(':').next().unwrap_or(part).trim();
                binder == "_"
                    || binder.starts_with(char::is_lowercase) && binder.chars().all(is_ident)
            })
        });
    Some(LetElseMatchHead {
        value_end,
        effectful,
        covered: whole_variant.then(|| name.to_string()),
    })
}

/// The offset (relative to `arm`) of the innermost `(` left unclosed in `arm` — a
/// `(`-only balance scan, the payload paren the cursor sits in — or `None` if
/// every `(` is closed.
fn innermost_open_paren(arm: &str) -> Option<usize> {
    let mut stack: Vec<usize> = Vec::new();
    for (i, c) in arm.char_indices() {
        match c {
            '(' => stack.push(i),
            ')' => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack.pop()
}

/// The byte offset of the innermost `{` left unclosed in `before` (a `{`-only
/// balance scan — the block the cursor sits in), or `None` if every `{` is closed.
fn innermost_open_brace(before: &str) -> Option<usize> {
    let mut stack: Vec<usize> = Vec::new();
    for (i, c) in before.char_indices() {
        match c {
            '{' => stack.push(i),
            '}' => {
                stack.pop();
            }
            _ => {}
        }
    }
    stack.pop()
}

/// The offset just past the last top-level `,` inside the block opened at `brace`
/// (depth 0 relative to that brace), or just past the brace itself if the block
/// holds no top-level comma yet — the start of the arm the cursor is editing.
fn arm_start_offset(before: &str, brace: usize) -> usize {
    let mut depth = 0i32;
    let mut start = brace + 1; // just after the `{`
    for (rel, c) in before[brace + 1..].char_indices() {
        match c {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth -= 1,
            ',' if depth == 0 => start = brace + 1 + rel + c.len_utf8(),
            _ => {}
        }
    }
    start
}

fn to_completion_item(c: completion::Completion) -> CompletionItem {
    CompletionItem {
        kind: Some(match c.kind {
            completion::CompletionKind::Unit => CompletionItemKind::MODULE,
            completion::CompletionKind::Capability => CompletionItemKind::INTERFACE,
            completion::CompletionKind::Type => CompletionItemKind::STRUCT,
            completion::CompletionKind::Keyword => CompletionItemKind::KEYWORD,
            completion::CompletionKind::Snippet => CompletionItemKind::SNIPPET,
            completion::CompletionKind::Variant => CompletionItemKind::ENUM_MEMBER,
            completion::CompletionKind::Member => CompletionItemKind::METHOD,
            completion::CompletionKind::Field => CompletionItemKind::FIELD,
            completion::CompletionKind::Constructor => CompletionItemKind::CONSTRUCTOR,
            completion::CompletionKind::Function => CompletionItemKind::FUNCTION,
            completion::CompletionKind::Constant => CompletionItemKind::CONSTANT,
        }),
        // Snippet items carry `${n:…}` tab stops; everything else inserts its
        // label verbatim (the default).
        insert_text_format: c.insert_text.as_ref().map(|_| InsertTextFormat::SNIPPET),
        insert_text: c.insert_text,
        label: c.label,
        detail: c.detail,
        // #1888: a field's or variant's doc, rendered as Markdown.
        documentation: c.documentation.map(|value| {
            Documentation::MarkupContent(MarkupContent {
                kind: MarkupKind::Markdown,
                value,
            })
        }),
        ..Default::default()
    }
}

/// The byte offset of an LSP `(line, character)` position in `text`,
/// clamped to the end of the document when the position lies past it.
/// LSP positions count UTF-16 code units, so this goes through the shared
/// converter — a byte-faithful reading misplaces the cursor on any line
/// with non-ASCII text before it.
fn cursor_offset(text: &str, pos: Position) -> usize {
    crate::position::position_to_offset(text, pos).unwrap_or(text.len())
}

/// v0.34 (ADR 0067): a serializable mirror of [`bynk_check::index::SymbolKey`] for
/// round-tripping through `CallHierarchyItem.data` — the index kind isn't
/// `Serialize`, so the kind travels as its `display()` string.
#[derive(serde::Serialize, serde::Deserialize)]
struct SerKey {
    unit: String,
    kind: String,
    name: String,
}

impl From<&bynk_check::index::SymbolKey> for SerKey {
    fn from(k: &bynk_check::index::SymbolKey) -> Self {
        SerKey {
            unit: k.unit.clone(),
            kind: k.kind.display().to_string(),
            name: k.name.clone(),
        }
    }
}

impl SerKey {
    /// Recover a `SymbolKey` from a `CallHierarchyItem`'s `data`. `None` for a
    /// missing/garbled payload or an unknown kind — the follow-up then returns
    /// no calls rather than guessing.
    fn read(data: &Option<serde_json::Value>) -> Option<bynk_check::index::SymbolKey> {
        let sk: SerKey = serde_json::from_value(data.as_ref()?.clone()).ok()?;
        let kind = match sk.kind.as_str() {
            "type" => bynk_check::index::SymbolKind::Type,
            "fn" => bynk_check::index::SymbolKind::Fn,
            "capability" => bynk_check::index::SymbolKind::Capability,
            "service" => bynk_check::index::SymbolKind::Service,
            "agent" => bynk_check::index::SymbolKind::Agent,
            "provider" => bynk_check::index::SymbolKind::Provider,
            _ => return None,
        };
        Some(bynk_check::index::SymbolKey {
            unit: sk.unit,
            kind,
            name: sk.name,
        })
    }
}

fn lsp_symbol_kind(kind: bynk_check::index::SymbolKind) -> SymbolKind {
    match kind {
        bynk_check::index::SymbolKind::Type => SymbolKind::STRUCT,
        bynk_check::index::SymbolKind::Fn => SymbolKind::FUNCTION,
        bynk_check::index::SymbolKind::Capability => SymbolKind::INTERFACE,
        bynk_check::index::SymbolKind::Service | bynk_check::index::SymbolKind::Agent => {
            SymbolKind::CLASS
        }
        bynk_check::index::SymbolKind::Provider => SymbolKind::OBJECT,
        bynk_check::index::SymbolKind::Method => SymbolKind::METHOD,
        bynk_check::index::SymbolKind::CapabilityOp => SymbolKind::METHOD,
        bynk_check::index::SymbolKind::Field => SymbolKind::FIELD,
        bynk_check::index::SymbolKind::Actor => SymbolKind::INTERFACE,
        bynk_check::index::SymbolKind::Handler => SymbolKind::METHOD,
        bynk_check::index::SymbolKind::Messages => SymbolKind::STRUCT,
        // #1883: an asset is a named value that never changes — as the
        // document outline already shows it (`document_symbols::asset_symbol`).
        bynk_check::index::SymbolKind::Asset => SymbolKind::CONSTANT,
        // #2387: likewise a named string constant.
        bynk_check::index::SymbolKind::Embed => SymbolKind::CONSTANT,
    }
}

/// Whether a watched-file URI names a `bynk.toml` manifest — the trigger for a
/// live config reload. Matches on the file-name component (not a path suffix),
/// so a file like `notbynk.toml` doesn't spuriously fire.
fn is_bynk_toml(uri: &Url) -> bool {
    let Ok(path) = uri.to_file_path() else {
        return false;
    };
    path.file_name().and_then(|n| n.to_str()) == Some("bynk.toml")
}

/// The `codeDescription` link for a diagnostic `code` (#853): a clickable link
/// to the code's Book explanation when the compiler curates one, else `None`
/// (the designed graceful-fallback state — an uncurated code renders no link,
/// which is not an error). Split out from [`make_diagnostic`] so the
/// mapped→`Some` / uncurated→`None` contract is directly testable.
fn code_description(code: &str) -> Option<CodeDescription> {
    let href = Url::parse(&bynk_syntax::diagnostics::explain(code)?.href()).ok()?;
    Some(CodeDescription { href })
}

#[cfg(test)]
mod code_description_tests {
    use super::code_description;

    #[test]
    fn mapped_code_gets_a_valid_book_link() {
        let cd = code_description("bynk.resolve.unknown_type")
            .expect("a curated code produces a codeDescription");
        assert_eq!(cd.href.scheme(), "https");
        assert_eq!(cd.href.host_str(), Some("bynk-lang.org"));
        assert!(cd.href.path().starts_with("/book/"));
    }

    #[test]
    fn uncurated_code_gets_no_link() {
        // A real code with no curated explanation, and a nonsense code, both
        // fall back to no link (graceful — not an error).
        assert!(code_description("bynk.resolve.duplicate_type").is_none());
        assert!(code_description("bynk.not.a_real_code").is_none());
    }

    #[test]
    fn every_curated_explanation_yields_a_parseable_url() {
        // Guards that no curated href ever silently drops its link because
        // `Url::parse` rejected it.
        for e in bynk_syntax::diagnostics::EXPLANATIONS {
            assert!(
                code_description(e.code).is_some(),
                "curated explanation `{}` produced no codeDescription — its href \
                 `{}` did not parse as a URL",
                e.code,
                e.href()
            );
        }
    }
}

/// Every analysed project file's URI and the positions of the snapshot it
/// was analysed at, by identity path — where [`make_diagnostic`] places a
/// secondary label that lies in a sibling file.
type ProjectFiles<'a> = std::collections::HashMap<PathBuf, (Url, crate::position::PositionMap<'a>)>;

fn make_diagnostic(
    d: &bynk_ide::Diagnostic,
    positions: &crate::position::PositionMap,
    uri: &Url,
    files: &ProjectFiles<'_>,
) -> Diagnostic {
    let range = positions.range(d.error.span);
    let severity = match d.severity {
        bynk_syntax::Severity::Error => DiagnosticSeverity::ERROR,
        bynk_syntax::Severity::Warning => DiagnosticSeverity::WARNING,
    };
    let mut related_information: Vec<DiagnosticRelatedInformation> = Vec::new();
    let mut label_notes: Vec<&str> = Vec::new();
    for (i, (span, msg)) in d.error.labels.iter().enumerate() {
        // A label's span is an offset into the text of the file it lies in:
        // this document's, or a sibling's in the same project, which has its
        // own URI and positions. A label in a file with no snapshot (a
        // toolchain-injected unit) has nowhere to point, so it is a note.
        let (label_uri, label_positions) = match d.label_file(i) {
            bynk_ide::LabelFile::Primary => (uri, positions),
            bynk_ide::LabelFile::Other(path) => match files.get(path) {
                Some((u, p)) => (u, p),
                None => {
                    label_notes.push(msg);
                    continue;
                }
            },
            // A first-party unit (`bynk.bynk`) has no document to point at.
            bynk_ide::LabelFile::FirstParty { .. } | bynk_ide::LabelFile::Unavailable => {
                label_notes.push(msg);
                continue;
            }
        };
        related_information.push(DiagnosticRelatedInformation {
            location: Location {
                uri: label_uri.clone(),
                range: label_positions.range(*span),
            },
            message: msg.clone(),
        });
    }
    let mut message = d.error.message.clone();
    for note in d.error.notes.iter().map(String::as_str).chain(label_notes) {
        message.push_str("\n\n");
        message.push_str("note: ");
        message.push_str(note);
    }
    Diagnostic {
        range,
        severity: Some(severity),
        code: Some(NumberOrString::String(d.error.category.to_string())),
        // #853: a curated code carries a `codeDescription` link to its Book
        // explanation (rendered as a link on the code in Problems/hover); an
        // uncurated code has no entry and stays `None` — the designed
        // graceful-fallback state, not an error.
        code_description: code_description(d.error.category),
        source: Some(SERVER_NAME.to_string()),
        message,
        related_information: if related_information.is_empty() {
            None
        } else {
            Some(related_information)
        },
        tags: None,
        data: None,
    }
}

/// Slice C: the server's entry point, moved out of `main.rs` so the crate
/// has a `[lib]` target. `main.rs` is now a thin shim over this.
///
/// #1667: returns the process's exit code: success after an orderly
/// `shutdown`, failure (with a line on stderr) when the session ended any
/// other way.
pub async fn run() -> std::process::ExitCode {
    // Answer `--version`/`-V` and exit before entering the stdio LSP loop, so
    // tooling (e.g. the VS Code status bar) can query the version without the
    // server blocking on stdin.
    if std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
    {
        println!("{SERVER_NAME} {SERVER_VERSION}");
        return std::process::ExitCode::SUCCESS;
    }
    // Logging to ~/.bynk-lsp.log. Default level: warn; tunable via
    // RUST_LOG or the LSP client's trace setting.
    if let Some(home) = std::env::var_os("HOME") {
        let path: PathBuf = PathBuf::from(home).join(".bynk-lsp.log");
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use tracing_subscriber::prelude::*;
            let env_filter = tracing_subscriber::EnvFilter::try_from_env("BYNK_LSP_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
            let file_layer = tracing_subscriber::fmt::layer()
                .with_writer(std::sync::Mutex::new(file))
                .with_ansi(false);
            tracing_subscriber::registry()
                .with(env_filter)
                .with(file_layer)
                .try_init()
                .ok();
        }
    }
    tracing::info!("bynkc-lsp v{} starting", SERVER_VERSION);
    let stdout = tokio::io::stdout();
    // #1667: the server reads a re-framed copy of stdin, so one malformed
    // message is skipped rather than ending the session (see `transport`).
    let (server_input, pump_output) = tokio::io::duplex(1 << 16);
    tokio::spawn(transport::pump(tokio::io::stdin(), pump_output));
    let shutdown_requested = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&shutdown_requested);
    // #846: this server's first custom (non-standard) request — everything
    // else is a `LanguageServer` trait method, registered automatically by
    // `LspService::new`. `bynk/sequenceModel` has no trait slot, so it needs
    // the builder's `custom_method` instead.
    let (service, socket) =
        LspService::build(move |client| Backend::with_shutdown_flag(client, Arc::clone(&flag)))
            .custom_method("bynk/sequenceModel", Backend::sequence_model)
            .custom_method("bynk/agentStateModel", Backend::agent_state_model)
            .custom_method("bynk/documentationModel", Backend::documentation_model)
            .custom_method("bynk/architectureModel", Backend::architecture_model)
            .custom_method("bynk/wireContract", Backend::wire_contract)
            .custom_method("bynk/explain", Backend::explain)
            .custom_method("bynk/emittedOutput", Backend::emitted_output)
            .finish();
    Server::new(server_input, stdout, socket)
        .serve(service)
        .await;
    if shutdown_requested.load(std::sync::atomic::Ordering::SeqCst) {
        std::process::ExitCode::SUCCESS
    } else {
        eprintln!(
            "bynkc-lsp: the session ended without a `shutdown` request \
             (the client closed the connection, or sent `exit` early); see ~/.bynk-lsp.log"
        );
        std::process::ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -- Slice A: the project model, driven through `Backend` ---------------
    //
    // These are the crate's first *behaviour-over-time* tests: they drive the
    // real `Backend` — the layer the track doc (§4.1) notes has always been
    // testable in-crate via `LspService::new(Backend::new)`, and never was.
    // Everything else in this module asserts *static* shape.
    //
    // Hermetic on purpose. `bynk-lsp` is published and `Cargo.toml`'s `exclude`
    // list can only drop `tests/*.rs`, never this file — so an in-crate test
    // reading a sibling directory would fail `cargo test` on the released
    // tarball. (`find_source_root_walks_up_to_the_nearest_src` below already
    // does exactly that; not this slice's to fix.) The sibling-reading fixtures
    // live in `tests/project_model.rs`, which *is* excluded.

    /// A throwaway project, removed on drop — including on panic.
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch_project(tag: &str, files: &[(&str, &str)]) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "bynk_lsp_sliceA_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, body) in files {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }
        Scratch(dir)
    }

    /// Build a `Backend` over a real `LspService`, rooted at `root`.
    ///
    /// `LspService::new(Backend::new)` is what `main` itself calls — the
    /// `Client` it hands back is the only thing `Backend` needed, and it has
    /// been available for this since the server was written.
    async fn backend_at(root: &std::path::Path) -> Backend {
        let (service, _socket) = tower_lsp::LspService::new(Backend::new);
        let backend = service.inner().clone();
        // Slice D: seed one project entry, keyed by the **canonical** root so a
        // request's `resolve_root`-based routing lands on the same key.
        let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        {
            let mut state = backend.state.write().await;
            state.folders.push(canonical.clone());
            state.projects.insert(
                canonical.clone(),
                ProjectState {
                    config: project::load_config(&canonical).unwrap_or_default(),
                    ..Default::default()
                },
            );
        }
        backend
    }

    /// #1667: a panic in an analysis round reaches the client once per
    /// failure streak, as a `window/showMessage`, and the streak ends at the
    /// next clean round. Before, the `JoinError` was dropped without a trace.
    #[tokio::test]
    async fn a_panicking_round_tells_the_client_once_and_clears_on_success() {
        use futures::StreamExt;
        use tower::{Service, ServiceExt};

        let s = scratch_project(
            "panic",
            &[
                ("bynk.toml", "[project]\nname = \"panic\"\n"),
                ("src/thing.bynk", "context thing\n"),
            ],
        );
        let (mut service, socket) = tower_lsp::LspService::new(Backend::new);
        // The client sends nothing until the server is initialised.
        let initialize = tower_lsp::jsonrpc::Request::build("initialize")
            .params(serde_json::json!({ "capabilities": {} }))
            .id(1)
            .finish();
        service
            .ready()
            .await
            .unwrap()
            .call(initialize)
            .await
            .unwrap();
        let initialized = tower_lsp::jsonrpc::Request::build("initialized")
            .params(serde_json::json!({}))
            .finish();
        service
            .ready()
            .await
            .unwrap()
            .call(initialized)
            .await
            .unwrap();
        // Drain the client side as the server writes to it: its channel is
        // small, so an unread socket would block the server's next
        // notification (a `publishDiagnostics`, or the `showMessage` itself).
        let shown = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let drain = {
            let shown = Arc::clone(&shown);
            tokio::spawn(socket.for_each(move |message| {
                if message.method() == "window/showMessage" {
                    shown
                        .lock()
                        .unwrap()
                        .push(format!("{:?}", message.params()));
                }
                async {}
            }))
        };
        let backend = service.inner().clone();
        let canonical = s.0.canonicalize().unwrap();
        backend
            .state
            .write()
            .await
            .projects
            .insert(canonical.clone(), ProjectState::default());
        let failed = |b: &Backend| {
            let b = b.clone();
            let root = canonical.clone();
            async move { b.state.read().await.projects[&root].analysis_failed }
        };

        let inject = |b: &Backend| {
            let b = b.clone();
            let root = canonical.clone();
            async move {
                b.state
                    .write()
                    .await
                    .projects
                    .get_mut(&root)
                    .unwrap()
                    .panic_next_round = true;
            }
        };
        inject(&backend).await;
        backend.run_project_diagnostics(canonical.clone()).await;
        assert!(failed(&backend).await, "the failed round is recorded");
        // A second failure in the same streak says nothing new.
        inject(&backend).await;
        backend.run_project_diagnostics(canonical.clone()).await;

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        {
            let shown = shown.lock().unwrap();
            assert_eq!(shown.len(), 1, "told once per streak: {shown:?}");
            assert!(shown[0].contains("internal error"), "{shown:?}");
        }

        backend.run_project_diagnostics(canonical.clone()).await;
        assert!(!failed(&backend).await, "a clean round ends the streak");
        // A failure after the streak ended is a new streak, and is told.
        inject(&backend).await;
        backend.run_project_diagnostics(canonical.clone()).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(shown.lock().unwrap().len(), 2, "a new streak is told again");
        drain.abort();
    }

    /// A diagnostic's secondary label in a sibling file of a multi-file unit
    /// points at that file's URI, at its position there. Before, every label
    /// took the diagnostic's own URI, so "previously declared here" for a
    /// service declared in `a_web.bynk` pointed into `z_more.bynk`, at the same
    /// byte offsets.
    #[tokio::test]
    async fn a_label_in_a_sibling_file_points_at_that_file() {
        use futures::StreamExt;
        use tower::{Service, ServiceExt};

        let service_src =
            "service api {\n  on call() -> Effect[Int] {\n    Effect.pure(1)\n  }\n}\n";
        let s = scratch_project(
            "sibling_label",
            &[
                ("bynk.toml", "[project]\nname = \"demo\"\n"),
                (
                    "src/app/a_web.bynk",
                    &format!("context app\n\n{service_src}"),
                ),
                (
                    "src/app/z_more.bynk",
                    &format!("context app\n\n\n\n\n{service_src}"),
                ),
            ],
        );
        let (mut service, socket) = tower_lsp::LspService::new(Backend::new);
        for request in [
            tower_lsp::jsonrpc::Request::build("initialize")
                .params(serde_json::json!({ "capabilities": {} }))
                .id(1)
                .finish(),
            tower_lsp::jsonrpc::Request::build("initialized")
                .params(serde_json::json!({}))
                .finish(),
        ] {
            service.ready().await.unwrap().call(request).await.unwrap();
        }
        let published = Arc::new(std::sync::Mutex::new(Vec::<PublishDiagnosticsParams>::new()));
        let drain = {
            let published = Arc::clone(&published);
            tokio::spawn(socket.for_each(move |message| {
                if message.method() == "textDocument/publishDiagnostics"
                    && let Some(params) = message.params()
                {
                    published
                        .lock()
                        .unwrap()
                        .push(serde_json::from_value(params.clone()).unwrap());
                }
                async {}
            }))
        };
        let backend = service.inner().clone();
        let canonical = s.0.canonicalize().unwrap();
        backend
            .state
            .write()
            .await
            .projects
            .insert(canonical.clone(), ProjectState::default());
        backend.run_project_diagnostics(canonical.clone()).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drain.abort();

        let a_web = Url::from_file_path(canonical.join("src/app/a_web.bynk")).unwrap();
        let z_more = Url::from_file_path(canonical.join("src/app/z_more.bynk")).unwrap();
        let published = published.lock().unwrap();
        let duplicate = published
            .iter()
            .filter(|p| p.uri == z_more)
            .flat_map(|p| &p.diagnostics)
            .find(|d| {
                d.code
                    == Some(NumberOrString::String(
                        "bynk.resolve.duplicate_service".into(),
                    ))
            })
            .unwrap_or_else(|| panic!("z_more.bynk must report the duplicate: {published:?}"));
        let related = duplicate
            .related_information
            .as_deref()
            .expect("the duplicate carries its \"previously declared here\" label");
        assert_eq!(related.len(), 1, "{related:?}");
        assert_eq!(related[0].message, "previously declared here");
        assert_eq!(
            related[0].location.uri, a_web,
            "the label lies in a_web.bynk, not the diagnostic's own file"
        );
        // `service api {` is line 2 (0-based) of a_web.bynk; `api` at column 8.
        assert_eq!(
            related[0].location.range,
            Range::new(Position::new(2, 8), Position::new(2, 11)),
        );
    }

    /// #2081: a round reconciles events against the committed
    /// `bynk.schema.lock`, read-only, so a non-additive change reaches the
    /// editor as `bynk.event.non_additive_schema_change`, as it reaches
    /// `bynk check`. The lock is never written, and a changed lock is re-read
    /// on the next round (its stamp moved), clearing the diagnostic.
    #[tokio::test]
    async fn a_round_reports_a_non_additive_event_change_against_the_lock() {
        use futures::StreamExt;
        use tower::{Service, ServiceExt};

        const CODE: &str = "bynk.event.non_additive_schema_change";
        const LOCK_V1: &str = "version = 1\n\n\
            [events.\"commerce.order.PaymentConfirmed\"]\nschema = 1\n\n\
            [[events.\"commerce.order.PaymentConfirmed\".fields]]\n\
            name = \"orderId\"\ntype = \"String\"\ndefault = false\n";
        // The same event recorded with the source's current shape.
        const LOCK_V2: &str = "version = 1\n\n\
            [events.\"commerce.order.PaymentConfirmed\"]\nschema = 1\n\n\
            [[events.\"commerce.order.PaymentConfirmed\".fields]]\n\
            name = \"orderId\"\ntype = \"String\"\ndefault = false\n\n\
            [[events.\"commerce.order.PaymentConfirmed\".fields]]\n\
            name = \"region\"\ntype = \"String\"\ndefault = false\n";
        // `region` added with no default: not additive against `LOCK_V1`.
        const SOURCE: &str = "context commerce.order\n\n\
            consumes bynk { Events }\n\n\
            event PaymentConfirmed = {\n  orderId: String,\n  region: String,\n}\n\n\
            service markPaid {\n  on call(orderId: String) -> Effect[()] given Events {\n    \
            Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId, region: \"US\" })\n  \
            }\n}\n";

        let s = scratch_project(
            "schema_lock",
            &[
                ("bynk.toml", "[project]\nname = \"shop\"\n"),
                ("src/commerce/order.bynk", SOURCE),
                ("bynk.schema.lock", LOCK_V1),
            ],
        );
        let (mut service, socket) = tower_lsp::LspService::new(Backend::new);
        for request in [
            tower_lsp::jsonrpc::Request::build("initialize")
                .params(serde_json::json!({ "capabilities": {} }))
                .id(1)
                .finish(),
            tower_lsp::jsonrpc::Request::build("initialized")
                .params(serde_json::json!({}))
                .finish(),
        ] {
            service.ready().await.unwrap().call(request).await.unwrap();
        }
        let published = Arc::new(std::sync::Mutex::new(Vec::<PublishDiagnosticsParams>::new()));
        let drain = {
            let published = Arc::clone(&published);
            tokio::spawn(socket.for_each(move |message| {
                if message.method() == "textDocument/publishDiagnostics"
                    && let Some(params) = message.params()
                {
                    published
                        .lock()
                        .unwrap()
                        .push(serde_json::from_value(params.clone()).unwrap());
                }
                async {}
            }))
        };
        let backend = service.inner().clone();
        let canonical = s.0.canonicalize().unwrap();
        backend
            .state
            .write()
            .await
            .projects
            .insert(canonical.clone(), ProjectState::default());
        // The project-level diagnostic anchors on `bynk.toml`.
        let manifest = Url::from_file_path(canonical.join("bynk.toml")).unwrap();
        let reported = |published: &[PublishDiagnosticsParams]| {
            published
                .iter()
                .rfind(|p| p.uri == manifest)
                .is_some_and(|p| {
                    p.diagnostics
                        .iter()
                        .any(|d| d.code == Some(NumberOrString::String(CODE.into())))
                })
        };

        backend.run_project_diagnostics(canonical.clone()).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            reported(&published.lock().unwrap()),
            "the round must report {CODE} against the committed lock: {:?}",
            published.lock().unwrap()
        );
        assert_eq!(
            std::fs::read_to_string(canonical.join("bynk.schema.lock")).unwrap(),
            LOCK_V1,
            "the editor must never write the lock"
        );

        // Record the new shape, as `bynkc compile` would after a fix: the next
        // round re-reads the lock and the diagnostic clears.
        std::fs::write(canonical.join("bynk.schema.lock"), LOCK_V2).unwrap();
        backend.run_project_diagnostics(canonical.clone()).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !reported(&published.lock().unwrap()),
            "a lock matching the source must clear {CODE}: {:?}",
            published.lock().unwrap()
        );

        // Restore the stale lock: it is read again and the diagnostic returns.
        std::fs::write(canonical.join("bynk.schema.lock"), LOCK_V1).unwrap();
        backend.run_project_diagnostics(canonical.clone()).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            reported(&published.lock().unwrap()),
            "the old lock is read again"
        );
        // No lock at all: a fresh project, nothing to report.
        std::fs::remove_file(canonical.join("bynk.schema.lock")).unwrap();
        backend.run_project_diagnostics(canonical.clone()).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drain.abort();
        assert!(
            !reported(&published.lock().unwrap()),
            "with no lock there is nothing to report: {:?}",
            published.lock().unwrap()
        );
        assert!(
            !canonical.join("bynk.schema.lock").exists(),
            "the editor must not create a lock"
        );
    }

    /// State-migrations S0 (#2204): a round reconciles agent state shapes
    /// against the committed lock too, so renaming a `store` field reaches the
    /// editor as `bynk.agent.non_additive_state_change` on the agent's own
    /// file, as it reaches `bynk check`. The lock is never written.
    #[tokio::test]
    async fn a_round_reports_a_non_additive_agent_state_change_against_the_lock() {
        use futures::StreamExt;
        use tower::{Service, ServiceExt};

        const CODE: &str = "bynk.agent.non_additive_state_change";
        const LOCK: &str = "version = 2\n\n[events]\n\n\
            [agents.\"shop.Cart\"]\nversion = 1\nfloor = 1\n\n\
            [[agents.\"shop.Cart\".shapes]]\nversion = 1\nadditive = false\n\n\
            [[agents.\"shop.Cart\".shapes.fields]]\n\
            name = \"owner\"\nkind = \"Cell\"\ntype = \"String\"\nrefinement = \"String\"\n";
        const SOURCE: &str = "context shop\n\nagent Cart {\n  key id: String\n\n  \
            store ownerName: Cell[String]\n\n  on call ping() -> Effect[Int] {\n    1\n  }\n}\n";

        let s = scratch_project(
            "agent_schema_lock",
            &[
                ("bynk.toml", "[project]\nname = \"shop\"\n"),
                ("src/shop/cart.bynk", SOURCE),
                ("bynk.schema.lock", LOCK),
            ],
        );
        let (mut service, socket) = tower_lsp::LspService::new(Backend::new);
        for request in [
            tower_lsp::jsonrpc::Request::build("initialize")
                .params(serde_json::json!({ "capabilities": {} }))
                .id(1)
                .finish(),
            tower_lsp::jsonrpc::Request::build("initialized")
                .params(serde_json::json!({}))
                .finish(),
        ] {
            service.ready().await.unwrap().call(request).await.unwrap();
        }
        let published = Arc::new(std::sync::Mutex::new(Vec::<PublishDiagnosticsParams>::new()));
        let drain = {
            let published = Arc::clone(&published);
            tokio::spawn(socket.for_each(move |message| {
                if message.method() == "textDocument/publishDiagnostics"
                    && let Some(params) = message.params()
                {
                    published
                        .lock()
                        .unwrap()
                        .push(serde_json::from_value(params.clone()).unwrap());
                }
                async {}
            }))
        };
        let backend = service.inner().clone();
        let canonical = s.0.canonicalize().unwrap();
        backend
            .state
            .write()
            .await
            .projects
            .insert(canonical.clone(), ProjectState::default());
        let source = Url::from_file_path(canonical.join("src/shop/cart.bynk")).unwrap();

        backend.run_project_diagnostics(canonical.clone()).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        drain.abort();
        let published = published.lock().unwrap();
        assert!(
            published
                .iter()
                .rfind(|p| p.uri == source)
                .is_some_and(|p| p
                    .diagnostics
                    .iter()
                    .any(|d| d.code == Some(NumberOrString::String(CODE.into())))),
            "the round must report {CODE} on the agent's file: {published:?}"
        );
        assert_eq!(
            std::fs::read_to_string(canonical.join("bynk.schema.lock")).unwrap(),
            LOCK,
            "the editor must never write the lock"
        );
    }

    /// A label with nowhere to point — in a toolchain-injected unit, or in a
    /// file the round has no snapshot of — is a `note:` on the message, never
    /// a `relatedInformation` entry at this document's own positions.
    #[test]
    fn a_label_with_no_file_to_point_at_becomes_a_note() {
        let text = "context app\n";
        let positions = crate::position::PositionMap::new(text);
        let uri = Url::parse("file:///z.bynk").unwrap();
        let d = bynk_ide::Diagnostic {
            severity: bynk_syntax::Severity::Error,
            error: bynk_syntax::CompileError::new(
                "bynk.test",
                bynk_syntax::span::Span::new(0, 7),
                "msg",
            )
            .with_label(bynk_syntax::span::Span::new(0, 1), "in an injected unit")
            .with_label(bynk_syntax::span::Span::new(0, 1), "in an unanalysed file")
            .with_label(bynk_syntax::span::Span::new(0, 1), "in no known file"),
            label_files: vec![
                bynk_ide::LabelFile::FirstParty {
                    identity_path: "bynk.bynk",
                    source: "commons bynk\n",
                },
                bynk_ide::LabelFile::Other(PathBuf::from("src/missing.bynk")),
                bynk_ide::LabelFile::Unavailable,
            ],
        };
        let diagnostic = make_diagnostic(&d, &positions, &uri, &ProjectFiles::new());
        assert!(
            diagnostic.related_information.is_none(),
            "{:?}",
            diagnostic.related_information
        );
        assert_eq!(
            diagnostic.message,
            "msg\n\nnote: in an injected unit\n\nnote: in an unanalysed file\n\nnote: in no known file"
        );
    }

    /// Test helpers for the single-project behaviour tests (each builds exactly
    /// one project via `backend_at` or an equivalent insert). They read whatever
    /// the one entry's key is, so a test need not thread the canonical root.
    impl Backend {
        async fn test_root(&self) -> PathBuf {
            self.state
                .read()
                .await
                .projects
                .keys()
                .next()
                .cloned()
                .expect("a test project entry")
        }
        async fn run_round(&self) {
            let root = self.test_root().await;
            self.run_project_diagnostics(root).await;
        }
        async fn test_analysis(&self) -> Option<Arc<Analysis>> {
            let root = self.test_root().await;
            self.project_analysis(&root).await
        }
        async fn test_round_started(&self) -> u64 {
            let root = self.test_root().await;
            self.state
                .read()
                .await
                .projects
                .get(&root)
                .map(|p| p.analysis_round_started)
                .unwrap_or(0)
        }
    }

    /// The slice, end to end through the server: a round covers **every**
    /// `include` tree, and each file keeps a distinct project-relative identity
    /// (ADR 0198). Before slice A the round was handed `<root>/src` and the
    /// `tests/` tree did not exist as far as the LSP was concerned.
    #[tokio::test]
    async fn a_round_covers_every_include_tree() {
        let s = scratch_project(
            "round",
            &[
                ("bynk.toml", "[project]\nname = \"round\"\n"),
                ("src/thing.bynk", "context thing\n"),
                // Same basename, second root — the ADR 0198 collision.
                ("tests/thing.bynk", "suite thing\n"),
            ],
        );
        let backend = backend_at(&s.0).await;
        backend.run_round().await;

        let analysis = backend.test_analysis().await.expect("a round committed");
        let mut keys: Vec<String> = analysis
            .snapshots
            .keys()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["src/thing.bynk", "tests/thing.bynk"],
            "the round must cover both include trees, with distinct identities",
        );
    }

    /// The identity a request resolves through. `uri_to_rel` is one
    /// `strip_prefix` against the project root — total across `include` trees,
    /// where the old `src` base could only ever name files in one of them.
    #[tokio::test]
    async fn a_uri_in_any_include_tree_resolves_to_its_analysed_file() {
        let s = scratch_project(
            "uri",
            &[
                ("bynk.toml", "[project]\nname = \"uri\"\n"),
                ("src/thing.bynk", "context thing\n"),
                ("tests/thing.bynk", "suite thing\n"),
            ],
        );
        let backend = backend_at(&s.0).await;
        backend.run_round().await;
        let analysis = backend.test_analysis().await.expect("round");

        for (rel, label) in [
            ("src/thing.bynk", "primary"),
            ("tests/thing.bynk", "secondary"),
        ] {
            let abs = s.0.join(rel);
            let uri = Url::from_file_path(abs.canonicalize().unwrap_or(abs)).unwrap();
            let resolved = Backend::uri_to_rel(&analysis, &uri)
                .unwrap_or_else(|| panic!("{label} root URI must resolve"));
            assert_eq!(
                resolved.to_string_lossy().replace('\\', "/"),
                rel,
                "a {label}-tree URI must name its own analysed file",
            );
            assert!(
                analysis.snapshots.contains_key(&resolved),
                "…and that file must be in the round",
            );
        }
    }

    /// Finding #62: `project_content` must exclude the calling file's own
    /// path — `bynk-ide`'s completion helpers already parse it fresh from the
    /// live buffer, so leaving it in the map would additionally serve its
    /// on-disk copy (stale relative to any unsaved edit) alongside the buffer
    /// parse.
    #[tokio::test]
    async fn project_content_excludes_the_calling_file() {
        let s = scratch_project(
            "self_excl",
            &[
                ("bynk.toml", "[project]\nname = \"self_excl\"\n"),
                ("a.bynk", "context a\n"),
                ("b.bynk", "context b\n"),
            ],
        );
        let backend = backend_at(&s.0).await;
        let abs = s.0.join("a.bynk");
        let uri = Url::from_file_path(abs.canonicalize().unwrap_or(abs)).unwrap();
        let content = backend
            .project_content(&uri)
            .await
            .expect("a project root resolves to a content map");
        assert!(
            !content.keys().any(|p| p.file_name().unwrap() == "a.bynk"),
            "the calling file's own path must not be in its own project content map: {content:?}"
        );
        assert!(
            content.keys().any(|p| p.file_name().unwrap() == "b.bynk"),
            "a sibling project file must still be present: {content:?}"
        );
        assert_eq!(
            content
                .iter()
                .find(|(p, _)| p.file_name().unwrap() == "b.bynk")
                .map(|(_, text)| text.as_str()),
            Some("context b\n"),
            "the sibling's content must be the real file content, not just its path"
        );
    }

    /// `exclude` reaches the server, not just the compiler. `project.rs` used to
    /// parse it and throw it away — its own comment said the analyse walk "does
    /// not yet prune by `exclude`".
    #[tokio::test]
    async fn an_excluded_tree_is_not_analysed() {
        let s = scratch_project(
            "excl",
            &[
                (
                    "bynk.toml",
                    "[project]\nname = \"excl\"\n\n[paths]\ninclude = [\".\"]\nexclude = [\"generated\"]\n",
                ),
                ("a.bynk", "context a\n"),
                ("generated/gen.bynk", "context gen\n"),
            ],
        );
        let backend = backend_at(&s.0).await;
        backend.run_round().await;
        let analysis = backend.test_analysis().await.expect("round");
        let keys: Vec<String> = analysis
            .snapshots
            .keys()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(keys, vec!["a.bynk"], "excluded trees stay out of the round");
    }

    /// CI repro (#653): the VS Code extension's fixture workspace — a legacy
    /// `[paths] src`/`tests` manifest (keys ADR 0147 retired, so
    /// `read_project_paths` ignores them → `conventional()` → `["src"]`) with a
    /// dotted commons in `src/`. Drives the real `references` handler.
    #[tokio::test]
    async fn references_resolve_in_the_vscode_fixture_layout() {
        let s = scratch_project(
            "vsc",
            &[
                (
                    "bynk.toml",
                    "[project]\nname = \"fixture\"\nversion = \"0.1.0\"\n\n[paths]\nsrc = \"src\"\ntests = \"tests\"\n",
                ),
                (
                    "src/text.bynk",
                    "commons fixture.text\n\nfn shout(s: String) -> String {\n  s\n}\n\nfn greet(name: String) -> String {\n  \"Hi, \\(shout(name))!\"\n}\n",
                ),
            ],
        );
        // Root exactly as `initialize` does: resolve from the workspace folder.
        let (root, config) = Backend::resolve_root(&s.0).expect("bynk.toml is present");
        assert_eq!(root, s.0, "the manifest's directory is the project root");

        let (service, _socket) = tower_lsp::LspService::new(Backend::new);
        let backend = service.inner().clone();
        {
            let mut st = backend.state.write().await;
            // Key by the canonical root, so the entry matches `references`'
            // URI-based routing (`resolve_root` canonicalises).
            let canonical = root.canonicalize().unwrap_or_else(|_| root.clone());
            st.projects.insert(
                canonical,
                ProjectState {
                    config,
                    ..Default::default()
                },
            );
        }
        backend.run_round().await;

        let analysis = backend.test_analysis().await.expect("a round committed");
        let keys: Vec<String> = analysis
            .snapshots
            .keys()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert_eq!(keys, vec!["src/text.bynk"], "the fixture file is analysed");

        // The URI the editor sends, mapped through the round's identity.
        let abs = s.0.join("src/text.bynk");
        let uri = Url::from_file_path(abs.canonicalize().unwrap_or(abs)).unwrap();
        let rel = Backend::uri_to_rel(&analysis, &uri).expect("URI resolves into the round");
        assert_eq!(rel, PathBuf::from("src/text.bynk"));

        // `shout`'s declaration site: line 2 (0-based), at `fn shout`.
        let text = analysis.snapshots.get(&rel).expect("snapshot present");
        let decl = text.find("shout").expect("`shout` in source");
        let pos = crate::position::offset_to_position(text, decl);

        let refs = backend
            .references(ReferenceParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                    position: pos,
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: ReferenceContext {
                    include_declaration: true,
                },
            })
            .await
            .expect("references must not error");
        let found = refs.unwrap_or_default();
        assert!(
            !found.is_empty(),
            "`shout` is referenced by `greet` — references must resolve; got none",
        );
    }

    // -- Slice B: the freshness contract, driven through a real Backend -------
    //
    // These are behaviour-over-time tests (§4.1): they edit a buffer and then
    // make a request, asserting the request answers against the *new* text.
    // The static tests above can't see this — the defect lives between the
    // edit and the request, which only a driven sequence exercises.

    /// Open `src/a.bynk`, round it, then edit and drive `did_change`. `uri`,
    /// the round-1 relative path, and the edited version are returned.
    async fn open_round_edit(
        backend: &Backend,
        root: &std::path::Path,
        v1_text: &str,
        v2_text: &str,
    ) -> (Url, PathBuf) {
        let abs = root.join("src/a.bynk");
        let uri = Url::from_file_path(abs.canonicalize().unwrap_or(abs)).unwrap();

        backend
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "bynk".into(),
                    version: 1,
                    text: v1_text.to_string(),
                },
            })
            .await;
        backend.run_round().await;

        // The round exists and is version 1.
        let a1 = backend.test_analysis().await.expect("round 1");
        let rel = Backend::uri_to_rel(&a1, &uri).expect("uri resolves");
        assert_eq!(a1.versions.get(&rel), Some(&1), "round 1 is version 1");

        // Edit: the buffer becomes `v2_text` at version 2. The debounce this
        // schedules is superseded by the request-driven refresh below.
        backend
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: 2,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: v2_text.to_string(),
                }],
            })
            .await;
        (uri, rel)
    }

    /// The headline. After an edit that inserts a line above a symbol, a
    /// position request at the symbol's *new* location resolves to the symbol —
    /// because the gate refreshes to the edited buffer first. Under the old
    /// behaviour the new position was resolved against the round-1 snapshot,
    /// landing on the wrong text.
    #[tokio::test]
    async fn a_position_after_an_edit_resolves_against_the_new_text() {
        let v1 = "commons q.a\n\nfn target(x: Int) -> Int {\n  x\n}\n";
        // Prepend a blank line: `target` moves from line 2 to line 3.
        let v2 = format!("\n{v1}");
        let s = scratch_project(
            "fresh_hd",
            &[("bynk.toml", "[project]\nname=\"q\"\n"), ("src/a.bynk", v1)],
        );
        let backend = backend_at(&s.0).await;
        let (uri, rel) = open_round_edit(&backend, &s.0, v1, &v2).await;

        // The gate refreshes to the edited version and text.
        let a = backend.analysis_for(&uri).await.expect("current analysis");
        assert_eq!(a.versions.get(&rel), Some(&2), "gate refreshed to the edit");
        assert_eq!(a.snapshots.get(&rel).map(String::as_str), Some(v2.as_str()));

        // `target`'s new position resolves to `target` in the refreshed snapshot.
        let off_v2 = v2.find("target").unwrap();
        let new_pos = crate::position::offset_to_position(&v2, off_v2);
        let (a2, rel2, off) = backend
            .index_position(&uri, new_pos)
            .await
            .expect("position resolves");
        assert!(
            a2.snapshots.get(&rel2).unwrap()[off..].starts_with("target"),
            "the new position must land on `target` in the current snapshot — \
             the whole point of refreshing",
        );
    }

    /// The gate never returns a round whose snapshot for the file predates the
    /// buffer. A cached round at version 1 with the buffer at version 2 must be
    /// refreshed, not served.
    #[tokio::test]
    async fn a_stale_round_is_never_served() {
        let v1 = "commons q.a\n\nfn f(x: Int) -> Int {\n  x\n}\n";
        let v2 = format!("{v1}\nfn g(y: Int) -> Int {{\n  y\n}}\n");
        let s = scratch_project(
            "fresh_stale",
            &[("bynk.toml", "[project]\nname=\"q\"\n"), ("src/a.bynk", v1)],
        );
        let backend = backend_at(&s.0).await;
        let (uri, rel) = open_round_edit(&backend, &s.0, v1, &v2).await;

        // Precondition: the *cached* round is still version 1 (no refresh yet).
        let cached = backend.test_analysis().await.unwrap();
        assert_eq!(cached.versions.get(&rel), Some(&1), "cached round is stale");

        // The gate must not hand back that stale round.
        let a = backend.analysis_for(&uri).await.unwrap();
        assert_eq!(
            a.versions.get(&rel),
            Some(&2),
            "analysis_for must refresh past a stale cached round, never serve it",
        );
    }

    /// #733: `committed_analysis` is the non-refreshing counterpart of
    /// `analysis_for`. Where the strict gate refreshes past a stale round (the
    /// test above), this one **serves the committed round as-is** — even with the
    /// buffer a version ahead — and never triggers a refresh. That is what lets a
    /// decoration request answer from the committed round while the user types,
    /// instead of forcing a whole-project round on every keystroke.
    #[tokio::test]
    async fn committed_analysis_serves_the_stale_round_without_refreshing() {
        let v1 = "commons q.a\n\nfn f(x: Int) -> Int {\n  x\n}\n";
        let v2 = format!("{v1}\nfn g(y: Int) -> Int {{\n  y\n}}\n");
        let s = scratch_project(
            "fresh_committed",
            &[("bynk.toml", "[project]\nname=\"q\"\n"), ("src/a.bynk", v1)],
        );
        let backend = backend_at(&s.0).await;
        let (uri, rel) = open_round_edit(&backend, &s.0, v1, &v2).await;

        // Precondition: the cached round is still version 1 (buffer is at 2).
        assert_eq!(
            backend.test_analysis().await.unwrap().versions.get(&rel),
            Some(&1),
            "cached round is stale",
        );

        // The non-refreshing gate hands back that stale round unchanged...
        let a = backend
            .committed_analysis(&uri)
            .await
            .expect("committed round");
        assert_eq!(
            a.versions.get(&rel),
            Some(&1),
            "committed_analysis serves the committed round, stale and all",
        );
        // ...and left the cached round untouched (no refresh was triggered).
        assert_eq!(
            backend.test_analysis().await.unwrap().versions.get(&rel),
            Some(&1),
            "committed_analysis must not trigger a refresh",
        );
    }

    /// DECISION B: concurrent requests after one edit coalesce onto a single
    /// round, not one each. The refresh lock serialises them; the second finds
    /// the first's round already current.
    #[tokio::test]
    async fn concurrent_requests_after_one_edit_share_one_round() {
        let v1 = "commons q.a\n\nfn f(x: Int) -> Int {\n  x\n}\n";
        let v2 = format!("\n{v1}");
        let s = scratch_project(
            "fresh_coal",
            &[("bynk.toml", "[project]\nname=\"q\"\n"), ("src/a.bynk", v1)],
        );
        let backend = backend_at(&s.0).await;
        let (uri, _rel) = open_round_edit(&backend, &s.0, v1, &v2).await;

        let started_before = backend.test_round_started().await;

        // Fire several gate calls concurrently.
        let calls = (0..5).map(|_| {
            let b = backend.clone();
            let u = uri.clone();
            tokio::spawn(async move { b.analysis_for(&u).await.is_some() })
        });
        for c in calls {
            assert!(
                c.await.unwrap(),
                "each concurrent request must get an analysis"
            );
        }

        let started_after = backend.test_round_started().await;
        assert_eq!(
            started_after - started_before,
            1,
            "five concurrent requests after one edit must share ONE round, not run five",
        );
    }

    /// DECISION D: a file outside every `include` root cannot be answered at the
    /// client's version — the gate declines rather than serving something.
    #[tokio::test]
    async fn a_file_outside_the_project_is_declined() {
        let v1 = "commons q.a\n\nfn f(x: Int) -> Int {\n  x\n}\n";
        let s = scratch_project(
            "fresh_out",
            &[("bynk.toml", "[project]\nname=\"q\"\n"), ("src/a.bynk", v1)],
        );
        let backend = backend_at(&s.0).await;
        // Round the project so a cached analysis exists.
        backend.run_round().await;

        // A URI for a file the project does not contain, opened as a buffer.
        let outside = s.0.join("elsewhere.bynk");
        std::fs::write(&outside, v1).unwrap();
        let uri = Url::from_file_path(outside.canonicalize().unwrap()).unwrap();
        backend
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "bynk".into(),
                    version: 1,
                    text: v1.to_string(),
                },
            })
            .await;

        assert!(
            backend.analysis_for(&uri).await.is_none(),
            "a file outside the include roots is never a snapshot key — decline, \
             don't serve a round that doesn't cover it",
        );
    }

    /// Review of #666: rename emits versioned edits across every file that
    /// references the symbol, so it must refresh **all** open buffers, not just
    /// the cursor's. Edit a non-cursor file that references the symbol, then
    /// rename from the (unedited) definition file: the edit for the dirty file
    /// must carry its *current* version, or the client rejects the whole rename.
    /// Under the per-URI gate the cursor's file was current, so no refresh ran
    /// and the dirty file kept its stale version.
    #[tokio::test]
    async fn a_multi_file_rename_stamps_a_dirty_non_cursor_file_at_its_current_version() {
        let util = "commons demo.util\n\ntype Money = Int where Positive\n";
        let thing = "commons demo.thing\n\nuses demo.util\n\nfn f(m: Money) -> Money {\n  m\n}\n";
        let s = scratch_project(
            "rename_multi",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                ("src/demo/util.bynk", util),
                ("src/demo/thing.bynk", thing),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = |rel: &str| {
            let abs = s.0.join(rel);
            Url::from_file_path(abs.canonicalize().unwrap_or(abs)).unwrap()
        };
        let util_uri = uri("src/demo/util.bynk");
        let thing_uri = uri("src/demo/thing.bynk");

        for (u, text) in [(&util_uri, util), (&thing_uri, thing)] {
            backend
                .did_open(DidOpenTextDocumentParams {
                    text_document: TextDocumentItem {
                        uri: u.clone(),
                        language_id: "bynk".into(),
                        version: 1,
                        text: text.to_string(),
                    },
                })
                .await;
        }
        backend.run_round().await;

        // Edit the NON-cursor file (`thing`) to version 2 — a blank line above,
        // so `Money`'s references shift but still resolve.
        let thing_v2 = format!("\n{thing}");
        backend
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: thing_uri.clone(),
                    version: 2,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: thing_v2.clone(),
                }],
            })
            .await;

        // Rename `Money` from its definition in `util` (untouched, still v1).
        let money_off = util.find("Money").unwrap();
        let pos = crate::position::offset_to_position(util, money_off);
        let edit = backend
            .rename(RenameParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier {
                        uri: util_uri.clone(),
                    },
                    position: pos,
                },
                new_name: "Amount".into(),
                work_done_progress_params: Default::default(),
            })
            .await
            .expect("rename must not error")
            .expect("rename must produce edits");

        let DocumentChanges::Edits(edits) = edit.document_changes.unwrap() else {
            panic!("expected document-change edits");
        };
        let thing_edit = edits
            .iter()
            .find(|e| e.text_document.uri == thing_uri)
            .expect("the rename must edit `thing`, which references the symbol");
        assert_eq!(
            thing_edit.text_document.version,
            Some(2),
            "the dirty non-cursor file's edit must carry its current version (2), \
             not the stale round's (1) — else the client rejects the whole rename",
        );
    }

    /// #302: renaming a unit's file rewrites its own declaration header
    /// **and** every other file's `uses`/`consumes` reference — over a split
    /// `src`/`tests` project, exercising the project-relative (`src/`-prefixed)
    /// path the `src`/`tests` split leaves on every identity path.
    #[tokio::test]
    async fn will_rename_files_updates_the_declaration_and_every_reference() {
        let charge = "commons billing.charge\n\ntype ChargeId = Int where Positive\n";
        let main = "context app.main\n\nuses billing.charge\n";
        let s = scratch_project(
            "will_rename_basic",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                ("src/billing/charge.bynk", charge),
                ("src/app/main.bynk", main),
            ],
        );
        let backend = backend_at(&s.0).await;
        let root_canon = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        let uri = |rel: &str| Url::from_file_path(root_canon.join(rel)).unwrap();
        let old_uri = uri("src/billing/charge.bynk");
        let new_uri = uri("src/billing/pay.bynk");
        let main_uri = uri("src/app/main.bynk");

        backend.run_round().await;

        let edit = backend
            .will_rename_files(RenameFilesParams {
                files: vec![FileRename {
                    old_uri: old_uri.to_string(),
                    new_uri: new_uri.to_string(),
                }],
            })
            .await
            .expect("will_rename_files must not error")
            .expect("must produce edits");

        let DocumentChanges::Edits(edits) = edit.document_changes.unwrap() else {
            panic!("expected document-change edits");
        };

        let own = edits
            .iter()
            .find(|e| e.text_document.uri == old_uri)
            .expect("the moved file's own declaration must be rewritten");
        assert_eq!(own.edits.len(), 1);
        let OneOf::Left(own_edit) = &own.edits[0] else {
            panic!("expected a plain TextEdit");
        };
        assert_eq!(own_edit.new_text, "billing.pay");

        let referencer = edits
            .iter()
            .find(|e| e.text_document.uri == main_uri)
            .expect("the referencing file must be rewritten");
        assert_eq!(referencer.edits.len(), 1);
        let OneOf::Left(ref_edit) = &referencer.edits[0] else {
            panic!("expected a plain TextEdit");
        };
        assert_eq!(ref_edit.new_text, "billing.pay");
    }

    /// #302: renaming one member file within a multi-file unit's directory
    /// doesn't change the unit's qualified name (it's the directory, not the
    /// filename) — no edits are needed.
    #[tokio::test]
    async fn will_rename_files_is_a_noop_for_a_multi_file_unit_member() {
        let s = scratch_project(
            "will_rename_multi_file_noop",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                (
                    "src/billing/charge/one.bynk",
                    "context billing.charge\n\ntype ChargeId = Int where Positive\n",
                ),
                (
                    "src/billing/charge/two.bynk",
                    "context billing.charge\n\ntype PaymentId = Int where Positive\n",
                ),
            ],
        );
        let backend = backend_at(&s.0).await;
        let root_canon = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        let uri = |rel: &str| Url::from_file_path(root_canon.join(rel)).unwrap();

        backend.run_round().await;

        let edit = backend
            .will_rename_files(RenameFilesParams {
                files: vec![FileRename {
                    old_uri: uri("src/billing/charge/one.bynk").to_string(),
                    new_uri: uri("src/billing/charge/renamed.bynk").to_string(),
                }],
            })
            .await
            .expect("will_rename_files must not error");
        assert!(
            edit.is_none(),
            "renaming a member file within the same directory must not edit anything"
        );
    }

    /// #302: a `suite` file has no addressable name of its own
    /// (`SourceUnit::name()` is its *target*'s name) — renaming it produces
    /// no edits.
    #[tokio::test]
    async fn will_rename_files_is_a_noop_for_a_suite() {
        let s = scratch_project(
            "will_rename_suite_noop",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                (
                    "src/billing/charge.bynk",
                    "commons billing.charge\n\ntype ChargeId = Int where Positive\n",
                ),
                ("tests/billing_charge.bynk", "suite billing.charge\n"),
            ],
        );
        let backend = backend_at(&s.0).await;
        let root_canon = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        let uri = |rel: &str| Url::from_file_path(root_canon.join(rel)).unwrap();

        backend.run_round().await;

        let edit = backend
            .will_rename_files(RenameFilesParams {
                files: vec![FileRename {
                    old_uri: uri("tests/billing_charge.bynk").to_string(),
                    new_uri: uri("tests/billing_charge_renamed.bynk").to_string(),
                }],
            })
            .await
            .expect("will_rename_files must not error");
        assert!(edit.is_none(), "a suite rename must produce no edits");
    }

    /// #302 review: a `suite`'s own `target` is a *reference* too
    /// (`unit_reference_spans`' suite branch) — renaming the unit a suite
    /// tests must rewrite the suite's `suite <target>` header, not just
    /// `uses`/`consumes` clauses in ordinary units.
    #[tokio::test]
    async fn will_rename_files_updates_a_suite_s_target_reference() {
        let s = scratch_project(
            "will_rename_suite_target",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                (
                    "src/billing/charge.bynk",
                    "commons billing.charge\n\ntype ChargeId = Int where Positive\n",
                ),
                ("tests/billing_charge.bynk", "suite billing.charge\n"),
            ],
        );
        let backend = backend_at(&s.0).await;
        let root_canon = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        let uri = |rel: &str| Url::from_file_path(root_canon.join(rel)).unwrap();
        let suite_uri = uri("tests/billing_charge.bynk");

        backend.run_round().await;

        let edit = backend
            .will_rename_files(RenameFilesParams {
                files: vec![FileRename {
                    old_uri: uri("src/billing/charge.bynk").to_string(),
                    new_uri: uri("src/billing/pay.bynk").to_string(),
                }],
            })
            .await
            .expect("will_rename_files must not error")
            .expect("must produce edits");

        let DocumentChanges::Edits(edits) = edit.document_changes.unwrap() else {
            panic!("expected document-change edits");
        };
        let suite_edit = edits
            .iter()
            .find(|e| e.text_document.uri == suite_uri)
            .expect("the suite's own `suite <target>` header must be rewritten");
        assert_eq!(suite_edit.edits.len(), 1);
        let OneOf::Left(e) = &suite_edit.edits[0] else {
            panic!("expected a plain TextEdit");
        };
        assert_eq!(e.new_text, "billing.pay");
    }

    /// #302 review: renaming into a path that implies a name some other file
    /// already declares must not hand back an edit that would create a
    /// duplicate-name project — a lightweight `unit_sources` check, not
    /// `rename`'s full re-analysis.
    #[tokio::test]
    async fn will_rename_files_refuses_a_rename_that_collides_with_an_existing_unit() {
        let s = scratch_project(
            "will_rename_collision",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                (
                    "src/billing/charge.bynk",
                    "commons billing.charge\n\ntype ChargeId = Int where Positive\n",
                ),
                (
                    "src/billing/pay.bynk",
                    "commons billing.pay\n\ntype PaymentId = Int where Positive\n",
                ),
            ],
        );
        let backend = backend_at(&s.0).await;
        let root_canon = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        let uri = |rel: &str| Url::from_file_path(root_canon.join(rel)).unwrap();

        backend.run_round().await;

        // Renaming `charge.bynk` to `pay.bynk` would imply `billing.pay` —
        // already declared by the sibling file.
        let edit = backend
            .will_rename_files(RenameFilesParams {
                files: vec![FileRename {
                    old_uri: uri("src/billing/charge.bynk").to_string(),
                    new_uri: uri("src/billing/pay.bynk").to_string(),
                }],
            })
            .await
            .expect("will_rename_files must not error");
        assert!(
            edit.is_none(),
            "a rename that collides with an existing unit name must produce no edits"
        );
    }

    /// #302 review: `willRenameFiles`' `new_uri` names a file that doesn't
    /// exist yet, so `uri_to_rel`'s `canonicalize` fails and previously fell
    /// back to the client's raw, non-canonical path — which mismatches
    /// `project_root` (always canonical) whenever the workspace root sits
    /// behind a symlink, and the handler silently produced no edit.
    /// `uri_to_rel_for_new_path` canonicalizes the parent directory (which
    /// does exist) instead, so this must still produce edits.
    #[cfg(unix)]
    #[tokio::test]
    async fn will_rename_files_tolerates_a_symlinked_project_root() {
        let real = scratch_project(
            "will_rename_symlink_real",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                (
                    "src/billing/charge.bynk",
                    "commons billing.charge\n\ntype ChargeId = Int where Positive\n",
                ),
            ],
        );
        let alias = std::env::temp_dir().join(format!(
            "bynk_lsp_sliceA_will_rename_symlink_alias_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_file(&alias);
        std::os::unix::fs::symlink(&real.0, &alias).expect("symlink the scratch root");

        let backend = backend_at(&alias).await;
        // Built through the symlink, deliberately uncanonicalized — the path
        // shape a client actually sends (it opened the workspace at `alias`,
        // not at whatever `alias` resolves to).
        let uri = |rel: &str| Url::from_file_path(alias.join(rel)).unwrap();
        let old_uri = uri("src/billing/charge.bynk");
        let new_uri = uri("src/billing/pay.bynk"); // does not exist on disk

        backend.run_round().await;

        let edit = backend
            .will_rename_files(RenameFilesParams {
                files: vec![FileRename {
                    old_uri: old_uri.to_string(),
                    new_uri: new_uri.to_string(),
                }],
            })
            .await
            .expect("will_rename_files must not error")
            .expect("must produce edits despite the symlinked root");

        let DocumentChanges::Edits(edits) = edit.document_changes.unwrap() else {
            panic!("expected document-change edits");
        };
        assert_eq!(
            edits.len(),
            1,
            "only the moved file's own header changes here"
        );
        let OneOf::Left(e) = &edits[0].edits[0] else {
            panic!("expected a plain TextEdit");
        };
        assert_eq!(e.new_text, "billing.pay");

        let _ = std::fs::remove_file(&alias);
    }

    /// #2105: run `willRenameFiles` over `renames` (project-relative old →
    /// new paths) against a fresh round of `files`, and flatten the result to
    /// `project-relative file → sorted new texts`. `None` when no edit comes
    /// back. Also asserts that no file's edits overlap, the failure mode of a
    /// multi-file unit or a file-plus-folder request emitting an edit twice.
    async fn folder_rename(
        tag: &str,
        files: &[(&str, &str)],
        renames: &[(&str, &str)],
    ) -> Option<std::collections::BTreeMap<String, Vec<String>>> {
        let s = scratch_project(tag, files);
        let backend = backend_at(&s.0).await;
        let root_canon = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        // Joined component by component, so a `/`-separated fixture path is a
        // real path on Windows too (a `\\?\` canonical root does not split on
        // `/`).
        let uri = |rel: &str| {
            Url::from_file_path(root_canon.join(rel.split('/').collect::<PathBuf>())).unwrap()
        };
        // An edited document's URI maps back to its fixture path by comparing
        // file paths after a `Url` round trip on both sides, which normalises
        // the platform's spelling (drive letter, verbatim prefix, separators).
        let fixture_of = |edited: &Url| -> String {
            let edited = edited.to_file_path().unwrap();
            files
                .iter()
                .map(|(rel, _)| *rel)
                .find(|rel| uri(rel).to_file_path().ok().as_ref() == Some(&edited))
                .unwrap_or_else(|| panic!("edit to a non-fixture file {}", edited.display()))
                .to_string()
        };
        backend.run_round().await;
        let edit = backend
            .will_rename_files(RenameFilesParams {
                files: renames
                    .iter()
                    .map(|(old, new)| FileRename {
                        old_uri: uri(old).to_string(),
                        new_uri: uri(new).to_string(),
                    })
                    .collect(),
            })
            .await
            .expect("will_rename_files must not error")?;
        let DocumentChanges::Edits(edits) = edit.document_changes.unwrap() else {
            panic!("expected document-change edits");
        };
        let mut out = std::collections::BTreeMap::new();
        for doc in edits {
            let rel = fixture_of(&doc.text_document.uri);
            let mut ranges: Vec<Range> = Vec::new();
            let mut texts = Vec::new();
            for e in doc.edits {
                let OneOf::Left(e) = e else {
                    panic!("expected a plain TextEdit");
                };
                assert!(
                    !ranges
                        .iter()
                        .any(|r| r.start <= e.range.end && e.range.start <= r.end),
                    "overlapping edits in {rel}: {:?} and {ranges:?}",
                    e.range
                );
                ranges.push(e.range);
                texts.push(e.new_text);
            }
            texts.sort();
            assert!(
                out.insert(rel.clone(), texts).is_none(),
                "{rel} edited twice"
            );
        }
        Some(out)
    }

    fn expect_edits(pairs: &[(&str, &[&str])]) -> std::collections::BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(f, texts)| (f.to_string(), texts.iter().map(|t| t.to_string()).collect()))
            .collect()
    }

    /// The project the folder-rename tests share: two units in `billing/`, one
    /// referencing the other, and a context outside referencing both.
    const BILLING: &[(&str, &str)] = &[
        ("bynk.toml", "[project]\nname=\"demo\"\n"),
        (
            "src/billing/charge.bynk",
            "commons billing.charge\n\ntype ChargeId = Int where Positive\n",
        ),
        (
            "src/billing/refund.bynk",
            "commons billing.refund\n\nuses billing.charge\n\ntype RefundId = Int where Positive\n",
        ),
        (
            "src/app/main.bynk",
            "context app.main\n\nuses billing.charge\nuses billing.refund\n",
        ),
    ];

    /// #2105: renaming a folder renames every unit in it and rewrites every
    /// reference to them, including one between two units inside the folder.
    #[tokio::test]
    async fn will_rename_files_renames_a_folder_of_units() {
        let edits = folder_rename(
            "will_rename_folder",
            BILLING,
            &[("src/billing", "src/payments")],
        )
        .await
        .expect("a folder rename must produce edits");
        assert_eq!(
            edits,
            expect_edits(&[
                ("src/app/main.bynk", &["payments.charge", "payments.refund"]),
                ("src/billing/charge.bynk", &["payments.charge"]),
                (
                    "src/billing/refund.bynk",
                    &["payments.charge", "payments.refund"]
                ),
            ])
        );
    }

    /// #2105: moving a folder under a different parent prefixes every unit
    /// in it, nested folders included.
    #[tokio::test]
    async fn will_rename_files_moves_a_folder_with_nested_units() {
        let mut files = BILLING.to_vec();
        files.push((
            "src/billing/core/ledger.bynk",
            "commons billing.core.ledger\n\ntype Entry = Int where Positive\n",
        ));
        files.push((
            "src/app/books.bynk",
            "context app.books\n\nuses billing.core.ledger\n",
        ));
        files.push((
            "src/finance/tax.bynk",
            "commons finance.tax\n\ntype Rate = Int where Positive\n",
        ));
        let edits = folder_rename(
            "will_rename_folder_move",
            &files,
            &[("src/billing", "src/finance/billing")],
        )
        .await
        .expect("a folder move must produce edits");
        assert_eq!(
            edits,
            expect_edits(&[
                ("src/app/books.bynk", &["finance.billing.core.ledger"]),
                (
                    "src/app/main.bynk",
                    &["finance.billing.charge", "finance.billing.refund"]
                ),
                ("src/billing/charge.bynk", &["finance.billing.charge"]),
                (
                    "src/billing/core/ledger.bynk",
                    &["finance.billing.core.ledger"]
                ),
                (
                    "src/billing/refund.bynk",
                    &["finance.billing.charge", "finance.billing.refund"]
                ),
            ])
        );
    }

    /// #2105: renaming a nested folder renames only the units under it; a
    /// sibling folder sharing its name as a prefix (`core` vs `corex`) is not
    /// "under" it.
    #[tokio::test]
    async fn will_rename_files_renames_only_the_nested_folder() {
        let edits = folder_rename(
            "will_rename_folder_nested",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                (
                    "src/billing/core/ledger.bynk",
                    "commons billing.core.ledger\n",
                ),
                (
                    "src/billing/corex/extra.bynk",
                    "commons billing.corex.extra\n",
                ),
                (
                    "src/app/main.bynk",
                    "context app.main\n\nuses billing.core.ledger\nuses billing.corex.extra\n",
                ),
            ],
            &[("src/billing/core", "src/billing/base")],
        )
        .await
        .expect("a nested folder rename must produce edits");
        assert_eq!(
            edits,
            expect_edits(&[
                ("src/app/main.bynk", &["billing.base.ledger"]),
                ("src/billing/core/ledger.bynk", &["billing.base.ledger"]),
            ])
        );
    }

    /// #2105: a folder that *is* a multi-file unit renames the unit: every
    /// member's header changes, and each reference is rewritten exactly once
    /// (`folder_rename` fails on an overlapping or duplicated edit).
    #[tokio::test]
    async fn will_rename_files_renames_a_multi_file_unit_folder_once() {
        let edits = folder_rename(
            "will_rename_folder_multi_file",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                (
                    "src/billing/charge/one.bynk",
                    "context billing.charge\n\ntype ChargeId = Int where Positive\n",
                ),
                (
                    "src/billing/charge/two.bynk",
                    "context billing.charge\n\ntype PaymentId = Int where Positive\n",
                ),
                (
                    "src/app/main.bynk",
                    "context app.main\n\nconsumes billing.charge\n",
                ),
            ],
            &[("src/billing/charge", "src/billing/payment")],
        )
        .await
        .expect("a multi-file unit folder rename must produce edits");
        assert_eq!(
            edits,
            expect_edits(&[
                ("src/app/main.bynk", &["billing.payment"]),
                ("src/billing/charge/one.bynk", &["billing.payment"]),
                ("src/billing/charge/two.bynk", &["billing.payment"]),
            ])
        );
    }

    /// #2105: a folder holding no `.bynk` file produces no edit.
    #[tokio::test]
    async fn will_rename_files_is_a_noop_for_a_folder_without_units() {
        let mut files = BILLING.to_vec();
        files.push(("src/assets/logo.txt", "not bynk\n"));
        let edits = folder_rename(
            "will_rename_folder_empty",
            &files,
            &[("src/assets", "src/static")],
        )
        .await;
        assert!(edits.is_none(), "{edits:?}");
    }

    /// #2105: a request naming a file and also its enclosing folder (both
    /// moving the same way) edits each file once, like the folder alone.
    #[tokio::test]
    async fn will_rename_files_deduplicates_a_file_inside_a_renamed_folder() {
        let edits = folder_rename(
            "will_rename_folder_mixed",
            BILLING,
            &[
                ("src/billing/charge.bynk", "src/payments/charge.bynk"),
                ("src/billing", "src/payments"),
            ],
        )
        .await
        .expect("a mixed rename must produce edits");
        assert_eq!(
            edits,
            expect_edits(&[
                ("src/app/main.bynk", &["payments.charge", "payments.refund"]),
                ("src/billing/charge.bynk", &["payments.charge"]),
                (
                    "src/billing/refund.bynk",
                    &["payments.charge", "payments.refund"]
                ),
            ])
        );
    }

    /// #2105 (DECISION B): moving a folder into another project (here a
    /// nested `bynk.toml`, which the old `strip_prefix` check let through)
    /// produces no edit.
    #[tokio::test]
    async fn will_rename_files_refuses_a_move_into_another_project() {
        let mut files = BILLING.to_vec();
        files.push(("vendor/bynk.toml", "[project]\nname=\"vendor\"\n"));
        files.push(("vendor/src/keep.bynk", "commons keep\n"));
        let folder = folder_rename(
            "will_rename_folder_cross_project",
            &files,
            &[("src/billing", "vendor/src/billing")],
        )
        .await;
        assert!(folder.is_none(), "{folder:?}");
        let file = folder_rename(
            "will_rename_file_cross_project",
            &files,
            &[("src/billing/charge.bynk", "vendor/src/charge.bynk")],
        )
        .await;
        assert!(file.is_none(), "{file:?}");
    }

    /// #485: a rootless multi-file-commons file (a `src/` tree with no
    /// `bynk.toml`, the layout the compiler fixtures use) resolves its
    /// implicit source root — the nearest ancestor `src/` — so project-mode
    /// analysis kicks in instead of sibling-blind single-file `diagnose`.
    #[test]
    fn find_source_root_walks_up_to_the_nearest_src() {
        let ws = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let make = ws.join(
            "bynkc/tests/fixtures/positive/\
             252_multi_file_commons_dotted_test/src/shipping/rates/make.bynk",
        );
        assert!(make.is_file(), "fixture present: {}", make.display());

        let src = Backend::find_source_root(&make).expect("an ancestor src/");
        assert!(
            src.ends_with("252_multi_file_commons_dotted_test/src"),
            "nearest ancestor src, got {}",
            src.display()
        );

        // No `bynk.toml` on the path, so resolution falls back to the implicit
        // src tree, and the project root is `src`'s parent.
        //
        // Slice A: the old invariant here was `root.join(config.src_dir) == src`
        // — the analysis root re-derived by reducing the manifest to one
        // directory. That reduction is gone: the round is rooted at the project
        // and `bynk_ide::AnalysisRoots::Project` resolves the trees from the
        // manifest (here, absent → `ProjectPaths::conventional`, which picks up
        // exactly this `src/`). So what must hold is that the root is `src`'s
        // parent, and that conventional discovery finds this file from it.
        let (root, _config) = Backend::resolve_root(&make).expect("implicit project");
        assert_eq!(root, src.parent().expect("src has a parent"));

        // No `bynk.toml` in this fixture (the test is exactly about the
        // absent-manifest → conventional-layout path), so an empty overlay
        // is correct here, not a stand-in for a real manifest read.
        let found = bynk_ide::discover_files(
            &bynk_ide::AnalysisRoots::Project(root.clone()),
            &std::collections::HashMap::new(),
        );
        let make_canon = make.canonicalize().unwrap_or(make.clone());
        assert!(
            found
                .iter()
                .any(|p| p.canonicalize().unwrap_or_else(|_| p.clone()) == make_canon),
            "the compiler's own discovery must reach {} from the project root {}; got {found:?}",
            make.display(),
            root.display(),
        );
    }

    /// A file with no `bynk.toml` and no ancestor `src/` stays in single-file
    /// mode — resolution returns `None`, so the caller keeps the per-buffer
    /// `diagnose` path.
    #[test]
    fn resolve_root_is_none_without_toml_or_src() {
        // The crate manifest sits under `bynk-lsp/`, not inside any `src/`.
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert!(p.is_file());
        assert!(Backend::find_source_root(&p).is_none());
        assert!(Backend::resolve_root(&p).is_none());
    }

    // v0.124 (slice 3): the `<expr> is <cursor>` scrutinee-offset detection that
    // feeds `is`-pattern completion.
    #[test]
    fn is_scrutinee_offset_locates_the_scrutinee() {
        let text = "  order.status is Pen";
        let off = is_scrutinee_offset(text, text.len()).expect("at an is-position");
        // Lands on the last char of `order.status` (the `s` of `status`).
        assert_eq!(&text[off..off + 1], "s");
        assert!(off < text.find(" is ").unwrap());
        // No trailing partial, cursor right after `is `.
        let text2 = "  x is ";
        let off2 = is_scrutinee_offset(text2, text2.len()).expect("at an is-position");
        assert_eq!(&text2[off2..off2 + 1], "x");
        // `basis` is not a standalone `is`.
        assert!(is_scrutinee_offset("  basis ", "  basis ".len()).is_none());
        // Not an is-position at all.
        assert!(is_scrutinee_offset("  let x = ", "  let x = ".len()).is_none());
    }

    // v0.128: the `match <expr> { <arm-start>` scrutinee-offset detection that
    // feeds match-arm variant completion.
    #[test]
    fn let_else_match_head_reads_the_value_and_the_covered_variant() {
        // #2010: the first arm, right after the brace.
        let t = "  let Ok(n) = load(r) else match {\n    ";
        let h = let_else_match_head(t, t.len()).expect("at an arm-start");
        assert_eq!(&t[h.value_end..h.value_end + 1], ")"); // last char of `load(r)`
        assert!(!h.effectful);
        assert_eq!(h.covered.as_deref(), Some("Ok"));

        // `<-`, a later arm, a partial pattern typed.
        let t = "let Some(x) <- find(k) else match {\n  None => 0,\n  Er";
        let h = let_else_match_head(t, t.len()).expect("at a later arm-start");
        assert!(h.effectful);
        assert_eq!(&t[h.value_end..h.value_end + 1], ")");
        assert_eq!(h.covered.as_deref(), Some("Some"));

        // A pattern that covers only part of a variant leaves it offerable.
        let t = "let Err(Missing) = r else match {\n  ";
        assert_eq!(let_else_match_head(t, t.len()).unwrap().covered, None);

        // A qualified nullary variant is covered whole.
        let t = "let Status.Done = s else match {\n  ";
        assert_eq!(
            let_else_match_head(t, t.len()).unwrap().covered.as_deref(),
            Some("Done")
        );

        // An `==` in the value is not the binding `=`.
        let t = "let Ok(b) = check(a == 1) else match {\n  ";
        let h = let_else_match_head(t, t.len()).unwrap();
        assert_eq!(&t[h.value_end..h.value_end + 1], ")");

        // Not a let-else: a plain `match`, or an arm body.
        assert!(let_else_match_head("match c {\n  ", 11).is_none());
        let t = "let Ok(n) = r else match {\n  Err(e) => ";
        assert!(let_else_match_head(t, t.len()).is_none());
    }

    #[test]
    fn match_scrutinee_offset_locates_the_scrutinee() {
        // First arm, cursor right after the opening brace.
        let t = "match order.status {\n  ";
        let off = match_scrutinee_offset(t, t.len()).expect("at an arm-start");
        assert_eq!(&t[off..off + 1], "s"); // last char of `order.status`
        assert!(off < t.find(" {").unwrap());

        // First arm with a partial pattern typed.
        let t = "match color { Re";
        let off = match_scrutinee_offset(t, t.len()).expect("at an arm-start");
        assert_eq!(&t[off..off + 1], "r"); // last char of `color`

        // A later arm after a top-level comma, mid-partial.
        let t = "match c {\n  Red => 1,\n  Gr";
        let off = match_scrutinee_offset(t, t.len()).expect("at a later arm-start");
        assert_eq!(&t[off..off + 1], "c");

        // A top-level comma inside a preceding arm body does not confuse the
        // header (the nested call's comma is at depth > 0).
        let t = "match c {\n  Red => f(a, b),\n  ";
        assert!(match_scrutinee_offset(t, t.len()).is_some());

        // Inside an arm *body* (after `=>`) — not a pattern position.
        assert!(
            match_scrutinee_offset("match c {\n  Red => ", "match c {\n  Red => ".len()).is_none()
        );

        // A non-`match` block offers nothing.
        assert!(match_scrutinee_offset("fn f() {\n  ", "fn f() {\n  ".len()).is_none());

        // A nested constructor position (`Ok(<cursor>`) is not an arm-start.
        assert!(match_scrutinee_offset("match c {\n  Ok(", "match c {\n  Ok(".len()).is_none());

        // No open brace / no scrutinee → nothing.
        assert!(match_scrutinee_offset("match c ", "match c ".len()).is_none());
        assert!(match_scrutinee_offset("match {\n  ", "match {\n  ".len()).is_none());
    }

    // v0.145 (ADR 0169): the `match <expr> { … Variant(<partial>` nested-pattern
    // detection that feeds payload-variant completion — the position
    // `match_scrutinee_offset` deliberately bails on.
    #[test]
    fn nested_pattern_offset_locates_the_scrutinee_and_variant() {
        // Cursor right inside a variant's payload parens.
        let t = "match res {\n  Some(";
        let (off, variant) = nested_pattern_offset(t, t.len()).expect("inside a nested pattern");
        assert_eq!(&t[off..off + 1], "s"); // last char of `res`
        assert_eq!(variant, "Some");

        // With a partial nested pattern typed, and a qualifier.
        let t = "match res {\n  Ok(Po";
        let (off, variant) = nested_pattern_offset(t, t.len()).expect("mid partial");
        assert_eq!(&t[off..off + 1], "s");
        assert_eq!(variant, "Ok");

        // A later arm after a top-level comma.
        let t = "match r {\n  Ok(n) => n,\n  Err(";
        let (off, variant) = nested_pattern_offset(t, t.len()).expect("later arm");
        assert_eq!(&t[off..off + 1], "r");
        assert_eq!(variant, "Err");

        // A lowercase-led token before `(` is a binding/call, not a variant
        // constructor — no nested completion (there is no inner type to open).
        assert!(nested_pattern_offset("match r {\n  ok(", "match r {\n  ok(".len()).is_none());

        // An arm-start (no open paren) is the flat position, not a nested one.
        assert!(nested_pattern_offset("match c {\n  ", "match c {\n  ".len()).is_none());
        assert!(nested_pattern_offset("match c {\n  Ok", "match c {\n  Ok".len()).is_none());

        // Inside an arm body (after `=>`) is not a pattern position.
        let t = "match c {\n  Ok(n) => g(";
        assert!(nested_pattern_offset(t, t.len()).is_none());

        // A non-`match` block offers nothing.
        assert!(nested_pattern_offset("fn f() {\n  h(", "fn f() {\n  h(".len()).is_none());
    }

    /// A watched-file change on `bynk.toml` is recognised (so the config can be
    /// reloaded live), while a sibling `.bynk` file or a merely `…bynk.toml`-
    /// suffixed name is not — the name-component match, not a path suffix.
    #[test]
    fn is_bynk_toml_matches_only_the_manifest() {
        // Build URIs from a host-absolute base so `from_file_path` succeeds on
        // Windows too (a Unix-style `/proj` path is not absolute there — no
        // drive letter — and would fail to convert). Mirrors the sibling
        // `find_source_root` test's `CARGO_MANIFEST_DIR` base.
        let base = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let toml = Url::from_file_path(base.join("bynk.toml")).expect("abs path");
        assert!(is_bynk_toml(&toml));
        let nested = Url::from_file_path(base.join("sub").join("bynk.toml")).expect("abs path");
        assert!(is_bynk_toml(&nested));

        // A source file is not the manifest.
        let src = Url::from_file_path(base.join("src").join("main.bynk")).expect("abs path");
        assert!(!is_bynk_toml(&src));
        // A file whose name merely *ends with* `bynk.toml` must not fire.
        let decoy = Url::from_file_path(base.join("notbynk.toml")).expect("abs path");
        assert!(!is_bynk_toml(&decoy));
        // A non-file URI never matches.
        let remote = Url::parse("https://example.com/bynk.toml").expect("url");
        assert!(!is_bynk_toml(&remote));
    }

    /// #2100 slice 5: the organise-imports source action is computed only
    /// when the client asks for its kind (directly, or via its `source`
    /// parent), never for an unfiltered lightbulb request or another kind —
    /// and when it is, its edit is the re-checked organise.
    #[tokio::test]
    async fn organise_imports_is_offered_only_when_asked_for() {
        let other = "commons shared.other\n\ntype Other = { n: Int }\n";
        let widget = "commons shared.widget\n\ntype Widget = { size: Int }\n";
        let app = "commons app.a\n\nuses shared.widget\nuses shared.other\n\nfn area(w: Widget) -> Int {\n  w.size\n}\n";
        let s = scratch_project(
            "organise_only",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                ("src/shared/other.bynk", other),
                ("src/shared/widget.bynk", widget),
                ("src/app/a.bynk", app),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = file_uri(&s.0, "src/app/a.bynk");
        open(&backend, &uri, app).await;
        backend.run_round().await;

        let request = |only: Option<Vec<CodeActionKind>>| CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range::default(),
            context: CodeActionContext {
                diagnostics: Vec::new(),
                only,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let organise_titles = |actions: Vec<CodeActionOrCommand>| -> Vec<String> {
            actions
                .into_iter()
                .filter_map(|a| match a {
                    CodeActionOrCommand::CodeAction(a)
                        if a.kind == Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS) =>
                    {
                        Some(a.title)
                    }
                    _ => None,
                })
                .collect()
        };

        for only in [None, Some(vec![CodeActionKind::QUICKFIX])] {
            let actions = backend
                .code_action(request(only.clone()))
                .await
                .unwrap()
                .unwrap();
            assert!(organise_titles(actions).is_empty(), "offered for {only:?}");
        }
        for only in [
            CodeActionKind::SOURCE,
            CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
        ] {
            let actions = backend
                .code_action(request(Some(vec![only.clone()])))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(actions.len(), 1, "one action for {only:?}");
            let CodeActionOrCommand::CodeAction(action) = &actions[0] else {
                panic!("expected a CodeAction");
            };
            assert_eq!(action.kind, Some(CodeActionKind::SOURCE_ORGANIZE_IMPORTS));
            let Some(DocumentChanges::Edits(doc_edits)) =
                &action.edit.as_ref().unwrap().document_changes
            else {
                panic!("expected versioned document edits");
            };
            assert_eq!(doc_edits[0].text_document.version, Some(1));
            let OneOf::Left(edit) = &doc_edits[0].edits[0] else {
                panic!("expected a plain TextEdit");
            };
            // `shared.other` is unused: the header keeps only `shared.widget`.
            assert_eq!(edit.new_text, "uses shared.widget");
        }
    }

    /// #2100 slice 6: fix-all is computed only when asked for (`source`,
    /// the generic `source.fixAll` VS Code sends on save, or its own
    /// `source.fixAll.bynk`), and carries every machine-applicable fix.
    #[tokio::test]
    async fn fix_all_is_offered_only_when_asked_for() {
        let src = "context app.c\n\ncapability Clock {\n  fn now() -> Effect[Int]\n}\n\nprovides Clock = Sys {\n  fn now() -> Effect[Int] {\n    1\n  }\n}\n\nservice one {\n  on call(x: Int) -> Effect[Int] given Clock {\n    x\n  }\n}\n\nservice two {\n  on call(x: Int) -> Effect[Int] given Clock {\n    x\n  }\n}\n";
        let s = scratch_project(
            "fix_all_only",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                ("src/app/c.bynk", src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = file_uri(&s.0, "src/app/c.bynk");
        open(&backend, &uri, src).await;
        backend.run_round().await;

        let request = |only: Option<Vec<CodeActionKind>>| CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range::default(),
            context: CodeActionContext {
                diagnostics: Vec::new(),
                only,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let fix_all = |actions: Vec<CodeActionOrCommand>| -> Vec<CodeAction> {
            actions
                .into_iter()
                .filter_map(|a| match a {
                    CodeActionOrCommand::CodeAction(a)
                        if a.kind == Some(crate::fix_all::kind()) =>
                    {
                        Some(a)
                    }
                    _ => None,
                })
                .collect()
        };
        for only in [None, Some(vec![CodeActionKind::QUICKFIX])] {
            let actions = backend
                .code_action(request(only.clone()))
                .await
                .unwrap()
                .unwrap();
            assert!(fix_all(actions).is_empty(), "offered for {only:?}");
        }
        for only in [
            CodeActionKind::SOURCE,
            CodeActionKind::SOURCE_FIX_ALL,
            crate::fix_all::kind(),
        ] {
            let actions = backend
                .code_action(request(Some(vec![only.clone()])))
                .await
                .unwrap()
                .unwrap();
            let found = fix_all(actions);
            assert_eq!(found.len(), 1, "one fix-all for {only:?}");
            let Some(DocumentChanges::Edits(doc_edits)) =
                &found[0].edit.as_ref().unwrap().document_changes
            else {
                panic!("expected versioned document edits");
            };
            // Both handlers' unused `given Clock` go in the one action.
            assert_eq!(doc_edits[0].edits.len(), 2);
        }
    }

    /// #2100 slice 3: fill-match-arms is listed on the unfiltered lightbulb
    /// request without an edit, and `codeAction/resolve` fills in the
    /// re-checked, versioned edit. A resolve for a stale version is refused.
    #[tokio::test]
    async fn fill_match_arms_lists_then_resolves() {
        let src = "commons app.m\n\ntype Shape =\n  | Circle(r: Int)\n  | Dot\n\nfn area(s: Shape) -> Int {\n  match s {\n    Dot => 1,\n  }\n}\n";
        let s = scratch_project(
            "fill_arms",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                ("src/app/m.bynk", src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = file_uri(&s.0, "src/app/m.bynk");
        open(&backend, &uri, src).await;
        backend.run_round().await;

        let at = crate::position::offset_to_position(src, src.find("Dot =>").unwrap());
        let actions = backend
            .code_action(CodeActionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                range: Range { start: at, end: at },
                context: CodeActionContext::default(),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            })
            .await
            .unwrap()
            .unwrap();
        let listed: Vec<CodeAction> = actions
            .into_iter()
            .filter_map(|a| match a {
                CodeActionOrCommand::CodeAction(a) if a.title.starts_with("Fill") => Some(a),
                _ => None,
            })
            .collect();
        assert_eq!(listed.len(), 1, "one fill action");
        assert!(listed[0].edit.is_none(), "listed without its edit");

        let resolved = backend
            .code_action_resolve(listed[0].clone())
            .await
            .unwrap();
        let Some(DocumentChanges::Edits(doc_edits)) =
            &resolved.edit.as_ref().unwrap().document_changes
        else {
            panic!("expected versioned document edits");
        };
        assert_eq!(doc_edits[0].text_document.version, Some(1));
        let inserted: String = doc_edits[0]
            .edits
            .iter()
            .map(|e| match e {
                OneOf::Left(e) => e.new_text.clone(),
                OneOf::Right(e) => e.text_edit.new_text.clone(),
            })
            .collect();
        assert_eq!(inserted, "\n    Circle(_) => 0,");

        let mut stale = listed[0].clone();
        stale.data.as_mut().unwrap()["version"] = serde_json::json!(0);
        assert!(backend.code_action_resolve(stale).await.is_err());

        // Any other action resolves to itself.
        let other = CodeAction {
            title: "x".into(),
            ..Default::default()
        };
        assert_eq!(
            backend.code_action_resolve(other.clone()).await.unwrap(),
            other
        );
    }

    /// #2100 slice 1: inline-variable is listed on the unfiltered lightbulb
    /// request at a `let`'s name without an edit, and `codeAction/resolve`
    /// fills in the re-checked, versioned edit.
    #[tokio::test]
    async fn inline_variable_lists_then_resolves() {
        let src = "commons app.m\n\nfn f(a: Int) -> Int {\n  let y = a + 1\n  y * 3\n}\n";
        let s = scratch_project(
            "inline_var",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                ("src/app/m.bynk", src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = file_uri(&s.0, "src/app/m.bynk");
        open(&backend, &uri, src).await;
        backend.run_round().await;

        let at = crate::position::offset_to_position(src, src.find("y =").unwrap());
        let actions = backend
            .code_action(CodeActionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                range: Range { start: at, end: at },
                context: CodeActionContext::default(),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            })
            .await
            .unwrap()
            .unwrap();
        let listed: Vec<CodeAction> = actions
            .into_iter()
            .filter_map(|a| match a {
                CodeActionOrCommand::CodeAction(a)
                    if a.kind == Some(CodeActionKind::REFACTOR_INLINE) =>
                {
                    Some(a)
                }
                _ => None,
            })
            .collect();
        assert_eq!(listed.len(), 1, "one inline action");
        assert_eq!(listed[0].title, "Inline `y`");
        assert!(listed[0].edit.is_none());

        let resolved = backend
            .code_action_resolve(listed[0].clone())
            .await
            .unwrap();
        let Some(DocumentChanges::Edits(doc_edits)) =
            &resolved.edit.as_ref().unwrap().document_changes
        else {
            panic!("expected versioned document edits");
        };
        assert_eq!(doc_edits[0].text_document.version, Some(1));
        let texts: Vec<String> = doc_edits[0]
            .edits
            .iter()
            .map(|e| match e {
                OneOf::Left(e) => e.new_text.clone(),
                OneOf::Right(e) => e.text_edit.new_text.clone(),
            })
            .collect();
        // Not canonical (two-space indent), so the raw edits apply: the `let`
        // line is deleted and the use replaced.
        assert_eq!(texts, vec![String::new(), "(a + 1)".to_string()]);

        let mut stale = listed[0].clone();
        stale.data.as_mut().unwrap()["version"] = serde_json::json!(0);
        assert!(backend.code_action_resolve(stale).await.is_err());
    }

    /// #2100 slice 4: a provider stub is listed on the unfiltered lightbulb
    /// request at a capability's name without an edit, and
    /// `codeAction/resolve` fills in the re-checked, versioned edit.
    #[tokio::test]
    async fn provider_stub_lists_then_resolves() {
        let src = "context app.c\n\ncapability Clock {\n  fn now() -> Effect[Int]\n}\n";
        let s = scratch_project(
            "provider_stub",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                ("src/app/c.bynk", src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = file_uri(&s.0, "src/app/c.bynk");
        open(&backend, &uri, src).await;
        backend.run_round().await;

        let at = crate::position::offset_to_position(src, src.find("Clock").unwrap());
        let actions = backend
            .code_action(CodeActionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                range: Range { start: at, end: at },
                context: CodeActionContext::default(),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            })
            .await
            .unwrap()
            .unwrap();
        let listed: Vec<CodeAction> = actions
            .into_iter()
            .filter_map(|a| match a {
                CodeActionOrCommand::CodeAction(a) if a.title.starts_with("Generate provider") => {
                    Some(a)
                }
                _ => None,
            })
            .collect();
        assert_eq!(listed.len(), 1, "one stub action");
        assert_eq!(listed[0].kind, Some(CodeActionKind::REFACTOR));
        assert!(listed[0].edit.is_none());

        let resolved = backend
            .code_action_resolve(listed[0].clone())
            .await
            .unwrap();
        let Some(DocumentChanges::Edits(doc_edits)) =
            &resolved.edit.as_ref().unwrap().document_changes
        else {
            panic!("expected versioned document edits");
        };
        assert_eq!(doc_edits[0].text_document.version, Some(1));
        let OneOf::Left(edit) = &doc_edits[0].edits[0] else {
            panic!("expected a plain TextEdit");
        };
        assert_eq!(
            edit.new_text,
            "\n\nprovides Clock = ClockImpl {\n  fn now() -> Effect[Int] {\n    0\n  }\n}"
        );

        let mut stale = listed[0].clone();
        stale.data.as_mut().unwrap()["version"] = serde_json::json!(0);
        assert!(backend.code_action_resolve(stale).await.is_err());
    }

    /// #2100 slice 2: extract-function is listed without an edit and
    /// resolved (re-checked) lazily; a capability-using selection gets a
    /// disabled action with its reason, only for a client that shows those.
    #[tokio::test]
    async fn extract_function_lists_then_resolves_or_says_why_not() {
        let src = "context app.m\n\ncapability Clock {\n  fn now() -> Effect[Int]\n}\n\nprovides Clock = Sys {\n  fn now() -> Effect[Int] {\n    1\n  }\n}\n\nservice s {\n  on call(x: Int) -> Effect[Int] given Clock {\n    let n <- Clock.now()\n    let y = x * 2 + 1\n    n + y\n  }\n}\n";
        let s = scratch_project(
            "extract_fn",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                ("src/app/m.bynk", src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = file_uri(&s.0, "src/app/m.bynk");
        open(&backend, &uri, src).await;
        backend.run_round().await;

        let request = |sel: &str| {
            let start = src.find(sel).unwrap();
            CodeActionParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                range: Range {
                    start: crate::position::offset_to_position(src, start),
                    end: crate::position::offset_to_position(src, start + sel.len()),
                },
                context: CodeActionContext::default(),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
            }
        };
        let extract_fn = |actions: Vec<CodeActionOrCommand>| -> Vec<CodeAction> {
            actions
                .into_iter()
                .filter_map(|a| match a {
                    CodeActionOrCommand::CodeAction(a)
                        if a.title.starts_with("Extract function") =>
                    {
                        Some(a)
                    }
                    _ => None,
                })
                .collect()
        };

        let listed = extract_fn(
            backend
                .code_action(request("x * 2 + 1"))
                .await
                .unwrap()
                .unwrap(),
        );
        assert_eq!(listed.len(), 1);
        assert!(listed[0].edit.is_none());
        let resolved = backend
            .code_action_resolve(listed[0].clone())
            .await
            .unwrap();
        let Some(DocumentChanges::Edits(doc_edits)) =
            &resolved.edit.as_ref().unwrap().document_changes
        else {
            panic!("expected versioned document edits");
        };
        assert_eq!(doc_edits[0].text_document.version, Some(1));
        assert_eq!(doc_edits[0].edits.len(), 2);

        // Without `disabledSupport` a capability-using selection lists nothing.
        let refused = extract_fn(
            backend
                .code_action(request("Clock.now()"))
                .await
                .unwrap()
                .unwrap(),
        );
        assert!(refused.is_empty());
        // With it, a disabled action says why.
        backend.state.write().await.supports_code_action_disabled = true;
        let refused = extract_fn(
            backend
                .code_action(request("Clock.now()"))
                .await
                .unwrap()
                .unwrap(),
        );
        assert_eq!(refused.len(), 1);
        let reason = &refused[0].disabled.as_ref().expect("disabled").reason;
        assert!(reason.contains("capability-free"), "{reason}");
    }

    /// #2117 review: fix-all is withheld when its combined edit fails the
    /// DECISION B re-check. The committed round's diagnostics are replaced
    /// with one machine-applicable suggestion: a harmless one is offered, one
    /// that introduces a type error is not.
    #[tokio::test]
    async fn fix_all_is_withheld_when_the_recheck_fails() {
        use bynk_syntax::error::{Applicability, CompileError};
        let src = "commons app.m\n\nfn f() -> Int {\n  1\n}\n";
        let s = scratch_project(
            "fix_all_gate",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                ("src/app/m.bynk", src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = file_uri(&s.0, "src/app/m.bynk");
        open(&backend, &uri, src).await;
        backend.run_round().await;

        let one = src.find("  1\n").unwrap() + 2;
        let one = bynk_syntax::span::Span::new(one, one + 1);
        // Swap the round's diagnostics for one carrying `replacement` as a
        // machine-applicable fix of the `1` body. It reuses a registered code:
        // the diagnostics registry rejects any `bynk.*` literal it doesn't
        // list.
        let inject = |replacement: &'static str| {
            let backend = backend.clone();
            async move {
                let root = backend.test_root().await;
                let mut state = backend.state.write().await;
                let project = state.projects.get_mut(&root).unwrap();
                let analysis = Arc::get_mut(project.analysis.as_mut().unwrap())
                    .expect("the round is not shared");
                let rel = analysis.snapshots.keys().next().unwrap().clone();
                let error = CompileError::new("bynk.given.unused_capability", one, "injected")
                    .with_suggestion(
                        "replace",
                        vec![(one, replacement.to_string())],
                        Applicability::MachineApplicable,
                    );
                analysis.diagnostics.insert(
                    rel,
                    vec![bynk_ide::Diagnostic {
                        severity: bynk_syntax::Severity::Warning,
                        label_files: Vec::new(),
                        error,
                    }],
                );
            }
        };
        let fix_all_count = || {
            let backend = backend.clone();
            let uri = uri.clone();
            async move {
                backend
                    .code_action(CodeActionParams {
                        text_document: TextDocumentIdentifier { uri },
                        range: Range::default(),
                        context: CodeActionContext {
                            diagnostics: Vec::new(),
                            only: Some(vec![crate::fix_all::kind()]),
                            trigger_kind: None,
                        },
                        work_done_progress_params: Default::default(),
                        partial_result_params: Default::default(),
                    })
                    .await
                    .unwrap()
                    .unwrap()
                    .len()
            }
        };

        inject("2").await;
        assert_eq!(fix_all_count().await, 1, "a clean fix is offered");
        inject("\"one\"").await;
        assert_eq!(fix_all_count().await, 0, "a breaking fix is withheld");
    }

    /// The v0.26 capability advertisements — the "trivial unit check" the
    /// proposal scopes in place of a transport round-trip.
    #[test]
    fn advertises_code_actions_and_the_index_riders() {
        let caps = server_capabilities();
        let Some(CodeActionProviderCapability::Options(opts)) = caps.code_action_provider else {
            panic!("codeActionProvider not advertised with options");
        };
        assert_eq!(
            opts.code_action_kinds,
            Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::REFACTOR,
                CodeActionKind::REFACTOR_EXTRACT,
                CodeActionKind::REFACTOR_INLINE,
                CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                CodeActionKind::new("source.fixAll.bynk"),
            ])
        );
        assert_eq!(
            opts.resolve_provider,
            Some(true),
            "fill-arms resolves lazily"
        );
        assert!(matches!(
            caps.workspace_symbol_provider,
            Some(OneOf::Left(true))
        ));
        assert!(matches!(
            caps.document_highlight_provider,
            Some(OneOf::Left(true))
        ));
    }

    /// The v0.27 capability advertisement — the "trivial unit check" the
    /// proposal scopes in place of a transport round-trip.
    #[test]
    fn advertises_save_notifications() {
        // `diagnostics_mode = "on_save"` is driven by `didSave`; the sync
        // options must opt in explicitly or clients may not send it (#513).
        let caps = server_capabilities();
        let Some(TextDocumentSyncCapability::Options(opts)) = caps.text_document_sync else {
            panic!("textDocumentSync not advertised with options");
        };
        assert_eq!(opts.change, Some(TextDocumentSyncKind::FULL));
        assert!(matches!(
            opts.save,
            Some(TextDocumentSyncSaveOptions::Supported(true))
        ));
    }

    #[test]
    fn advertises_inlay_hints() {
        let caps = server_capabilities();
        assert!(matches!(caps.inlay_hint_provider, Some(OneOf::Left(true))));
    }

    /// Slice 6: go-to-type-definition (value → its type's declaration).
    #[test]
    fn advertises_type_definition() {
        let caps = server_capabilities();
        assert!(matches!(
            caps.type_definition_provider,
            Some(TypeDefinitionProviderCapability::Simple(true))
        ));
    }

    /// Slice 6b: `uses`/`consumes` document links.
    #[test]
    fn advertises_document_links() {
        let caps = server_capabilities();
        assert!(caps.document_link_provider.is_some());
    }

    /// #302: `willRenameFiles` over `.bynk` files; #2105: and every folder.
    #[test]
    fn advertises_will_rename_files() {
        let caps = server_capabilities();
        let file_ops = caps
            .workspace
            .as_ref()
            .and_then(|w| w.file_operations.as_ref())
            .expect("workspace.fileOperations advertised");
        let will_rename = file_ops
            .will_rename
            .as_ref()
            .expect("willRename registered");
        let filter = &will_rename.filters[0];
        assert_eq!(filter.pattern.glob, "**/*.bynk");
        assert_eq!(filter.pattern.matches, Some(FileOperationPatternKind::File));
        let folders = &will_rename.filters[1];
        assert_eq!(folders.scheme.as_deref(), Some("file"));
        assert_eq!(folders.pattern.glob, "**");
        assert_eq!(
            folders.pattern.matches,
            Some(FileOperationPatternKind::Folder)
        );
    }

    /// #2098: `bynk/explain` is advertised under `experimental.explain`.
    #[test]
    fn advertises_the_explain_request() {
        let caps = server_capabilities();
        let experimental = caps.experimental.expect("experimental advertised");
        assert_eq!(experimental.get("explain"), Some(&serde_json::json!(true)));
    }

    /// #2471: `bynk/emittedOutput` is advertised under `experimental.emittedOutput`.
    #[test]
    fn advertises_the_emitted_output_request() {
        let caps = server_capabilities();
        let experimental = caps.experimental.expect("experimental advertised");
        assert_eq!(
            experimental.get("emittedOutput"),
            Some(&serde_json::json!(true))
        );
    }

    /// #2098: `initialize` records whether the client runs the explain command.
    #[tokio::test]
    async fn initialize_records_the_explain_command_capability() {
        for (experimental, expected) in [
            (None, false),
            (
                Some(serde_json::json!({ "explainDiagnosticCommand": true })),
                true,
            ),
        ] {
            let backend = bare_backend().await;
            backend
                .initialize(InitializeParams {
                    capabilities: ClientCapabilities {
                        experimental,
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .await
                .expect("initialize");
            assert_eq!(
                backend.state.read().await.supports_explain_command,
                expected
            );
        }
    }

    /// #2098: a curated diagnostic in range gets one "Explain …" quick fix,
    /// carrying the client command and the code, placed after every real fix
    /// and never preferred — but only for a client that runs the command.
    #[tokio::test]
    async fn code_action_offers_explain_for_a_curated_code_when_the_client_supports_it() {
        let src = "commons demo.a\n\nfn f(u: Usr) -> Int {\n  1\n}\n";
        let s = scratch_project(
            "explain_action",
            &[
                ("bynk.toml", "[project]\nname=\"demo\"\n"),
                ("src/demo/a.bynk", src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = file_uri(&s.0, "src/demo/a.bynk");
        backend.run_round().await;
        let at = crate::position::offset_to_position(src, src.find("Usr").unwrap());
        let request = || CodeActionParams {
            text_document: TextDocumentIdentifier { uri: uri.clone() },
            range: Range::new(at, at),
            context: CodeActionContext::default(),
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let titles = |actions: &CodeActionResponse| -> Vec<String> {
            actions
                .iter()
                .map(|a| match a {
                    CodeActionOrCommand::CodeAction(c) => c.title.clone(),
                    CodeActionOrCommand::Command(c) => c.title.clone(),
                })
                .collect()
        };
        let explain_title = "Explain `bynk.resolve.unknown_type`";

        // Without the client capability: no explain action.
        let without = backend.code_action(request()).await.unwrap().unwrap();
        assert!(
            !titles(&without).iter().any(|t| t == explain_title),
            "{:?}",
            titles(&without)
        );

        backend.state.write().await.supports_explain_command = true;
        let with = backend.code_action(request()).await.unwrap().unwrap();
        let names = titles(&with);
        let pos = names
            .iter()
            .position(|t| t == explain_title)
            .unwrap_or_else(|| panic!("explain action offered: {names:?}"));
        assert_eq!(pos, names.len() - 1, "explain comes last: {names:?}");
        let CodeActionOrCommand::CodeAction(action) = &with[pos] else {
            panic!("a CodeAction");
        };
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        assert_eq!(action.is_preferred, Some(false));
        assert!(action.edit.is_none());
        let cmd = action.command.as_ref().expect("carries a command");
        assert_eq!(cmd.command, "bynk.explainDiagnostic");
        assert_eq!(
            cmd.arguments,
            Some(vec![serde_json::json!("bynk.resolve.unknown_type")])
        );
    }

    /// #2098: the request itself, through the backend handler.
    #[tokio::test]
    async fn explain_request_serves_curated_and_declines_the_rest() {
        let backend = bare_backend().await;
        let ask = |code: &str| explain_request::ExplainParams { code: code.into() };
        let curated = backend
            .explain(ask("bynk.resolve.unknown_type"))
            .await
            .unwrap()
            .expect("curated");
        assert_eq!(curated.title, "bynk.resolve.unknown_type");
        assert!(
            backend
                .explain(ask("bynk.resolve.duplicate_type"))
                .await
                .unwrap()
                .is_none()
        );
        assert!(backend.explain(ask("nonsense")).await.unwrap().is_none());
    }

    /// Slice 5: completion advertises `.` triggers and lazy doc resolution.
    #[test]
    fn advertises_completion_with_dot_trigger_and_resolve() {
        let caps = server_capabilities();
        let opts = caps.completion_provider.expect("completion advertised");
        assert_eq!(opts.resolve_provider, Some(true), "resolve_provider");
        assert!(
            opts.trigger_characters
                .as_deref()
                .is_some_and(|t| t.iter().any(|c| c == ".")),
            "`.` trigger char"
        );
    }

    /// The v0.28 capability advertisement: full + range with the frozen
    /// legend (the legend's content is pinned in `index_queries`).
    #[test]
    fn advertises_semantic_tokens() {
        let caps = server_capabilities();
        let Some(SemanticTokensServerCapabilities::SemanticTokensOptions(opts)) =
            caps.semantic_tokens_provider
        else {
            panic!("semanticTokensProvider not advertised with options");
        };
        assert_eq!(opts.full, Some(SemanticTokensFullOptions::Bool(true)));
        assert_eq!(opts.range, Some(true));
        assert_eq!(opts.legend, crate::index_queries::semantic_tokens_legend());
    }

    // ---- Slice D: per-workspace state (Q4) ----

    /// A backend with **no** seeded project — the real lazy-discovery flow,
    /// where `did_open` and requests create entries by routing (`resolve_root`).
    async fn bare_backend() -> Backend {
        let (service, _socket) = tower_lsp::LspService::new(Backend::new);
        service.inner().clone()
    }

    fn file_uri(root: &std::path::Path, rel: &str) -> Url {
        let abs = root.join(rel);
        Url::from_file_path(abs.canonicalize().unwrap_or(abs)).unwrap()
    }

    async fn set_folders(backend: &Backend, roots: &[&std::path::Path]) {
        backend.state.write().await.folders = roots
            .iter()
            .map(|r| r.canonicalize().unwrap_or_else(|_| r.to_path_buf()))
            .collect();
    }

    async fn open(backend: &Backend, uri: &Url, text: &str) {
        backend
            .did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: "bynk".into(),
                    version: 1,
                    text: text.to_string(),
                },
            })
            .await;
    }

    fn snapshot_keys(a: &Analysis) -> Vec<String> {
        let mut keys: Vec<String> = a
            .snapshots
            .keys()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        keys.sort();
        keys
    }

    /// Two `bynk.toml` projects under **one** workspace folder are two projects
    /// (Q4: route by discovered root, not folder). Opening a file in each creates
    /// its own entry, and each analyses **only its own** tree — the overlay
    /// isolation guard, too: project A's round never sees project B's file.
    #[tokio::test]
    async fn two_projects_under_one_folder_are_two_projects() {
        let ax_src = "commons a.x\n\nfn ax(n: Int) -> Int {\n  n\n}\n";
        let by_src = "commons b.y\n\nfn by(n: Int) -> Int {\n  n\n}\n";
        let s = scratch_project(
            "d_two",
            &[
                ("a/bynk.toml", "[project]\nname=\"a\"\n"),
                ("a/src/x.bynk", ax_src),
                ("b/bynk.toml", "[project]\nname=\"b\"\n"),
                ("b/src/y.bynk", by_src),
            ],
        );
        let backend = bare_backend().await;
        set_folders(&backend, &[&s.0]).await;
        let ax = file_uri(&s.0, "a/src/x.bynk");
        let by = file_uri(&s.0, "b/src/y.bynk");
        open(&backend, &ax, ax_src).await;
        open(&backend, &by, by_src).await;

        assert_ne!(
            Backend::root_for_uri_uncached(&ax).unwrap(),
            Backend::root_for_uri_uncached(&by).unwrap(),
            "the two files resolve to different project roots",
        );
        assert_eq!(
            backend.state.read().await.projects.len(),
            2,
            "one entry per project, not one for the shared folder",
        );

        let a = backend.analysis_for(&ax).await.expect("A analysed");
        let b = backend.analysis_for(&by).await.expect("B analysed");
        assert_eq!(
            snapshot_keys(&a),
            vec!["src/x.bynk"],
            "A sees only A's file"
        );
        assert_eq!(
            snapshot_keys(&b),
            vec!["src/y.bynk"],
            "B sees only B's file"
        );
    }

    /// Q4 lifecycle: `did_change_workspace_folders` removing a folder with **no
    /// open buffer** prunes the idle project entry and clears nothing it must
    /// keep. Routing no longer resolves it because the seed is gone.
    #[tokio::test]
    async fn removing_a_folder_prunes_an_idle_project() {
        let a = "commons p.a\n\nfn f(n: Int) -> Int {\n  n\n}\n";
        let s = scratch_project(
            "d_prune",
            &[("bynk.toml", "[project]\nname=\"p\"\n"), ("src/a.bynk", a)],
        );
        let folder = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        let backend = bare_backend().await;
        set_folders(&backend, &[&s.0]).await;
        let uri = file_uri(&s.0, "src/a.bynk");
        open(&backend, &uri, a).await;
        backend.analysis_for(&uri).await.expect("analysed");
        // Close the buffer, so nothing but the folder pins the project.
        backend
            .did_close(DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
            })
            .await;
        assert_eq!(backend.state.read().await.projects.len(), 1);

        backend
            .did_change_workspace_folders(DidChangeWorkspaceFoldersParams {
                event: WorkspaceFoldersChangeEvent {
                    added: vec![],
                    removed: vec![WorkspaceFolder {
                        uri: Url::from_file_path(&folder).unwrap(),
                        name: "p".into(),
                    }],
                },
            })
            .await;
        assert!(
            backend.state.read().await.projects.is_empty(),
            "an idle project is pruned when its last covering folder is removed",
        );
    }

    /// Q4 lifecycle: a project that still holds an **open buffer** survives folder
    /// removal — routing needs it until the buffer closes.
    #[tokio::test]
    async fn removing_a_folder_retains_a_project_with_an_open_buffer() {
        let a = "commons p.a\n\nfn f(n: Int) -> Int {\n  n\n}\n";
        let s = scratch_project(
            "d_retain",
            &[("bynk.toml", "[project]\nname=\"p\"\n"), ("src/a.bynk", a)],
        );
        let folder = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        let backend = bare_backend().await;
        set_folders(&backend, &[&s.0]).await;
        let uri = file_uri(&s.0, "src/a.bynk");
        open(&backend, &uri, a).await; // buffer stays open

        backend
            .did_change_workspace_folders(DidChangeWorkspaceFoldersParams {
                event: WorkspaceFoldersChangeEvent {
                    added: vec![],
                    removed: vec![WorkspaceFolder {
                        uri: Url::from_file_path(&folder).unwrap(),
                        name: "p".into(),
                    }],
                },
            })
            .await;
        assert_eq!(
            backend.state.read().await.projects.len(),
            1,
            "a project with an open buffer must survive folder removal",
        );
        assert!(
            backend.analysis_for(&uri).await.is_some(),
            "and it must still answer requests",
        );
    }

    /// Q4 §C: closing the **last** buffer of a project whose folder was already
    /// removed prunes it — the mirror of the folder path. Without it the project
    /// lingers forever with published diagnostics no folder or buffer justifies.
    #[tokio::test]
    async fn closing_the_last_buffer_of_a_folder_removed_project_prunes_it() {
        let a = "commons p.a\n\nfn f(n: Int) -> Int {\n  n\n}\n";
        let s = scratch_project(
            "d_close_prune",
            &[("bynk.toml", "[project]\nname=\"p\"\n"), ("src/a.bynk", a)],
        );
        let folder = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        let backend = bare_backend().await;
        set_folders(&backend, &[&s.0]).await;
        let uri = file_uri(&s.0, "src/a.bynk");
        open(&backend, &uri, a).await;
        backend.analysis_for(&uri).await.expect("analysed");

        // Remove the folder while the buffer is open — retained (its buffer pins it).
        backend
            .did_change_workspace_folders(DidChangeWorkspaceFoldersParams {
                event: WorkspaceFoldersChangeEvent {
                    added: vec![],
                    removed: vec![WorkspaceFolder {
                        uri: Url::from_file_path(&folder).unwrap(),
                        name: "p".into(),
                    }],
                },
            })
            .await;
        assert_eq!(
            backend.state.read().await.projects.len(),
            1,
            "retained while its buffer is open",
        );

        // Close the last buffer — now fully orphaned (no folder, no buffer).
        backend
            .did_close(DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
            })
            .await;
        assert!(
            backend.state.read().await.projects.is_empty(),
            "closing the last buffer of a folder-removed project must prune it",
        );
    }

    /// Q4: a rename spans **one** project — a stale buffer in another project must
    /// not block it (`analysis_covering_open_buffers` is per-project). Under a
    /// whole-server gate, B's dirty buffer would refuse A's rename.
    #[tokio::test]
    async fn a_rename_in_one_project_ignores_a_dirty_buffer_in_another() {
        let a_src = "commons a.x\n\ntype Money = Int where Positive\n\nfn charge(m: Money) -> Money {\n  m\n}\n";
        let b_src = "commons b.y\n\nfn by(n: Int) -> Int {\n  n\n}\n";
        let s = scratch_project(
            "d_rename_iso",
            &[
                ("a/bynk.toml", "[project]\nname=\"a\"\n"),
                ("a/src/x.bynk", a_src),
                ("b/bynk.toml", "[project]\nname=\"b\"\n"),
                ("b/src/y.bynk", b_src),
            ],
        );
        let backend = bare_backend().await;
        set_folders(&backend, &[&s.0]).await;
        let ax = file_uri(&s.0, "a/src/x.bynk");
        let by = file_uri(&s.0, "b/src/y.bynk");
        open(&backend, &ax, a_src).await;
        open(&backend, &by, b_src).await;
        backend.analysis_for(&ax).await.expect("A analysed");
        backend.analysis_for(&by).await.expect("B analysed");

        // Make B's buffer dirty (version 2, not yet re-analysed).
        backend
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: by.clone(),
                    version: 2,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: format!("\n{b_src}"),
                }],
            })
            .await;

        // Rename `Money` in A — must succeed despite B being dirty.
        let off = a_src.find("Money").unwrap();
        let pos = crate::position::offset_to_position(a_src, off);
        let edit = backend
            .rename(RenameParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: ax.clone() },
                    position: pos,
                },
                new_name: "Amount".into(),
                work_done_progress_params: Default::default(),
            })
            .await
            .expect("rename must not error");
        assert!(
            edit.is_some(),
            "a rename in project A must not be blocked by a dirty buffer in project B",
        );
    }

    // ---- Slice E: startup analysis & dynamic watchers ----

    /// `initialize` captures the client's `didChangeWatchedFiles` dynamic-
    /// registration support, which gates the server-side watcher registration.
    #[tokio::test]
    async fn initialize_captures_the_dynamic_watcher_capability() {
        let backend = bare_backend().await;
        let params = InitializeParams {
            capabilities: ClientCapabilities {
                workspace: Some(WorkspaceClientCapabilities {
                    did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                        dynamic_registration: Some(true),
                        relative_pattern_support: None,
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        backend.initialize(params).await.expect("initialize");
        assert!(
            backend.state.read().await.supports_dynamic_watchers,
            "the client's dynamic-registration support must be captured for `initialized`",
        );
    }

    /// #733: `initialize` captures each pull-based decoration's `refresh_support`
    /// independently — the flag gates whether a committed round nudges the client
    /// to re-pull that decoration. The three `and_then` chains are easy to
    /// mis-wire (a swapped field reads the wrong capability), so pin each: two
    /// advertised, one withheld, one whole family absent.
    #[tokio::test]
    async fn initialize_captures_each_decoration_refresh_capability() {
        let backend = bare_backend().await;
        let params = InitializeParams {
            capabilities: ClientCapabilities {
                workspace: Some(WorkspaceClientCapabilities {
                    // Semantic tokens: advertised.
                    semantic_tokens: Some(SemanticTokensWorkspaceClientCapabilities {
                        refresh_support: Some(true),
                    }),
                    // Inlay hints: explicitly withheld.
                    inlay_hint: Some(InlayHintWorkspaceClientCapabilities {
                        refresh_support: Some(false),
                    }),
                    // Code lens: the whole family absent (no capability at all).
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        backend.initialize(params).await.expect("initialize");
        let refresh = backend.state.read().await.supports_refresh;
        assert!(
            refresh.semantic_tokens,
            "semantic tokens: advertised → true"
        );
        assert!(!refresh.inlay_hints, "inlay hints: withheld → false");
        assert!(!refresh.code_lens, "code lens: absent → false");
    }

    /// #2207: a workspace root's `bynk.toml` isn't a project; its members are.
    #[tokio::test]
    async fn discover_projects_under_skips_a_workspace_root() {
        let s = scratch_project(
            "e_discover_workspace",
            &[
                ("bynk.toml", "[workspace]\nmembers = [\"packages/*\"]\n"),
                ("packages/a/bynk.toml", "[package]\nname=\"acme/a\"\n"),
                ("packages/a/src/x.bynk", "commons a.x\n"),
            ],
        );
        let roots = Backend::discover_projects_under(&s.0);
        let names: Vec<String> = roots
            .iter()
            .map(|r| r.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["a"], "the member, not the root; got {roots:?}");
    }

    /// The discovery walk finds every nested `bynk.toml` project under a folder
    /// (a monorepo), and skips the caches it must never descend.
    #[tokio::test]
    async fn discover_projects_under_finds_nested_projects_and_skips_caches() {
        let s = scratch_project(
            "e_discover",
            &[
                ("packages/a/bynk.toml", "[project]\nname=\"a\"\n"),
                ("packages/a/src/x.bynk", "commons a.x\n"),
                ("packages/b/bynk.toml", "[project]\nname=\"b\"\n"),
                ("packages/b/src/y.bynk", "commons b.y\n"),
                // A manifest under a skipped dir must NOT be discovered.
                ("node_modules/dep/bynk.toml", "[project]\nname=\"dep\"\n"),
            ],
        );
        let mut roots = Backend::discover_projects_under(&s.0);
        roots.sort();
        let names: Vec<String> = roots
            .iter()
            .map(|r| r.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec!["a", "b"],
            "both monorepo projects found, node_modules skipped; got {roots:?}",
        );
    }

    /// Startup analysis: `initialized` warms every project under the workspace
    /// folders — creating each entry so diagnostics/features are ready — **with
    /// no `did_open`**. This is spec §2.3's documented startup analysis.
    #[tokio::test]
    async fn initialized_warms_every_project_under_the_folders() {
        let s = scratch_project(
            "e_warm",
            &[
                ("packages/a/bynk.toml", "[project]\nname=\"a\"\n"),
                (
                    "packages/a/src/x.bynk",
                    "commons a.x\n\nfn ax(n: Int) -> Int {\n  n\n}\n",
                ),
                ("packages/b/bynk.toml", "[project]\nname=\"b\"\n"),
                (
                    "packages/b/src/y.bynk",
                    "commons b.y\n\nfn by(n: Int) -> Int {\n  n\n}\n",
                ),
            ],
        );
        let backend = bare_backend().await;
        set_folders(&backend, &[&s.0]).await;

        // No file opened — just the handshake completion.
        backend.initialized(InitializedParams {}).await;

        assert_eq!(
            backend.state.read().await.projects.len(),
            2,
            "both monorepo projects are warmed at `initialized`, before any open",
        );
        // And each is genuinely analysable without an open buffer.
        let ax = file_uri(&s.0, "packages/a/src/x.bynk");
        assert!(
            backend.analysis_for(&ax).await.is_some(),
            "a warmed project answers index requests with no `did_open`",
        );
    }

    /// The implicit-`src/` project (#485 — a `src/` tree with no `bynk.toml`) is
    /// warmed at startup too, not only lazily on first open. `resolve_root` finds
    /// only a `src/` *ancestor*, so the folder-is-the-root case needs the explicit
    /// check in `discover_projects_under`.
    #[tokio::test]
    async fn initialized_warms_an_implicit_src_project() {
        let s = scratch_project(
            "e_implicit",
            &[(
                "src/a.bynk",
                "commons demo.a\n\nfn f(n: Int) -> Int {\n  n\n}\n",
            )],
        );
        let backend = bare_backend().await;
        set_folders(&backend, &[&s.0]).await;
        backend.initialized(InitializedParams {}).await;
        assert_eq!(
            backend.state.read().await.projects.len(),
            1,
            "a rootless `src/` project is warmed at startup, not only on open",
        );
    }

    /// Review of #677: the discovery walk must not follow a symlink cycle into a
    /// stack overflow — a `loop -> .` in an ordinary directory. The visited-set
    /// (canonicalised dirs) bounds it.
    #[cfg(unix)]
    #[tokio::test]
    async fn discover_projects_under_survives_a_symlink_cycle() {
        let s = scratch_project(
            "e_cycle",
            &[
                ("bynk.toml", "[project]\nname=\"p\"\n"),
                ("src/a.bynk", "commons p.a\n"),
            ],
        );
        // A directory symlink pointing back at the folder — a cycle.
        std::os::unix::fs::symlink(&s.0, s.0.join("loop")).ok();
        let roots = Backend::discover_projects_under(&s.0); // must terminate
        let canon = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        assert!(
            roots.contains(&canon),
            "the project is found and the walk terminates despite the cycle",
        );
    }

    /// Review of #677: with the per-query `workspace/symbol` walk dropped, a
    /// `bynk.toml` **created** after startup is picked up via its watcher event
    /// — the watcher warms the new project.
    #[tokio::test]
    async fn a_created_manifest_warms_a_new_project() {
        let s = scratch_project("e_created", &[("src/a.bynk", "commons p.a\n")]);
        std::fs::write(s.0.join("bynk.toml"), "[project]\nname=\"p\"\n").unwrap();
        let root = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        let backend = bare_backend().await;
        set_folders(&backend, &[&s.0]).await;
        assert!(
            backend.state.read().await.projects.is_empty(),
            "no entry before the watcher fires",
        );

        let toml_uri = Url::from_file_path(root.join("bynk.toml")).unwrap();
        backend
            .did_change_watched_files(DidChangeWatchedFilesParams {
                changes: vec![FileEvent {
                    uri: toml_uri,
                    typ: FileChangeType::CREATED,
                }],
            })
            .await;
        assert_eq!(
            backend.state.read().await.projects.len(),
            1,
            "a created bynk.toml warms its project via the watcher event",
        );
    }

    /// #2081: a `bynk.schema.lock` watcher event clears that project's cached
    /// lock, so a rewrite the stamp cannot see (same length, same mtime: an
    /// edit within one mtime tick) is still read on the next round.
    #[tokio::test]
    async fn a_schema_lock_watcher_event_invalidates_the_cached_lock() {
        const OLD: &str = "version = 1\n# aaaa\n";
        const NEW: &str = "version = 1\n# bbbb\n";
        let s = scratch_project(
            "lock_watch",
            &[
                ("bynk.toml", "[project]\nname = \"p\"\n"),
                ("bynk.schema.lock", OLD),
            ],
        );
        let backend = backend_at(&s.0).await;
        let root = backend.test_root().await;
        let lock = root.join("bynk.schema.lock");
        let on = |text: &str| bynk_ide::SchemaLock::On {
            existing: Some(text.to_string()),
        };
        assert_eq!(backend.schema_lock_for(&root).await, on(OLD));

        // Rewrite with the same length, then put the old mtime back: the
        // stamp is unchanged, so only the watcher can tell the cache.
        let mtime = std::fs::metadata(&lock).unwrap().modified().unwrap();
        std::fs::write(&lock, NEW).unwrap();
        std::fs::File::options()
            .write(true)
            .open(&lock)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        assert_eq!(
            backend.schema_lock_for(&root).await,
            on(OLD),
            "an unchanged stamp serves the cache"
        );

        backend
            .did_change_watched_files(DidChangeWatchedFilesParams {
                changes: vec![FileEvent {
                    uri: Url::from_file_path(&lock).unwrap(),
                    typ: FileChangeType::CHANGED,
                }],
            })
            .await;
        assert_eq!(
            backend.schema_lock_for(&root).await,
            on(NEW),
            "the watcher event must clear the cached lock"
        );
    }

    /// #682: a repeated route for the same URI is served from `root_cache`
    /// rather than re-walking the filesystem each time — a `None` route
    /// (single-file mode) is cached too, since it's just as stable an answer.
    #[tokio::test]
    async fn root_for_uri_populates_the_cache() {
        let s = scratch_project("g_cache_hit", &[("a.bynk", "commons demo.a\n")]);
        let uri = file_uri(&s.0, "a.bynk");
        let backend = bare_backend().await;

        assert!(
            backend.root_for_uri(&uri).await.is_none(),
            "no bynk.toml and no src/ ancestor — routes to no project",
        );
        assert_eq!(
            backend.state.read().await.root_cache.get(&uri),
            Some(&None),
            "the miss is cached too",
        );
    }

    /// #682 (DECISION C): a `bynk.toml` created after a URI was already routed
    /// (and cached) re-routes that URI once the watcher event invalidates the
    /// cache — a stale cached `None` must not survive the manifest's arrival.
    #[tokio::test]
    async fn a_created_manifest_invalidates_the_cached_route() {
        let s = scratch_project("g_cache_invalidate", &[("a.bynk", "commons p.a\n")]);
        let uri = file_uri(&s.0, "a.bynk");
        let backend = bare_backend().await;

        assert!(
            backend.root_for_uri(&uri).await.is_none(),
            "precondition: cached as routing to no project",
        );

        std::fs::write(s.0.join("bynk.toml"), "[project]\nname=\"p\"\n").unwrap();
        let root = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        let toml_uri = Url::from_file_path(root.join("bynk.toml")).unwrap();
        backend
            .did_change_watched_files(DidChangeWatchedFilesParams {
                changes: vec![FileEvent {
                    uri: toml_uri,
                    typ: FileChangeType::CREATED,
                }],
            })
            .await;

        assert_eq!(
            backend.root_for_uri(&uri).await,
            Some(root),
            "re-routes to the new project now the stale cache entry is gone",
        );
    }

    /// #822: the guard `root_for_uri` checks before writing back a cache miss
    /// — an accidental `!=`-for-`==` inversion here would silently reopen the
    /// TOCTOU the generation counter exists to close, and the real race is too
    /// timing-dependent to exercise deterministically, so this pins the
    /// predicate directly.
    #[test]
    fn root_cache_write_is_current_rejects_a_generation_that_moved() {
        assert!(
            Backend::root_cache_write_is_current(3, 3),
            "no clear happened since the read — the write-back applies",
        );
        assert!(
            !Backend::root_cache_write_is_current(3, 4),
            "a clear bumped the generation since the read — the write-back must be dropped",
        );
    }

    /// #822: both `root_cache.clear()` sites must bump `root_cache_generation`
    /// alongside the clear — the guard only closes the TOCTOU if every
    /// invalidation does both. `did_change_watched_files`'s bump is covered
    /// indirectly by `a_created_manifest_invalidates_the_cached_route`; this
    /// covers `did_change_workspace_folders`'s directly, since a regression
    /// dropping just that one bump would reopen the race there specifically.
    #[tokio::test]
    async fn a_workspace_folder_change_bumps_the_root_cache_generation() {
        let s = scratch_project("g_race_folder", &[("bynk.toml", "[project]\nname=\"p\"\n")]);
        let backend = bare_backend().await;
        let before = backend.state.read().await.root_cache_generation;

        backend
            .did_change_workspace_folders(DidChangeWorkspaceFoldersParams {
                event: WorkspaceFoldersChangeEvent {
                    added: vec![WorkspaceFolder {
                        uri: Url::from_file_path(&s.0).unwrap(),
                        name: "p".into(),
                    }],
                    removed: vec![],
                },
            })
            .await;

        assert!(
            backend.state.read().await.root_cache_generation > before,
            "a workspace-folder change must bump the generation, not just clear the cache",
        );
    }

    /// A folder added at runtime is warmed the same way (the proactive scan
    /// slice D deferred to E), so its projects appear without an open.
    #[tokio::test]
    async fn an_added_folder_is_warmed() {
        let s = scratch_project(
            "e_added",
            &[
                ("bynk.toml", "[project]\nname=\"p\"\n"),
                (
                    "src/a.bynk",
                    "commons p.a\n\nfn f(n: Int) -> Int {\n  n\n}\n",
                ),
            ],
        );
        let folder = s.0.canonicalize().unwrap_or_else(|_| s.0.clone());
        let backend = bare_backend().await; // no folders yet
        assert!(backend.state.read().await.projects.is_empty());

        backend
            .did_change_workspace_folders(DidChangeWorkspaceFoldersParams {
                event: WorkspaceFoldersChangeEvent {
                    added: vec![WorkspaceFolder {
                        uri: Url::from_file_path(&folder).unwrap(),
                        name: "p".into(),
                    }],
                    removed: vec![],
                },
            })
            .await;

        assert_eq!(
            backend.state.read().await.projects.len(),
            1,
            "an added workspace folder's project is warmed proactively",
        );
    }

    // ---- Slice F: one diagnostics scheduler ----

    fn change_params(uri: &Url, version: i32, text: &str) -> DidChangeTextDocumentParams {
        DidChangeTextDocumentParams {
            text_document: VersionedTextDocumentIdentifier {
                uri: uri.clone(),
                version,
            },
            content_changes: vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_string(),
            }],
        }
    }

    /// Slice F: a **single-file** buffer (no project) now debounces by
    /// generation — a burst bumps the URI's generation once per change, so only
    /// the last-scheduled task survives its freshness check and runs `diagnose`.
    /// Before F single-file had no generation and ran once per keystroke.
    #[tokio::test]
    async fn a_single_file_burst_coalesces_by_generation() {
        // A `.bynk` file with no `bynk.toml` and no `src/` — single-file mode.
        let s = scratch_project("f_single", &[("a.bynk", "commons demo.a\n")]);
        let uri = file_uri(&s.0, "a.bynk");
        assert!(
            Backend::root_for_uri_uncached(&uri).is_none(),
            "precondition: the file routes to no project",
        );
        let backend = bare_backend().await;
        for _ in 0..3 {
            backend.schedule_single_file(uri.clone()).await;
        }
        assert_eq!(
            backend
                .state
                .read()
                .await
                .single_file_generations
                .get(&uri)
                .copied(),
            Some(3),
            "each change bumps the generation; only the third task passes its check",
        );
    }

    /// Slice F: `did_close` clears a single-file buffer's debounce generation, so
    /// the map does not grow unboundedly across a session.
    #[tokio::test]
    async fn did_close_clears_the_single_file_generation() {
        let s = scratch_project("f_close", &[("a.bynk", "commons demo.a\n")]);
        let uri = file_uri(&s.0, "a.bynk");
        let backend = bare_backend().await;
        backend.schedule_single_file(uri.clone()).await;
        assert!(
            backend
                .state
                .read()
                .await
                .single_file_generations
                .contains_key(&uri),
            "the generation exists after scheduling",
        );
        backend
            .did_close(DidCloseTextDocumentParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
            })
            .await;
        assert!(
            !backend
                .state
                .read()
                .await
                .single_file_generations
                .contains_key(&uri),
            "did_close clears the single-file generation",
        );
    }

    /// Slice F: `did_change` in **project** mode now feeds the one generation-
    /// based scheduler directly (no separate pre-sleep, no second hardcoded
    /// debounce). A burst bumps the project's generation once per change, so a
    /// single round survives — coalescing, through the real handler.
    #[tokio::test]
    async fn a_project_change_burst_coalesces_through_did_change() {
        let src = "commons q.a\n\nfn f(x: Int) -> Int {\n  x\n}\n";
        let s = scratch_project(
            "f_burst",
            &[
                ("bynk.toml", "[project]\nname=\"q\"\n"),
                ("src/a.bynk", src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let root = backend.test_root().await;
        let uri = file_uri(&s.0, "src/a.bynk");
        open(&backend, &uri, src).await;

        let gen_before = {
            let state = backend.state.read().await;
            state.projects.get(&root).unwrap().analysis_generation
        };
        for v in 2..=5 {
            backend
                .did_change(change_params(
                    &uri,
                    v,
                    &format!("{}{src}", "\n".repeat(v as usize)),
                ))
                .await;
        }
        let gen_after = {
            let state = backend.state.read().await;
            state.projects.get(&root).unwrap().analysis_generation
        };
        assert_eq!(
            gen_after - gen_before,
            4,
            "each of the four changes bumps the generation once — only the last \
             scheduled round runs (no per-change round, no stacked debounce)",
        );
    }

    // -- #596: store-map query vocabulary, end to end through `completion` ----
    //
    // The unit tests in `completion.rs`/`kernel_methods.rs`/`store_ops.rs`
    // cover each half in isolation; a #812 review flagged the gap that no test
    // drove a real `textDocument/completion` request through `Backend` to
    // check the two halves actually merge (and, separately, that the
    // provenance-based half survives a project-wide resolve failure that
    // blanks `type_receiver`). These close both.

    fn completion_labels(response: Option<CompletionResponse>) -> Vec<String> {
        match response {
            Some(CompletionResponse::Array(items)) => items.into_iter().map(|i| i.label).collect(),
            Some(CompletionResponse::List(list)) => {
                list.items.into_iter().map(|i| i.label).collect()
            }
            None => Vec::new(),
        }
    }

    async fn complete_at(backend: &Backend, uri: &Url, text: &str, needle: &str) -> Vec<String> {
        let offset = text.find(needle).expect("needle present") + needle.len();
        let pos = crate::position::offset_to_position(text, offset);
        let response = backend
            .completion(CompletionParams {
                text_document_position: TextDocumentPositionParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                    position: pos,
                },
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: None,
            })
            .await
            .expect("completion must not error");
        completion_labels(response)
    }

    /// A `store Map` field's `.` completion merges both halves in one
    /// response: the `Query` kernel methods (`filter`, `collect`, …) from
    /// `kernel_methods::methods_for`, and the store-field vocabulary (entry
    /// ops + accessors) from the provenance-based path — driven through the
    /// real `Backend::completion`, not the pure helpers directly.
    #[tokio::test]
    async fn store_map_receiver_completion_merges_both_vocabularies() {
        let src = "context shop\n\nagent Inventory {\n  key id: String\n  store items: Map[String, Int]\n\n  on call f() -> Effect[()] {\n    items.\n  }\n}\n";
        let s = scratch_project(
            "store_map_merge",
            &[
                ("bynk.toml", "[project]\nname=\"shop\"\n"),
                ("src/a.bynk", src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = file_uri(&s.0, "src/a.bynk");
        open(&backend, &uri, src).await;
        backend.run_round().await;

        let labels = complete_at(&backend, &uri, src, "    items.").await;
        assert!(
            labels.contains(&"filter".to_string()),
            "the Query kernel vocabulary: {labels:?}"
        );
        assert!(
            labels.contains(&"collect".to_string()),
            "the Query kernel vocabulary: {labels:?}"
        );
        assert!(
            labels.contains(&"put".to_string()),
            "the store entry ops: {labels:?}"
        );
        assert!(
            labels.contains(&"entries".to_string()),
            "the Map query accessors: {labels:?}"
        );
    }

    /// #2010: inside `let <pattern> … else match { … }`, arm-start completion
    /// offers the value's variants minus the one the pattern already matches —
    /// through the real `Backend::completion`, for `=` and for `<-` (whose
    /// `Effect` payload is what the arms match).
    #[tokio::test]
    async fn let_else_match_offers_the_residual_variants() {
        let src = "context shop\n\nfn f(r: Result[Int, String]) -> Int {\n  let Ok(n) = r else match {\n    Err(e) => 0,\n  }\n  n\n}\n\nfn g(o: Option[Int]) -> Effect[Int] {\n  let Some(m) <- Effect.pure(o) else match {\n    None => 0,\n  }\n  m\n}\n";
        let s = scratch_project(
            "let_else_match_residual",
            &[
                ("bynk.toml", "[project]\nname=\"shop\"\n"),
                ("src/shop.bynk", src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = file_uri(&s.0, "src/shop.bynk");
        open(&backend, &uri, src).await;
        backend.run_round().await;

        let labels = complete_at(&backend, &uri, src, "r else match {\n    ").await;
        assert!(labels.contains(&"Err".to_string()), "{labels:?}");
        assert!(
            !labels.contains(&"Ok".to_string()),
            "`Ok(n)` covers `Ok`: {labels:?}"
        );

        let labels = complete_at(&backend, &uri, src, "pure(o) else match {\n    ").await;
        assert!(labels.contains(&"None".to_string()), "{labels:?}");
        assert!(
            !labels.contains(&"Some".to_string()),
            "`Some(m)` covers `Some`: {labels:?}"
        );
    }

    /// An unresolved type name elsewhere in the same file — in an unrelated
    /// `type` declaration, not even the agent using `items` — trips the
    /// *resolve* gate. That once blanked `expr_types` for the **whole file**
    /// (the one clean-file-ceiling gap ADR 0094 didn't close), and before the
    /// #812 review fix `value_member_completions` then returned early and
    /// never reached the store-field path. Since #1663 (Decision A) the
    /// checker runs past a resolve error, so both halves survive: the typed
    /// `Query` kernel methods and the store-field entry ops/accessors.
    #[tokio::test]
    async fn store_field_vocabulary_survives_an_unrelated_resolve_failure() {
        let src = "context shop\n\ntype Bad = { x: NoSuchType }\n\nagent Inventory {\n  key id: String\n  store items: Map[String, Int]\n\n  on call f() -> Effect[()] {\n    items.\n  }\n}\n";
        let s = scratch_project(
            "store_map_resolve_gap",
            &[
                ("bynk.toml", "[project]\nname=\"shop\"\n"),
                ("src/a.bynk", src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let uri = file_uri(&s.0, "src/a.bynk");
        open(&backend, &uri, src).await;
        backend.run_round().await;

        // Precondition: the round really did fail to type this file (the
        // fixture actually reaches the ceiling this test is about, rather
        // than passing vacuously because the file happened to check fine).
        let analysis = backend.test_analysis().await.expect("a round committed");
        let rel = Backend::uri_to_rel(&analysis, &uri).expect("uri resolves");
        assert!(
            analysis
                .diagnostics
                .get(&rel)
                .is_some_and(|ds| !ds.is_empty()),
            "the fixture must actually fail to check — an undeclared return \
             type is the trigger this test exercises",
        );

        let labels = complete_at(&backend, &uri, src, "    items.").await;
        // #1663: the resolve error in `Bad` no longer blanks the agent's types,
        // so the typed half (`Query` kernel methods) survives too.
        assert!(
            labels.contains(&"filter".to_string()),
            "an unrelated resolve error must not blank the typed half: {labels:?}"
        );
        assert!(
            labels.contains(&"put".to_string()),
            "store entry ops must survive an unrelated resolve failure: {labels:?}"
        );
        assert!(
            labels.contains(&"entries".to_string()),
            "Map query accessors must survive an unrelated resolve failure: {labels:?}"
        );
    }

    /// Content-ownership track (#1086) §8's "done when": an unsaved edit in
    /// file A is visible to a completion triggered from file B — driven
    /// through a real `Backend` (`did_open`/`did_change` → `completion`),
    /// not the pure `bynk_ide` helpers directly. `shared/widget.bynk`
    /// declares `Widget` with one field on disk; opening it and editing its
    /// buffer (never saved) to add a second field must be visible to
    /// `app/use.bynk`'s cross-file record-construction completion
    /// (`Widget { <cursor>`, via `record_field_names`/`project_content`) in
    /// the very same round. This exercises the sweep both files' rounds
    /// share, not `type_receiver` specifically — see
    /// `type_receivers_slow_path_sees_a_closed_files_disk_content` below for
    /// that path's own dedicated coverage (a review of this PR found this
    /// test alone doesn't reach it).
    #[tokio::test]
    async fn an_unsaved_edit_in_one_file_is_visible_to_completion_in_another() {
        let widget_v1 = "commons shared.widget\n\ntype Widget = { size: Int }\n";
        let widget_v2 = "commons shared.widget\n\ntype Widget = { size: Int, weight: Int }\n";
        let use_src =
            "commons app.use\n\nuses shared.widget\n\nfn make() -> Widget {\n  Widget { \n}\n";
        let s = scratch_project(
            "cross_file_unsaved",
            &[
                (
                    "bynk.toml",
                    "[project]\nname = \"cross\"\n\n[paths]\ninclude = [\"shared\", \"app\"]\n",
                ),
                ("shared/widget.bynk", widget_v1),
                ("app/use.bynk", use_src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let widget_uri = file_uri(&s.0, "shared/widget.bynk");
        let use_uri = file_uri(&s.0, "app/use.bynk");
        open(&backend, &widget_uri, widget_v1).await;
        open(&backend, &use_uri, use_src).await;
        backend.run_round().await;

        // Precondition: before the edit, only the on-disk field completes.
        let before = complete_at(&backend, &use_uri, use_src, "  Widget { ").await;
        assert!(before.contains(&"size".to_string()), "{before:?}");
        assert!(
            !before.contains(&"weight".to_string()),
            "the fixture must not already have `weight` on disk: {before:?}"
        );

        // Edit `widget.bynk`'s buffer — never saved to disk — to add `weight`.
        backend
            .did_change(DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: widget_uri.clone(),
                    version: 2,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    range: None,
                    range_length: None,
                    text: widget_v2.to_string(),
                }],
            })
            .await;

        let after = complete_at(&backend, &use_uri, use_src, "  Widget { ").await;
        assert!(
            after.contains(&"weight".to_string()),
            "an unsaved edit to `widget.bynk` must be visible to `use.bynk`'s \
             cross-file completion in the same round: {after:?}"
        );
        assert!(after.contains(&"size".to_string()), "{after:?}");

        // On-disk content is untouched — the visibility came from the buffer.
        assert_eq!(
            std::fs::read_to_string(s.0.join("shared/widget.bynk")).unwrap(),
            widget_v1,
            "the edit must never have been saved to disk"
        );
    }

    /// Content-ownership track (#1086) slice 5: dedicated coverage for
    /// `type_receiver`'s slow path specifically (the fix a PR review found
    /// the test above doesn't reach — `Widget { ` completion never calls
    /// `type_receiver` at all, it's pure syntax via `record_field_names`).
    /// `widget.bynk` is **never opened** — closed, on-disk only — so its
    /// content can only reach `w.`'s value-member completion in
    /// `use.bynk` through `type_receiver`'s own `sweep_project_content`
    /// call, not through any open-buffer overlay. No round runs before the
    /// request either, so `project_analysis_for`'s fast-path cache is empty
    /// and `type_receiver` must take its slow, re-analysing path — the one
    /// this slice fixed.
    #[tokio::test]
    async fn type_receivers_slow_path_sees_a_closed_files_disk_content() {
        let widget_src = "commons shared.widget\n\ntype Widget = { size: Int }\n";
        let use_src =
            "commons app.use\n\nuses shared.widget\n\nfn area(w: Widget) -> Int {\n  w.\n}\n";
        let s = scratch_project(
            "type_receiver_slow_path",
            &[
                (
                    "bynk.toml",
                    "[project]\nname = \"cross\"\n\n[paths]\ninclude = [\"shared\", \"app\"]\n",
                ),
                ("shared/widget.bynk", widget_src),
                ("app/use.bynk", use_src),
            ],
        );
        let backend = backend_at(&s.0).await;
        let use_uri = file_uri(&s.0, "app/use.bynk");
        // `widget.bynk` is deliberately never opened — no did_open, no round.
        open(&backend, &use_uri, use_src).await;

        let labels = complete_at(&backend, &use_uri, use_src, "  w.").await;
        assert!(
            labels.contains(&"size".to_string()),
            "type_receiver's slow path must resolve `w: Widget` off the \
             closed widget.bynk's real disk content: {labels:?}"
        );
    }
}
