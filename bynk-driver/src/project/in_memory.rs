use super::diagnostics::Mode;
use super::diagnostics::ProjectFailure;
use super::output::finish_build;
use super::pipeline::RunChecks;
use super::pipeline::run_checks;
use bynk_check::project_model::BuildTarget;
use bynk_emit::emit_model::ImportExt;
use bynk_emit::emit_model::ProjectOutput;
use bynk_emit::emit_model::RemoteTests;
use bynk_project::AttributedError;
use bynk_project::LabelFile;
use bynk_project::SchemaLock;
use bynk_project::assets;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use bynk_check::checker::{TyId, Types};
use bynk_check::firstparty::Platform;
use bynk_syntax::lexer;
use bynk_syntax::parser;

/// Compile a single **in-memory** Bynk source through the full project pipeline —
/// no filesystem access (in-browser track, slice 3). The source is the in-process
/// `Bundle` subset that `consumes bynk`; first-party injection and the per-platform
/// binding emission run exactly as for an on-disk build, so the returned
/// [`ProjectOutput`] is the complete module graph (the user unit + `runtime.ts` +
/// the `bynk-<platform>.ts` binding + `compose.ts`). The wasm entry point pairs
/// this with `bynk-strip` to produce JavaScript for the playground.
///
/// The module's logical path is **derived from its declared unit name** (a context
/// `app.demo` ⇒ `app/demo.bynk`), so the name↔path alignment check passes without
/// real files; a source that does not parse falls back to `main.bynk` and the parse
/// error is reported normally.
pub fn compile_in_memory(
    source: &str,
    target: BuildTarget,
    platform: Platform,
) -> Result<ProjectOutput, ProjectFailure> {
    // T3.6b (R4.1): one table for this virtual project's compile.
    let tys = &Arc::new(Types::new());
    // A single-tree (`src_root == tests_root`) virtual project rooted at `.`: the
    // one source file is supplied directly and its text layered in via the
    // overlay, so discovery and every other disk read are bypassed.
    let root = PathBuf::from(".");
    let path = in_memory_logical_path(source);
    let mut overlay = HashMap::new();
    overlay.insert(path.clone(), source.to_string());
    let trees = vec![(root.clone(), PathBuf::new())];
    let run = run_checks(
        &trees,
        target,
        platform,
        ImportExt::Js,
        Mode::Build,
        &overlay,
        &[],
        Some(vec![vec![path]]),
        false,
        false,
        false,
        false,
        true,
        &SchemaLock::Off,
        None,
        &root,
        None,
        None,
        None,
        None,
        None,
        false,
        tys,
        None,
    );
    finish_build(
        run,
        ImportExt::Js,
        Path::new(""),
        &[],
        Default::default(),
        None,
        None,
    )
}

/// [`compile_in_memory`] for several files (#1830): each `(path, source)` pair
/// is one project file, its path relative to a single-tree root exactly as on
/// disk (`demo/shop.bynk` for `context demo.shop`, `tests/shop.bynk` for a
/// suite). Test-only, so a unit test can build a multi-unit project, or a
/// unit with its suite, without a fixture directory; reach it through
/// `crate::testkit`. `tests` keeps the suites, as `bynk test` does; off is
/// the deployable build `bynk compile` makes.
#[doc(hidden)] // test support: `bynk-emit`'s tests drive the pipeline through it
pub fn compile_files_in_memory(
    files: &[(&str, &str)],
    target: BuildTarget,
    platform: Platform,
    tests: bool,
) -> Result<ProjectOutput, ProjectFailure> {
    compile_files_in_memory_remote(files, target, platform, tests, None, None)
}

/// [`compile_files_in_memory`] as a remote test build (#2138,
/// [`CompileOptions::remote_tests`]).
#[doc(hidden)] // test support: `bynk-emit`'s tests drive the pipeline through it
pub fn compile_files_in_memory_remote(
    files: &[(&str, &str)],
    target: BuildTarget,
    platform: Platform,
    tests: bool,
    remote: Option<&RemoteTests>,
    asset_files: Option<&assets::AssetFolder>,
) -> Result<ProjectOutput, ProjectFailure> {
    compile_files_in_memory_locked(files, target, platform, tests, remote, asset_files, None)
}

/// [`compile_files_in_memory_remote`] with a pre-read `bynk.contracts.lock`
/// (#2554, [`CompileOptions::contracts_lock`]).
#[doc(hidden)] // test support: `bynk-emit`'s tests drive the pipeline through it
pub fn compile_files_in_memory_locked(
    files: &[(&str, &str)],
    target: BuildTarget,
    platform: Platform,
    tests: bool,
    remote: Option<&RemoteTests>,
    asset_files: Option<&assets::AssetFolder>,
    contracts_lock: Option<&str>,
) -> Result<ProjectOutput, ProjectFailure> {
    let tys = &Arc::new(Types::new());
    let root = PathBuf::from(".");
    let paths: Vec<PathBuf> = files.iter().map(|(p, _)| PathBuf::from(p)).collect();
    let overlay: HashMap<PathBuf, String> = files
        .iter()
        .map(|(p, src)| (PathBuf::from(p), (*src).to_string()))
        .collect();
    let trees = vec![(root.clone(), PathBuf::new())];
    let run = run_checks(
        &trees,
        target,
        platform,
        ImportExt::Js,
        Mode::Build,
        &overlay,
        &[],
        Some(vec![paths]),
        false,
        false,
        false,
        false,
        tests,
        &SchemaLock::Off,
        remote,
        &root,
        asset_files,
        None,
        None,
        None,
        None,
        false,
        tys,
        None,
    );
    finish_build(
        run,
        ImportExt::Js,
        Path::new(""),
        &[],
        Default::default(),
        None,
        contracts_lock,
    )
}

/// Analyse a single **in-memory** Bynk source and return all diagnostics —
/// non-bailing, no emission (in-browser track, slice 5d). The editor calls this
/// on every (debounced) keystroke for live diagnostics: unlike [`compile_in_memory`]
/// (build mode, which bails at the first failing phase), this runs in `Analyse`
/// mode, so parse / resolve / check diagnostics are recovered and reported together
/// — and it works for a `context` (the playground's typical program), not only a
/// commons. Same fs-free seam as `compile_in_memory`.
pub fn analyse_in_memory(
    source: &str,
    target: BuildTarget,
    platform: Platform,
) -> Vec<AttributedError> {
    analyse_in_memory_with_types(source, target, platform).errors
}

/// The outcome of [`analyse_in_memory_with_types`]: diagnostics plus the
/// analysed file's `(span, type)` entries (span-sorted — see
/// [`ExprTypeSink::take_files`](bynk_check::expr_types::ExprTypeSink::take_files)), for a position→type query (#397, the
/// playground's hover), and its local bindings (#808, the playground's
/// completion — `bynk_check::locals::locals_at` over `locals` answers
/// "what's in scope at this offset").
pub struct InMemoryAnalysis {
    pub errors: Vec<AttributedError>,
    pub expr_types: Vec<(bynk_syntax::span::Span, TyId)>,
    /// T3.6b (R4.1): the table `expr_types`' ids resolve against.
    pub ty_intern: std::sync::Arc<bynk_check::checker::Types>,
    pub locals: Vec<bynk_check::locals::LocalBinding>,
}

/// Like [`analyse_in_memory`], but also exposes the expression-type map the
/// checker captured (ADR 0063's `expr_types` sink) — the same one
/// `bynk_check::analysis::analyse_project` drains — instead of discarding it. Per ADR 0094,
/// this is a best-effort **partial** map in `Analyse` mode: a function that
/// type-checked cleanly contributes its types even if a *different* function
/// in the same file has an error, so `expr_types` is empty only when the
/// expression at hand never typed at all (e.g. it sits in an unresolved
/// region, ADR 0094's "out of scope — the resolve gate"), not merely because
/// the file has some error somewhere.
pub fn analyse_in_memory_with_types(
    source: &str,
    target: BuildTarget,
    platform: Platform,
) -> InMemoryAnalysis {
    // T3.6b (R4.1): one table for the whole analysis — every unit it checks
    // interns into this, so the `TyId`s it hands back on `InMemoryAnalysis`
    // all resolve against the one table it also hands back.
    let tys = &Arc::new(Types::new());
    let root = PathBuf::from(".");
    let path = in_memory_logical_path(source);
    let mut overlay = HashMap::new();
    overlay.insert(path.clone(), source.to_string());
    let trees = vec![(root.clone(), PathBuf::new())];
    let run = run_checks(
        &trees,
        target,
        platform,
        ImportExt::Js,
        Mode::Analyse,
        &overlay,
        &[],
        Some(vec![vec![path.clone()]]),
        false,
        false,
        false,
        false,
        true,
        &SchemaLock::Off,
        None,
        &root,
        None,
        None,
        None,
        None,
        None,
        false,
        tys,
        None,
    );
    match run {
        RunChecks::Bailed {
            errors,
            mut exprs,
            mut locals,
            ..
        }
        | RunChecks::Checked {
            errors,
            mut exprs,
            mut locals,
            ..
        } => InMemoryAnalysis {
            errors: errors.into_all(),
            expr_types: exprs.take_files().remove(&path).unwrap_or_default(),
            ty_intern: Arc::clone(tys),
            locals: locals.take_files().remove(&path).unwrap_or_default(),
        },
    }
}

/// The outcome of [`analyse_in_memory_project`]: every file's diagnostics,
/// plus each `.bynk` file's expression types and local bindings keyed by its
/// project-relative path, for a position→type query in any of them.
pub struct InMemoryProjectAnalysis {
    pub errors: Vec<AttributedError>,
    pub expr_types: HashMap<PathBuf, Vec<(bynk_syntax::span::Span, TyId)>>,
    /// The table every file's `expr_types` ids resolve against.
    pub ty_intern: std::sync::Arc<bynk_check::checker::Types>,
    pub locals: HashMap<PathBuf, Vec<bynk_check::locals::LocalBinding>>,
    /// `bynk.toml` `[paths]` entries that were ignored, as `(key, entry)`
    /// (`("include", "/abs/src")`): an absolute `include` or `exclude` can't
    /// name anything in an in-memory project, so rather than silently matching
    /// nothing it is reported here for the caller to surface. Plain data, not a
    /// message: wording is the caller's.
    pub ignored_paths: Vec<(String, String)>,
}

/// #2102 slice 2: analyse a whole project held in memory, for the VS Code web
/// extension, which reads the workspace through `vscode.workspace.fs` and has
/// no filesystem to hand `run_checks`.
///
/// `files` maps **project-relative** paths to their text: `bynk.toml` (when the
/// project has one) and every `.bynk` file, at the paths they have on disk
/// (`src/app/web.bynk`). The project root is the empty path, so every path
/// stays relative, in the diagnostics too. `[paths] include`/`exclude` come
/// from the in-memory `bynk.toml`. With no `include`, the conventional layout
/// is decided from the files themselves (`src`, `tests`, else the root) rather
/// than [`ProjectPaths::conventional`](bynk_project::ProjectPaths::conventional)'s disk probe. Non-`.bynk` entries other
/// than `bynk.toml` are ignored. Pure: no filesystem.
pub fn analyse_in_memory_project(
    files: &HashMap<PathBuf, String>,
    target: BuildTarget,
    platform: Platform,
) -> InMemoryProjectAnalysis {
    let tys = &Arc::new(Types::new());
    let project_root = PathBuf::new();
    // Keys are `/`-separated whatever the caller's platform spelling.
    let files: &HashMap<PathBuf, String> = &files
        .iter()
        .map(|(path, text)| (slash_key(path), text.clone()))
        .collect();
    let (include, exclude, ignored_paths) = in_memory_project_paths(files);

    // One tree per distinct include, with its prefix (`.` is the empty one —
    // see `Roots::trees`). Built here rather than through `Roots` so a `.`
    // root stays the empty path, which every relative key starts with.
    let mut trees: Vec<(PathBuf, PathBuf)> = Vec::new();
    for root in include {
        if !trees.iter().any(|(r, _)| *r == root) {
            trees.push((root.clone(), root));
        }
    }
    let mut excludes: Vec<PathBuf> = exclude;
    excludes.extend(["out", "node_modules"].map(PathBuf::from));

    let mut buckets: Vec<Vec<PathBuf>> = vec![Vec::new(); trees.len()];
    let mut keys: Vec<&PathBuf> = files
        .keys()
        .filter(|p| p.extension().is_some_and(|e| e == "bynk"))
        .filter(|p| !excludes.iter().any(|ex| p.starts_with(ex)))
        .collect();
    keys.sort();
    for p in keys {
        if let Some(idx) = trees.iter().position(|(root, _)| p.starts_with(root)) {
            buckets[idx].push(p.clone());
        }
    }

    let run = run_checks(
        &trees,
        target,
        platform,
        ImportExt::Js,
        Mode::Analyse,
        files,
        &[],
        Some(buckets),
        false,
        false,
        false,
        false,
        true,
        &SchemaLock::Off,
        None,
        &project_root,
        None,
        None,
        None,
        None,
        None,
        false,
        tys,
        None,
    );
    match run {
        RunChecks::Bailed {
            errors,
            mut exprs,
            mut locals,
            ..
        }
        | RunChecks::Checked {
            errors,
            mut exprs,
            mut locals,
            ..
        } => InMemoryProjectAnalysis {
            errors: errors
                .into_all()
                .into_iter()
                .map(|mut e| {
                    e.source_path = e.source_path.as_deref().map(slash_key);
                    for label in &mut e.label_files {
                        if let LabelFile::Other(p) = label {
                            *p = slash_key(p);
                        }
                    }
                    e
                })
                .collect(),
            expr_types: exprs
                .take_files()
                .into_iter()
                .map(|(p, v)| (slash_key(&p), v))
                .collect(),
            ty_intern: Arc::clone(tys),
            locals: locals
                .take_files()
                .into_iter()
                .map(|(p, v)| (slash_key(&p), v))
                .collect(),
            ignored_paths,
        },
    }
}

/// A path as an in-memory project key: its components joined with `/`.
///
/// [`analyse_in_memory_project`]'s keys are `/`-separated on every platform
/// (they come from the web extension, which speaks URIs). The project model
/// builds identity paths with `PathBuf::join`, which inserts `\` on Windows
/// (`src` + `app/web.bynk` is `src\app/web.bynk` there), so every path
/// handed back is rebuilt this way, and keys taken in are too. On Unix a `\`
/// is an ordinary file-name character, not a separator, and is kept.
pub fn slash_key(path: &Path) -> PathBuf {
    let parts: Vec<String> = path
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    PathBuf::from(parts.join("/"))
}

/// The `[paths]` `include` and `exclude` lists of an in-memory project (see
/// [`analyse_in_memory_project`]). An absent or unreadable `bynk.toml`, or one
/// with no `include`, gets the conventional layout, judged by which of `src/`
/// and `tests/` hold a `.bynk` file. Entries are normalised with [`slash_key`],
/// so `./src` is `src` and the root (`.`) is the empty path. An absolute entry
/// is dropped and described in the third list (an `include` left empty by that
/// falls back to the conventional layout).
type IgnoredPaths = Vec<(String, String)>;

fn in_memory_project_paths(
    files: &HashMap<PathBuf, String>,
) -> (Vec<PathBuf>, Vec<PathBuf>, IgnoredPaths) {
    let doc = files
        .get(Path::new("bynk.toml"))
        .and_then(|text| text.parse::<toml::Table>().ok());
    let paths = doc
        .as_ref()
        .and_then(|d| d.get("paths"))
        .and_then(|v| v.as_table());
    // Each entry is normalised like the file keys (`./src` is `src`), so it
    // prefixes them. The project root itself (`.`, `./`) normalises to the
    // empty path, which prefixes every key.
    let mut ignored = Vec::new();
    let mut list = |key: &str| -> Vec<PathBuf> {
        let entries: Vec<&str> = match paths.and_then(|t| t.get(key)) {
            Some(toml::Value::Array(items)) => items.iter().filter_map(|v| v.as_str()).collect(),
            Some(toml::Value::String(s)) => vec![s.as_str()],
            _ => Vec::new(),
        };
        let mut kept = Vec::new();
        for entry in entries {
            if is_absolute_entry(entry) {
                ignored.push((key.to_string(), entry.to_string()));
            } else {
                kept.push(slash_key(Path::new(entry)));
            }
        }
        kept
    };
    let mut include = list("include");
    let exclude = list("exclude");
    if include.is_empty() {
        let has = |dir: &str| {
            files
                .keys()
                .any(|p| p.starts_with(dir) && p.extension().is_some_and(|e| e == "bynk"))
        };
        include = ["src", "tests"]
            .into_iter()
            .filter(|d| has(d))
            .map(PathBuf::from)
            .collect();
        if include.is_empty() {
            include.push(PathBuf::new());
        }
    }
    (include, exclude, ignored)
}

/// Whether a `[paths]` entry is absolute on any platform: rooted (`/src`,
/// `\\src`) or with a drive (`C:/src`), whatever the host's own path rules.
fn is_absolute_entry(entry: &str) -> bool {
    let bytes = entry.as_bytes();
    entry.starts_with('/')
        || entry.starts_with('\\')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || Path::new(entry).has_root()
}

/// Derive the conventional single-file path for an in-memory source from its
/// declared unit name (`app.demo` ⇒ `app/demo.bynk`), so `check_path_name_alignment`
/// is satisfied without a real file tree. Falls back to `main.bynk` when the source
/// does not parse — `run_checks` then re-parses and reports the error against it.
fn in_memory_logical_path(source: &str) -> PathBuf {
    let parts: Option<Vec<String>> = lexer::tokenize(source)
        .ok()
        .and_then(|tokens| parser::parse_unit(&tokens, source).ok())
        .map(|unit| unit.name().parts.iter().map(|i| i.name.clone()).collect());
    match parts {
        Some(p) if !p.is_empty() => {
            let mut path = PathBuf::from(p.join("/"));
            path.set_extension("bynk");
            path
        }
        _ => PathBuf::from("main.bynk"),
    }
}

// -- internals --

#[allow(dead_code)]
fn _ensure_components_used(_p: &Path) {
    let _ = Component::CurDir;
}
