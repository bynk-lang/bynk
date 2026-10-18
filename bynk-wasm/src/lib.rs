//! The Bynk compiler as a wasm module for the in-browser REPL/playground (the
//! in-browser track, slice 3 — ADR 0139).
//!
//! One entry — `bynk_compile` (wasm) / `compile` (native) — takes an in-memory
//! Bynk source and returns a runnable **JavaScript module graph** plus diagnostics,
//! with **no filesystem and no `tsc`**:
//!
//! ```text
//! source ─▶ bynk_emit::compile_in_memory (Bundle / Browser)  ─▶ ProjectOutput (TS)
//!        ─▶ bynk_strip::strip_project_to_js                   ─▶ ProjectOutput (JS)
//!        ─▶ { files: [{ path, contents }], diagnostics }
//! ```
//!
//! The pipeline reuses the on-disk path wholesale (first-party injection, the
//! per-platform binding, the strip-only emitter), so the returned graph is the
//! complete set the browser links: the user module, `runtime.js`, the
//! `bynk-browser.js` binding, and `compose.js`. The crate compiles to `wasm32`
//! (the `cdylib`); the same logic is exercised natively (the `rlib`) by the
//! slice-3 tests, with the browser harness deferred to the REPL shell (slice 4).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bynk_check::checker::{TyId, Types};
use bynk_check::expr_types::type_at_offset;
use bynk_check::firstparty::Platform;
use bynk_check::locals::{LocalBinding, locals_at};
use bynk_emit::project::{
    AttributedError, BuildTarget, InMemoryAnalysis, analyse_in_memory, analyse_in_memory_project,
    analyse_in_memory_with_types, compile_in_memory, slash_key,
};
use bynk_ide::completion;
use bynk_syntax::CompileError;
use bynk_syntax::span::Span;

/// One emitted JavaScript module of the compiled program.
#[derive(serde::Serialize)]
pub struct EmittedFile {
    /// Output-relative path (e.g. `main.js`, `runtime.js`, `bynk-browser.js`).
    pub path: String,
    /// The JavaScript source.
    pub contents: String,
}

/// A diagnostic flattened for the JS side, with a 1-indexed line/column.
#[derive(serde::Serialize)]
pub struct Diagnostic {
    /// The source module the diagnostic belongs to, if attributable.
    pub path: Option<String>,
    pub line: usize,
    pub col: usize,
    /// Byte offsets of the diagnostic span (for the editor's inline lint range).
    pub from: usize,
    pub to: usize,
    /// `"error"` or `"warning"`.
    pub severity: String,
    /// The stable diagnostic category (e.g. `bynk.parse.expected_token`).
    pub category: String,
    pub message: String,
    /// Finding #47: previously dropped entirely for the playground. Plain
    /// text, not positioned — the CLI/LSP renderers already carry the
    /// harder problem of a label's span belonging to a different module
    /// than `path` (finding #46); this flattening keeps to text only rather
    /// than getting that wrong here too.
    pub notes: Vec<String>,
    pub labels: Vec<String>,
}

/// The outcome of compiling one in-memory source.
#[derive(serde::Serialize)]
pub struct CompileResult {
    /// Whether a runnable JavaScript graph was produced.
    pub ok: bool,
    /// The runnable JS module graph (empty on failure).
    pub files: Vec<EmittedFile>,
    /// Errors on failure, or non-failing warnings on success.
    pub diagnostics: Vec<Diagnostic>,
}

fn severity_str(err: &CompileError) -> &'static str {
    match bynk_syntax::Severity::for_error(err) {
        bynk_syntax::Severity::Error => "error",
        bynk_syntax::Severity::Warning => "warning",
    }
}

/// The human-readable message carried by a caught panic payload, if any.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

/// A synthetic `bynk.wasm.panic` diagnostic standing in for an internal compiler
/// panic, so an unexpected `panic!`/index-out-of-bounds/`unreachable!` in the
/// pipeline becomes a structured error rather than propagating past the boundary.
fn panic_diagnostic(payload: Box<dyn std::any::Any + Send>) -> Diagnostic {
    Diagnostic {
        path: None,
        line: 0,
        col: 0,
        from: 0,
        to: 0,
        severity: "error".to_string(),
        category: "bynk.wasm.panic".to_string(),
        message: format!("internal compiler panic: {}", panic_message(&*payload)),
        notes: Vec::new(),
        labels: Vec::new(),
    }
}

/// Run a pipeline entry point, converting an unexpected panic into a diagnostic.
///
/// On the native `rlib` path (the tests and any host embedding) this genuinely
/// unwinds the panic and returns `Err(diagnostic)`, so a reachable-in-principle
/// `panic!` no longer propagates past the wasm boundary. On the actual
/// `wasm32-unknown-unknown` target a panic still traps (`RuntimeError:
/// unreachable`) because the stock target lowers unwinding to a trap — there the
/// blast radius is bounded instead by the `console_error_panic_hook` (a legible
/// console error and location) set in the wasm entry points, and this wrapper
/// becomes effective for free if the playground build ever adopts wasm exception
/// handling. Fixing the underlying panic sites remains the real fix (#717).
///
/// `Diagnostic` grew past clippy's large-`Err` threshold once `notes`/`labels`
/// (finding #47) joined it; boxing it here would need every one of this
/// module's several `Diagnostic { .. }` construction sites and its `serde`
/// serialisation to route through a `Box` for one lint, so it's overridden
/// instead — this is a single-error return on the panic path, not a hot loop.
#[allow(clippy::result_large_err)]
fn catch_panic<T>(f: impl FnOnce() -> T) -> Result<T, Diagnostic> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(panic_diagnostic)
}

/// Flatten attributed errors to [`Diagnostic`]s, resolving line/col against the
/// owning source where known (`sources`), else the user source (`fallback`).
fn to_diagnostics(
    errs: Vec<AttributedError>,
    sources: &HashMap<PathBuf, String>,
    fallback: &str,
) -> Vec<Diagnostic> {
    errs.into_iter()
        .map(|a| {
            let src = a
                .source_path
                .as_ref()
                .and_then(|p| sources.get(p))
                .map(String::as_str)
                .unwrap_or(fallback);
            let (line, col) = bynk_syntax::span::line_col(src, a.error.span.start);
            Diagnostic {
                path: a
                    .source_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned()),
                line,
                col,
                from: a.error.span.start,
                to: a.error.span.end,
                severity: severity_str(&a.error).to_string(),
                category: a.error.category.to_string(),
                message: a.error.message.clone(),
                notes: a.error.notes.clone(),
                labels: a.error.labels.iter().map(|(_, msg)| msg.clone()).collect(),
            }
        })
        .collect()
}

/// Compile a single in-memory Bynk source to a JavaScript module graph for the
/// given platform (the playground passes [`Platform::Browser`]). Pure: no
/// filesystem, no `tsc`. The in-process `Bundle` subset only; programs that reach
/// Workers/Cloudflare-only shapes are reported as diagnostics (slice-2 platform
/// lock), never silently mis-compiled.
pub fn compile(source: &str, platform: Platform) -> CompileResult {
    catch_panic(|| compile_inner(source, platform)).unwrap_or_else(|d| CompileResult {
        ok: false,
        files: Vec::new(),
        diagnostics: vec![d],
    })
}

fn compile_inner(source: &str, platform: Platform) -> CompileResult {
    match compile_in_memory(source, BuildTarget::Bundle, platform) {
        Ok(out) => match bynk_strip::strip_project_to_js(out) {
            Ok(js) => {
                // The user program is the single in-memory source, so warnings
                // resolve their line/col against it (the fallback).
                let diagnostics = to_diagnostics(js.warnings, &HashMap::new(), source);
                // #2148: the graph is the modules the browser links, so the
                // output's `package.json` (the ES-module marker for Node and
                // `tsc`) is left out.
                let files = js
                    .artefacts
                    .docs
                    .into_iter()
                    .filter(|(_, doc)| !matches!(doc, bynk_emit::project::Document::Json(_)))
                    .map(|(path, doc)| EmittedFile {
                        path: path.to_string_lossy().into_owned(),
                        contents: doc.text(),
                    })
                    .collect();
                CompileResult {
                    ok: true,
                    files,
                    diagnostics,
                }
            }
            // The emitter is strip-only (ADR 0136), so this is unreachable for a
            // successful compile — surfaced as a diagnostic rather than a panic.
            Err(e) => CompileResult {
                ok: false,
                files: Vec::new(),
                diagnostics: vec![Diagnostic {
                    path: None,
                    line: 0,
                    col: 0,
                    from: 0,
                    to: 0,
                    severity: "error".to_string(),
                    category: "bynk.wasm.strip_failed".to_string(),
                    message: e.to_string(),
                    notes: Vec::new(),
                    labels: Vec::new(),
                }],
            },
        },
        Err(failure) => {
            let sources: HashMap<PathBuf, String> = failure.snapshots.iter().cloned().collect();
            CompileResult {
                ok: false,
                files: Vec::new(),
                diagnostics: to_diagnostics(failure.errors, &sources, source),
            }
        }
    }
}

/// Compile to a JSON string — the wasm boundary representation of [`CompileResult`].
pub fn compile_to_json(source: &str, platform: Platform) -> String {
    serde_json::to_string(&compile(source, platform)).unwrap_or_else(|e| {
        format!(
            "{{\"ok\":false,\"files\":[],\"diagnostics\":[{{\"path\":null,\"line\":0,\"col\":0,\"from\":0,\"to\":0,\
             \"severity\":\"error\",\"category\":\"bynk.wasm.serialize_failed\",\"message\":{:?}}}]}}",
            e.to_string()
        )
    })
}

/// The diagnostics of a single in-memory source — non-bailing analysis, no emission
/// (the editor's live, on-type diagnostics — slice 5d).
#[derive(serde::Serialize)]
pub struct AnalyzeResult {
    pub diagnostics: Vec<Diagnostic>,
}

/// Analyse a source for diagnostics only (no compile/emit), for the given platform.
pub fn analyze(source: &str, platform: Platform) -> AnalyzeResult {
    catch_panic(|| analyze_inner(source, platform)).unwrap_or_else(|d| AnalyzeResult {
        diagnostics: vec![d],
    })
}

fn analyze_inner(source: &str, platform: Platform) -> AnalyzeResult {
    let errs = analyse_in_memory(source, BuildTarget::Bundle, platform);
    AnalyzeResult {
        diagnostics: to_diagnostics(errs, &HashMap::new(), source),
    }
}

/// Analyse to a JSON string — `{ diagnostics: [{ from, to, line, col, severity,
/// category, message }] }`.
pub fn analyze_to_json(source: &str, platform: Platform) -> String {
    serde_json::to_string(&analyze(source, platform))
        .unwrap_or_else(|_| "{\"diagnostics\":[]}".to_string())
}

/// The inferred type at a cursor position in a single in-memory source, or
/// `None` if the expression at that position failed to type — per ADR
/// 0094, a well-typed function still contributes types even when a *different*
/// function in the same file has an error, so this isn't blanked by every
/// mid-edit error, only by one at the position itself (or upstream of it, an
/// unresolved name). The editor's hover tooltip (#397).
#[derive(serde::Serialize)]
pub struct HoverResult {
    pub ty: Option<String>,
}

/// Hover for a byte `offset` into `source`, for the given platform.
pub fn hover(source: &str, offset: usize, platform: Platform) -> HoverResult {
    catch_panic(|| hover_inner(source, offset, platform)).unwrap_or(HoverResult { ty: None })
}

fn hover_inner(source: &str, offset: usize, platform: Platform) -> HoverResult {
    let analysis = analyse_in_memory_with_types(source, BuildTarget::Bundle, platform);
    let ty = type_at_offset(&analysis.expr_types, offset, &analysis.ty_intern)
        .map(|t| t.display(&analysis.ty_intern));
    HoverResult { ty }
}

/// Hover to a JSON string — `{ ty: string | null }`.
pub fn hover_to_json(source: &str, offset: usize, platform: Platform) -> String {
    serde_json::to_string(&hover(source, offset, platform))
        .unwrap_or_else(|_| "{\"ty\":null}".to_string())
}

/// One completion candidate, serialised for the JS side — a shadow of
/// `bynk_ide::completion::Completion`/`CompletionKind` (that crate stays
/// serde-free; this is the wire DTO, same pattern as [`EmittedFile`]/[`Diagnostic`]).
#[derive(serde::Serialize)]
pub struct CompletionCandidate {
    pub label: String,
    /// "unit"/"capability"/"type"/"keyword"/"snippet"/"variant"/"member"/
    /// "field"/"constructor"/"function"/"local".
    pub kind: &'static str,
    pub detail: Option<String>,
    pub insert_text: Option<String>,
}

/// The editor's completion list at a cursor position (#808).
#[derive(serde::Serialize)]
pub struct CompleteResult {
    pub items: Vec<CompletionCandidate>,
}

fn to_candidate(c: completion::Completion) -> CompletionCandidate {
    use completion::CompletionKind::*;
    let kind = match c.kind {
        Unit => "unit",
        Capability => "capability",
        Type => "type",
        Keyword => "keyword",
        Snippet => "snippet",
        Variant => "variant",
        Member => "member",
        Field => "field",
        Constructor => "constructor",
        Function => "function",
        Constant => "constant",
    };
    CompletionCandidate {
        label: c.label,
        kind,
        detail: c.detail,
        insert_text: c.insert_text,
    }
}

/// Completion at a byte `offset` into an in-memory Bynk source, for the given
/// platform (capability methods, types, keywords, in-scope locals, and
/// value-receiver members — #808, the other half of #397 hover shipped).
/// Single buffer, single call — no project files, no multi-doc overlay/caching
/// (the wasm boundary has none of those, so `files: None` throughout).
pub fn complete(source: &str, offset: usize, platform: Platform) -> CompleteResult {
    catch_panic(|| complete_inner(source, offset, platform))
        .unwrap_or(CompleteResult { items: Vec::new() })
}

fn complete_inner(source: &str, offset: usize, platform: Platform) -> CompleteResult {
    complete_with(source, offset, None, |text| {
        FileTypes::from(analyse_in_memory_with_types(
            text,
            BuildTarget::Bundle,
            platform,
        ))
    })
}

/// One file's expression types and local bindings, from either the
/// single-source or the project analysis.
struct FileTypes {
    expr_types: Vec<(Span, TyId)>,
    ty_intern: Arc<Types>,
    locals: Vec<LocalBinding>,
}

impl From<InMemoryAnalysis> for FileTypes {
    fn from(a: InMemoryAnalysis) -> Self {
        FileTypes {
            expr_types: a.expr_types,
            ty_intern: a.ty_intern,
            locals: a.locals,
        }
    }
}

/// Completion over one buffer, with `analyse` typing that buffer (or a
/// rewrite of it) in whatever context the caller has: alone, or inside its
/// project (#2102 slice 2). `files` is the project's `.bynk` sources, for the
/// candidates that look across units (`consumes U { … }`). An offset past the
/// end or inside a character yields no candidates rather than a panic.
fn complete_with(
    source: &str,
    offset: usize,
    files: Option<&HashMap<PathBuf, String>>,
    analyse: impl Fn(&str) -> FileTypes,
) -> CompleteResult {
    if !source.is_char_boundary(offset) {
        return CompleteResult { items: Vec::new() };
    }
    let line_prefix = source[..offset].rsplit('\n').next().unwrap_or("");
    let mut items: Vec<CompletionCandidate> = completion::complete(line_prefix, source, files)
        .into_iter()
        .map(to_candidate)
        .collect();

    // ADR 0093 D3: in-scope locals/params, alongside keywords/constructors at
    // a keyword or expression position — the same two disjoint positions
    // `bynk-lsp`'s handler merges locals into.
    if completion::is_keyword_position(line_prefix)
        || completion::is_expression_position(line_prefix)
    {
        let analysis = analyse(source);
        items.extend(locals_at(&analysis.locals, offset).into_iter().map(|b| {
            CompletionCandidate {
                label: b.name.clone(),
                kind: "local",
                detail: Some(b.ty.clone()),
                insert_text: None,
            }
        }));
    }
    // A lowercase `receiver.` is a value receiver: `complete()` yields nothing
    // there directly (ADR 0093 D4), so retype the rewritten buffer (dropping
    // the trailing partial member) and offer the receiver's kernel methods /
    // record fields.
    if items.is_empty()
        && let Some((rewritten, recv_offset)) = completion::value_receiver_rewrite(source, offset)
    {
        let analysis = analyse(&rewritten);
        if let Some(ty) = type_at_offset(&analysis.expr_types, recv_offset, &analysis.ty_intern) {
            items = completion::value_member_candidates(ty, &analysis.ty_intern, source, files)
                .into_iter()
                .map(to_candidate)
                .collect();
        }
    }
    CompleteResult { items }
}

/// Complete to a JSON string — `{ items: [{ label, kind, detail, insert_text }] }`.
pub fn complete_to_json(source: &str, offset: usize, platform: Platform) -> String {
    serde_json::to_string(&complete(source, offset, platform))
        .unwrap_or_else(|_| "{\"items\":[]}".to_string())
}

// #2102 slice 2: the same three editor queries over a whole in-memory project,
// for the VS Code web extension, which loads the workspace through
// `vscode.workspace.fs`. A project is a map of project-relative paths to text
// (`bynk.toml` and every `.bynk` file; see
// `bynk_emit::project::analyse_in_memory_project`), sent as the JSON object
// `{ "<path>": "<text>", … }`.

/// Parse a project's `{ path: text }` JSON. Malformed JSON is an empty project.
pub fn project_files(files_json: &str) -> HashMap<PathBuf, String> {
    serde_json::from_str::<HashMap<String, String>>(files_json)
        .unwrap_or_default()
        .into_iter()
        .map(|(path, text)| (PathBuf::from(path), text))
        .collect()
}

/// Every file's diagnostics, each carrying its project-relative `path`. A
/// project-level diagnostic (a manifest problem, say) has `path: null`.
/// A project analysis: every file's diagnostics, plus `warnings` about the
/// project itself that aren't tied to a file (an ignored absolute `[paths]`
/// entry), for the caller to show.
#[derive(serde::Serialize)]
pub struct ProjectAnalyzeResult {
    pub diagnostics: Vec<Diagnostic>,
    pub warnings: Vec<String>,
}

pub fn analyze_project(
    files: &HashMap<PathBuf, String>,
    platform: Platform,
) -> ProjectAnalyzeResult {
    catch_panic(|| {
        let analysis = analyse_in_memory_project(files, BuildTarget::Bundle, platform);
        // The analysis reports `/`-separated paths; look sources up the same way.
        let sources: HashMap<PathBuf, String> = files
            .iter()
            .map(|(path, text)| (slash_key(path), text.clone()))
            .collect();
        ProjectAnalyzeResult {
            diagnostics: to_diagnostics(analysis.errors, &sources, ""),
            warnings: analysis
                .ignored_paths
                .into_iter()
                .map(|(key, entry)| {
                    format!(
                        "bynk.toml: `[paths] {key}` entry `{entry}` is an absolute path, which \
                         can't name a file in the project, so it is ignored (use a path \
                         relative to the project root)"
                    )
                })
                .collect(),
        }
    })
    .unwrap_or_else(|d| ProjectAnalyzeResult {
        diagnostics: vec![d],
        warnings: Vec::new(),
    })
}

/// The project's analysis, narrowed to one file's types and locals.
fn project_file_types(
    files: &HashMap<PathBuf, String>,
    path: &std::path::Path,
    platform: Platform,
) -> FileTypes {
    let mut analysis = analyse_in_memory_project(files, BuildTarget::Bundle, platform);
    FileTypes {
        expr_types: analysis.expr_types.remove(path).unwrap_or_default(),
        ty_intern: analysis.ty_intern,
        locals: analysis.locals.remove(path).unwrap_or_default(),
    }
}

/// Hover at a byte `offset` into the project file at `path`, typed in the
/// context of the whole project.
pub fn hover_project(
    files: &HashMap<PathBuf, String>,
    path: &str,
    offset: usize,
    platform: Platform,
) -> HoverResult {
    catch_panic(|| {
        let types = project_file_types(files, &slash_key(std::path::Path::new(path)), platform);
        HoverResult {
            ty: type_at_offset(&types.expr_types, offset, &types.ty_intern)
                .map(|t| t.display(&types.ty_intern)),
        }
    })
    .unwrap_or(HoverResult { ty: None })
}

/// Completion at a byte `offset` into the project file at `path`: the
/// single-source candidates, plus the ones that need the rest of the project
/// (names a `uses` brings in, another unit's capabilities).
pub fn complete_project(
    files: &HashMap<PathBuf, String>,
    path: &str,
    offset: usize,
    platform: Platform,
) -> CompleteResult {
    catch_panic(|| {
        // One spelling for every key, so the edited buffer below replaces its
        // file rather than sitting beside a differently separated copy.
        let files: &HashMap<PathBuf, String> = &files
            .iter()
            .map(|(p, text)| (slash_key(p), text.clone()))
            .collect();
        let path = slash_key(std::path::Path::new(path));
        let Some(source) = files.get(&path) else {
            return CompleteResult { items: Vec::new() };
        };
        let sources: HashMap<PathBuf, String> = files
            .iter()
            .filter(|(p, _)| p.extension().is_some_and(|e| e == "bynk"))
            .map(|(p, t)| (p.clone(), t.clone()))
            .collect();
        complete_with(source, offset, Some(&sources), |text| {
            let mut edited = files.clone();
            edited.insert(path.clone(), text.to_string());
            project_file_types(&edited, &path, platform)
        })
    })
    .unwrap_or(CompleteResult { items: Vec::new() })
}

/// The platform an editor entry point analyses for, from the optional name the JS
/// side passes (#2102). `None` keeps the playground's `browser` (its programs run
/// in the page). The VS Code web extension passes `"cloudflare"`, the platform
/// `bynk-lsp` analyses a project for (`Platform::default()`), so a unit that
/// consumes `bynk.cloudflare` doesn't draw `bynk.target.vendor_required` in
/// vscode.dev when it checks clean on the desktop. An unrecognised name falls
/// back to `browser`, the same as `None`.
pub fn platform_named(name: Option<&str>) -> Platform {
    match name {
        Some("cloudflare") => Platform::Cloudflare,
        Some("node") => Platform::Node,
        _ => Platform::Browser,
    }
}

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::prelude::wasm_bindgen;

/// Route panics to `console.error` with a readable message and location. Idempotent
/// (`set_once` installs the hook exactly once), so every entry point may call it.
/// Without this a panic on adversarial input surfaces as an opaque `RuntimeError:
/// unreachable` with no clue to its origin (#717).
#[cfg(target_arch = "wasm32")]
fn install_panic_hook() {
    console_error_panic_hook::set_once();
}

/// The wasm entry point for live editor diagnostics: analyse an in-memory Bynk
/// source for the browser and return `{ diagnostics: [...] }` (with byte `from`/`to`
/// spans for inline marking). Non-bailing — all diagnostics at once. `platform`
/// is optional (see [`platform_named`]); the playground omits it.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bynk_analyze(source: &str, platform: Option<String>) -> String {
    install_panic_hook();
    analyze_to_json(source, platform_named(platform.as_deref()))
}

/// The wasm entry point for the editor's hover tooltip: the inferred type at a
/// byte `offset` into an in-memory Bynk source, as `{ "ty": string | null }`
/// (#397). `platform` is optional (see [`platform_named`]).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bynk_hover(source: &str, offset: u32, platform: Option<String>) -> String {
    install_panic_hook();
    hover_to_json(source, offset as usize, platform_named(platform.as_deref()))
}

/// The wasm entry point for the editor's completion: context-aware candidates
/// at a byte `offset` into an in-memory Bynk source, as
/// `{ "items": [{ "label", "kind", "detail", "insert_text" }] }` (#808).
/// `platform` is optional (see [`platform_named`]).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bynk_complete(source: &str, offset: u32, platform: Option<String>) -> String {
    install_panic_hook();
    complete_to_json(source, offset as usize, platform_named(platform.as_deref()))
}

/// The wasm entry point for project diagnostics (#2102 slice 2): analyse a
/// whole in-memory project, given as `{ "<project-relative path>": "<text>" }`
/// JSON, and return `{ diagnostics: [...] }`, each with its file's `path`.
/// `platform` is optional (see [`platform_named`]).
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bynk_analyze_project(files_json: &str, platform: Option<String>) -> String {
    install_panic_hook();
    let result = analyze_project(
        &project_files(files_json),
        platform_named(platform.as_deref()),
    );
    serde_json::to_string(&result).unwrap_or_else(|_| "{\"diagnostics\":[]}".to_string())
}

/// The wasm entry point for project hover (#2102 slice 2): the type at a byte
/// `offset` into the project file at `path`, as `{ "ty": string | null }`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bynk_hover_project(
    files_json: &str,
    path: &str,
    offset: u32,
    platform: Option<String>,
) -> String {
    install_panic_hook();
    let result = hover_project(
        &project_files(files_json),
        path,
        offset as usize,
        platform_named(platform.as_deref()),
    );
    serde_json::to_string(&result).unwrap_or_else(|_| "{\"ty\":null}".to_string())
}

/// The wasm entry point for project completion (#2102 slice 2): candidates at a
/// byte `offset` into the project file at `path`, as `{ "items": [...] }`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bynk_complete_project(
    files_json: &str,
    path: &str,
    offset: u32,
    platform: Option<String>,
) -> String {
    install_panic_hook();
    let result = complete_project(
        &project_files(files_json),
        path,
        offset as usize,
        platform_named(platform.as_deref()),
    );
    serde_json::to_string(&result).unwrap_or_else(|_| "{\"items\":[]}".to_string())
}

/// The wasm entry point: compile an in-memory Bynk source for the browser
/// playground, returning a JSON document
/// `{ ok, files: [{ path, contents }], diagnostics: [{ path, line, col, severity,
/// category, message }] }`.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen]
pub fn bynk_compile(source: &str) -> String {
    install_panic_hook();
    compile_to_json(source, Platform::Browser)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const PROG: &str = "context app.demo\n\
        \n\
        consumes bynk { Clock, Logger }\n\
        \n\
        service demo {\n\
        \x20 on call() -> Effect[Instant] given Clock, Logger {\n\
        \x20   let _ <- Logger.info(\"hi\")\n\
        \x20   let now <- Clock.now()\n\
        \x20   now\n\
        \x20 }\n\
        }\n";

    #[test]
    fn compiles_browser_program_to_js_graph() {
        let r = compile(PROG, Platform::Browser);
        assert!(
            r.ok,
            "should compile: {:?}",
            r.diagnostics.first().map(|d| &d.message)
        );
        // The full runnable graph: user module + runtime + browser binding + compose.
        let paths: Vec<&str> = r.files.iter().map(|f| f.path.as_str()).collect();
        assert!(
            paths.iter().all(|p| p.ends_with(".js")),
            "all JS: {paths:?}"
        );
        assert!(
            paths.contains(&"runtime.js"),
            "runtime.js present: {paths:?}"
        );
        assert!(
            paths.contains(&"bynk-browser.js"),
            "browser binding present: {paths:?}"
        );
        // No residual TypeScript type syntax survived the strip.
        let user = r
            .files
            .iter()
            .find(|f| f.path == "app/demo.js")
            .expect("user module");
        assert!(
            !user.contents.contains(": Promise<"),
            "annotations stripped:\n{}",
            user.contents
        );
    }

    #[test]
    fn surfaces_diagnostics_for_a_bad_program() {
        let r = compile("context app.demo\n\nthis is not bynk\n", Platform::Browser);
        assert!(!r.ok);
        assert!(r.files.is_empty());
        assert!(!r.diagnostics.is_empty());
        assert!(r.diagnostics.iter().all(|d| d.severity == "error"));
        // Line/col point into the user source.
        assert!(r.diagnostics.iter().any(|d| d.line >= 1));
    }

    #[test]
    fn cloudflare_shapes_are_rejected_in_the_browser() {
        // The slice-2 platform lock fires through the in-memory path too.
        let prog = "context cache.store\n\
            \n\
            consumes bynk.cloudflare { Kv }\n\
            \n\
            service cache {\n\
            \x20 on call(k: String) -> Effect[Option[String]] given Kv {\n\
            \x20   let v <- Kv.get(k)\n\
            \x20   v\n\
            \x20 }\n\
            }\n";
        let r = compile(prog, Platform::Browser);
        assert!(
            !r.ok,
            "a cloudflare-only program must not compile for the browser"
        );
        assert!(
            r.diagnostics
                .iter()
                .any(|d| d.category == "bynk.target.vendor_required"),
            "expected the platform lock: {:?}",
            r.diagnostics
                .iter()
                .map(|d| &d.category)
                .collect::<Vec<_>>()
        );
    }

    /// #2102: the VS Code web extension analyses for `cloudflare`, as `bynk-lsp`
    /// does, so a unit consuming `bynk.cloudflare` checks clean there; the
    /// playground's default (`browser`) still reports the vendor lock.
    #[test]
    fn platform_named_selects_the_analysis_platform() {
        assert_eq!(platform_named(None), Platform::Browser);
        assert_eq!(platform_named(Some("cloudflare")), Platform::Cloudflare);
        assert_eq!(platform_named(Some("node")), Platform::Node);
        assert_eq!(platform_named(Some("browser")), Platform::Browser);
        assert_eq!(platform_named(Some("mainframe")), Platform::Browser);

        let prog = "context kv.index\n\n\
            consumes bynk.cloudflare { Kv }\n\n\
            service scan {\n\
            \x20 on call(prefix: String) -> Effect[Result[List[String], ()]] given Kv {\n\
            \x20   let keys <- Kv.list(Some(prefix))\n\
            \x20   Ok(keys)\n\
            \x20 }\n\
            }\n";
        let categories = |p: Platform| -> Vec<String> {
            analyze(prog, p)
                .diagnostics
                .into_iter()
                .map(|d| d.category)
                .collect()
        };
        assert_eq!(
            categories(platform_named(Some("cloudflare"))),
            Vec::<String>::new()
        );
        assert!(
            categories(platform_named(None)).contains(&"bynk.target.vendor_required".to_string()),
            "the browser default keeps the vendor lock: {:?}",
            categories(platform_named(None))
        );
    }

    #[test]
    fn compile_to_json_is_valid_json() {
        let json = compile_to_json(PROG, Platform::Browser);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["ok"], true);
        assert!(v["files"].as_array().is_some_and(|a| !a.is_empty()));
    }

    #[test]
    fn analyze_reports_check_errors_for_a_context() {
        // A type mismatch in a *context* — returning a String where Int is declared.
        // The non-bailing analyse must report it (slice 5d's reason to exist: plain
        // single-source `diagnose` only checks commons, not contexts).
        let prog = "context app.demo\n\n\
            consumes bynk { Logger }\n\n\
            service demo {\n\
            \x20 on call() -> Effect[Int] given Logger {\n\
            \x20   let _ <- Logger.info(\"x\")\n\
            \x20   \"not an int\"\n\
            \x20 }\n\
            }\n";
        let r = analyze(prog, Platform::Browser);
        assert!(
            r.diagnostics.iter().any(|d| d.severity == "error"),
            "a type mismatch should be reported: {:?}",
            r.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
        // A real diagnostic carries a span for inline marking.
        assert!(r.diagnostics.iter().any(|d| d.to > d.from));
    }

    #[test]
    fn hover_reports_the_inferred_type_of_an_expression() {
        // The tail expression `now` (the *reference*, not the `let now <-`
        // binding) — the last occurrence of the substring in `PROG`.
        let offset = PROG.rfind("now").expect("PROG mentions `now`");
        let r = hover(PROG, offset, Platform::Browser);
        assert_eq!(r.ty.as_deref(), Some("Instant"));
    }

    #[test]
    fn hover_outside_any_expression_is_none() {
        // Offset 0 sits in the `context` keyword — a declaration, not an
        // expression, so nothing is recorded there.
        let r = hover(PROG, 0, Platform::Browser);
        assert_eq!(r.ty, None);
    }

    #[test]
    fn hover_to_json_is_valid_json() {
        let offset = PROG.rfind("now").expect("PROG mentions `now`");
        let json = hover_to_json(PROG, offset, Platform::Browser);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["ty"], "Instant");
    }

    #[test]
    fn hover_survives_a_sibling_error() {
        // ADR 0094: hovering a well-typed expression must not go blank just
        // because a *different* function in the same buffer is mid-edit and
        // broken — the whole point of exposing the checker's partial
        // `expr_types` map rather than its old all-or-nothing gate.
        let prog = "commons app.demo\n\n\
            fn good() -> Int {\n  42\n}\n\n\
            fn bad() -> Int {\n  \"oops\"\n}\n";
        let offset = prog.find("42").expect("prog mentions 42");
        let r = hover(prog, offset, Platform::Browser);
        assert_eq!(r.ty.as_deref(), Some("Int"));
    }

    #[test]
    fn hover_on_an_expression_that_failed_to_type_is_none() {
        // The checker records `Ty::Error` for `"oops" + 1`; hover shows no
        // type rather than the `<type error>` placeholder.
        let prog = "commons app.demo\n\nfn bad() -> Int {\n  \"oops\" + 1\n}\n";
        let offset = prog.find("+ 1").expect("prog mentions + 1");
        let r = hover(prog, offset, Platform::Browser);
        assert_eq!(r.ty, None);
    }

    #[test]
    fn complete_offers_in_scope_capability_after_given() {
        let prog = "context app.demo\n\n\
            consumes bynk { Clock, Logger }\n\n\
            service demo {\n\
            \x20 on call() -> Effect[Instant] given \n\
            \x20   Clock.now()\n\
            \x20 }\n\
            }\n";
        let offset = prog.find("given \n").expect("prog mentions given") + "given ".len();
        let r = complete(prog, offset, Platform::Browser);
        assert!(
            r.items
                .iter()
                .any(|c| c.label == "Logger" && c.kind == "capability"),
            "{:?}",
            r.items.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn complete_offers_in_scope_locals_at_expression_position() {
        // ADR 0093 D3/D4: `bynk_complete` folds the two contexts that live
        // handler-side in `bynk-lsp` (locals, value-receiver members) into
        // the one wasm call — no analysis overlay/caching, single buffer.
        let offset = PROG.rfind("now").expect("PROG mentions `now`");
        let r = complete(PROG, offset, Platform::Browser);
        assert!(
            r.items.iter().any(|c| c.label == "now"
                && c.kind == "local"
                && c.detail.as_deref() == Some("Instant")),
            "{:?}",
            r.items
                .iter()
                .map(|c| (&c.label, c.kind))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn complete_offers_value_receiver_members_after_dot() {
        let prog = "commons app.demo\n\n\
            fn f() -> String {\n\
            \x20 let value = \"hi\"\n\
            \x20 value.\n\
            }\n";
        let offset = prog.find("value.\n").expect("prog mentions value.") + "value.".len();
        let r = complete(prog, offset, Platform::Browser);
        assert!(
            r.items
                .iter()
                .any(|c| c.label == "split" && c.kind == "member"),
            "{:?}",
            r.items.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
    }

    #[test]
    fn complete_survives_a_sibling_error() {
        // Same ADR 0094 ceiling as hover: a broken sibling function must not
        // blank out completion in a well-typed one.
        let prog = "commons app.demo\n\n\
            fn good() -> Int {\n  let count = 42\n  count\n}\n\n\
            fn bad() -> Int {\n  \"oops\"\n}\n";
        let offset = prog.rfind("count").expect("prog mentions count");
        let r = complete(prog, offset, Platform::Browser);
        assert!(
            r.items
                .iter()
                .any(|c| c.label == "count" && c.kind == "local"),
            "{:?}",
            r.items
                .iter()
                .map(|c| (&c.label, c.kind))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn complete_to_json_is_valid_json() {
        let offset = PROG.rfind("now").expect("PROG mentions `now`");
        let json = complete_to_json(PROG, offset, Platform::Browser);
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert!(v["items"].as_array().is_some_and(|a| !a.is_empty()));
    }

    #[test]
    fn complete_survives_a_panic() {
        // An out-of-bounds offset panics inside `complete_inner`'s slicing
        // (`source[..offset]`); `complete`'s own `catch_panic` wrapper must
        // still degrade to empty items rather than propagate, same guarantee
        // `catch_panic_converts_panic_to_a_diagnostic` proves for the wrapper
        // in general.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r = complete("", usize::MAX, Platform::Browser);
        std::panic::set_hook(prev);
        assert!(r.items.is_empty());
    }

    #[test]
    fn catch_panic_converts_panic_to_a_diagnostic() {
        // Silence the default hook's stderr backtrace for this deliberate panic,
        // then restore it so no other test is affected.
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let caught = catch_panic(|| -> i32 { panic!("boom {}", 42) });
        std::panic::set_hook(prev);

        let d = caught.expect_err("a panic must become an Err(diagnostic)");
        assert_eq!(d.severity, "error");
        assert_eq!(d.category, "bynk.wasm.panic");
        assert!(
            d.message.contains("boom 42"),
            "the panic message is carried through: {}",
            d.message
        );
    }

    #[test]
    fn catch_panic_passes_a_value_through() {
        assert_eq!(catch_panic(|| 7).ok(), Some(7));
    }

    #[test]
    fn analyze_clean_program_has_no_errors() {
        let r = analyze(PROG, Platform::Browser);
        assert!(
            r.diagnostics.iter().all(|d| d.severity != "error"),
            "clean program should have no errors: {:?}",
            r.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    // #2102 slice 2: project analysis for the VS Code web extension.

    const TEXT: &str = "commons app.text\n\n\
        fn greeting(subject: String) -> String {\n\
        \x20 \"Hello, \\(subject)!\"\n\
        }\n";

    const WEB: &str = "context app.web\n\n\
        uses app.text\n\n\
        consumes bynk { Logger }\n\n\
        service api {\n\
        \x20 on call() -> Effect[String] given Logger {\n\
        \x20   let _ <- Logger.info(\"hi\")\n\
        \x20   let message = greeting(\"World\")\n\
        \x20   message\n\
        \x20 }\n\
        }\n";

    fn project(web: &str) -> HashMap<PathBuf, String> {
        HashMap::from([
            (
                PathBuf::from("bynk.toml"),
                "[project]\nname = \"p\"\nversion = \"0.1.0\"\n".to_string(),
            ),
            (PathBuf::from("src/app/text.bynk"), TEXT.to_string()),
            (PathBuf::from("src/app/web.bynk"), web.to_string()),
        ])
    }

    fn categories(r: &ProjectAnalyzeResult) -> Vec<(Option<String>, String)> {
        r.diagnostics
            .iter()
            .map(|d| (d.path.clone(), d.category.clone()))
            .collect()
    }

    #[test]
    fn a_project_resolves_uses_across_files() {
        // Alone, the context can't see `app.text`.
        let alone = analyze(WEB, Platform::Cloudflare);
        assert!(
            alone
                .diagnostics
                .iter()
                .any(|d| d.category == "bynk.uses.unknown_commons")
        );
        // With its project, it checks clean.
        let r = analyze_project(&project(WEB), Platform::Cloudflare);
        assert!(r.diagnostics.is_empty(), "{:?}", categories(&r));
    }

    #[test]
    fn a_project_reports_a_cross_file_misuse_against_its_file() {
        let r = analyze_project(
            &project(&WEB.replace("greeting(\"World\")", "greeting(1)")),
            Platform::Cloudflare,
        );
        let cats = categories(&r);
        assert!(!cats.is_empty(), "the misuse is reported");
        assert!(
            cats.iter()
                .all(|(path, _)| path.as_deref() == Some("src/app/web.bynk")),
            "attributed to the using file: {cats:?}"
        );
    }

    #[test]
    fn a_project_honours_paths_include() {
        // With `include = ["lib"]`, files under `src/` aren't part of it.
        let mut files = project(WEB);
        files.insert(
            PathBuf::from("bynk.toml"),
            "[project]\nname = \"p\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"lib\"]\n"
                .to_string(),
        );
        let text = files.remove(Path::new("src/app/text.bynk")).unwrap();
        files.insert(PathBuf::from("lib/app/text.bynk"), text);
        let web = files.remove(Path::new("src/app/web.bynk")).unwrap();
        files.insert(PathBuf::from("lib/app/web.bynk"), web);
        files.insert(PathBuf::from("src/app/stray.bynk"), "not bynk".to_string());
        let r = analyze_project(&files, Platform::Cloudflare);
        assert!(r.diagnostics.is_empty(), "{:?}", categories(&r));
    }

    fn manifest(paths: &str) -> String {
        format!("[project]\nname = \"p\"\nversion = \"0.1.0\"\n\n[paths]\n{paths}\n")
    }

    #[test]
    fn a_project_honours_a_dot_slash_include() {
        // `./src` names the same tree as `src`. Were it left as written, no key
        // would start with it and every file would silently drop out, so the
        // cross-file misuse below would go unreported.
        let mut files = project(&WEB.replace("greeting(\"World\")", "greeting(1)"));
        files.insert(
            PathBuf::from("bynk.toml"),
            manifest("include = [\"./src\"]"),
        );
        let r = analyze_project(&files, Platform::Cloudflare);
        let cats = categories(&r);
        assert!(!cats.is_empty(), "the files under ./src are analysed");
        assert!(
            cats.iter()
                .all(|(path, _)| path.as_deref() == Some("src/app/web.bynk")),
            "{cats:?}"
        );
    }

    #[test]
    fn a_project_warns_about_absolute_paths_entries() {
        // An absolute include can't name a file in the project: it is ignored
        // and reported, and with no include left the conventional `src`
        // layout applies, so the project still checks.
        let mut files = project(WEB);
        files.insert(
            PathBuf::from("bynk.toml"),
            manifest("include = [\"/home/me/proj/src\"]\nexclude = [\"C:/gen\", \"gen\"]"),
        );
        let r = analyze_project(&files, Platform::Cloudflare);
        assert!(r.diagnostics.is_empty(), "{:?}", categories(&r));
        assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
        assert!(r.warnings[0].contains("`[paths] include` entry `/home/me/proj/src`"));
        assert!(r.warnings[1].contains("`[paths] exclude` entry `C:/gen`"));
        // Relative entries draw no warning.
        let r = analyze_project(&project(WEB), Platform::Cloudflare);
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn a_project_honours_a_dot_slash_exclude_and_a_root_include() {
        // A flat layout (`include = ["./"]`, the root) with `exclude = ["./gen"]`:
        // the broken file under gen/ is left out, the rest checks clean.
        let mut files: HashMap<PathBuf, String> = project(WEB)
            .into_iter()
            .map(|(p, t)| (PathBuf::from(p.to_string_lossy().replace("src/", "")), t))
            .collect();
        files.insert(
            PathBuf::from("bynk.toml"),
            manifest("include = [\"./\"]\nexclude = [\"./gen\"]"),
        );
        files.insert(PathBuf::from("gen/junk.bynk"), "not bynk".to_string());
        let r = analyze_project(&files, Platform::Cloudflare);
        assert!(r.diagnostics.is_empty(), "{:?}", categories(&r));
        // Without the exclude, gen/junk.bynk is analysed and fails to parse.
        files.insert(PathBuf::from("bynk.toml"), manifest("include = [\".\"]"));
        let r = analyze_project(&files, Platform::Cloudflare);
        assert!(
            categories(&r)
                .iter()
                .any(|(path, _)| path.as_deref() == Some("gen/junk.bynk")),
            "{:?}",
            categories(&r)
        );
    }

    #[test]
    fn a_project_hovers_a_name_from_another_file() {
        let files = project(WEB);
        let offset = WEB.rfind("message").expect("WEB mentions `message`");
        let r = hover_project(&files, "src/app/web.bynk", offset, Platform::Cloudflare);
        assert_eq!(r.ty.as_deref(), Some("String"));
        // An unknown path hovers nothing.
        let r = hover_project(&files, "src/app/nope.bynk", offset, Platform::Cloudflare);
        assert_eq!(r.ty, None);
    }

    #[test]
    fn a_project_completes_locals_typed_across_files() {
        let src = WEB.replace("    message\n", "    m\n");
        let files = project(&src);
        let offset = src.rfind("    m\n").expect("the partial local") + "    m".len();
        let r = complete_project(&files, "src/app/web.bynk", offset, Platform::Cloudflare);
        let message = r
            .items
            .iter()
            .find(|c| c.label == "message")
            .unwrap_or_else(|| panic!("`message` is offered: {:?}", labels(&r)));
        assert_eq!(message.detail.as_deref(), Some("String"));
        // An offset inside a multi-byte character offers nothing, not a panic.
        let files = project(&WEB.replace("hi", "hé"));
        let inside = WEB.find("hi").unwrap() + 2;
        let r = complete_project(&files, "src/app/web.bynk", inside, Platform::Cloudflare);
        assert!(r.items.is_empty());
    }

    fn labels(r: &CompleteResult) -> Vec<&str> {
        r.items.iter().map(|c| c.label.as_str()).collect()
    }

    /// Windows CI caught `src\app/web.bynk`: the project model joins identity
    /// paths with the platform separator. Every path a project analysis hands
    /// back, and every key it takes, is `/`-separated on every platform.
    #[test]
    fn project_paths_are_slash_separated_on_every_platform() {
        let misuse = WEB.replace("greeting(\"World\")", "greeting(1)");
        // Keys spelled with the platform's own joins (`\` on Windows).
        let native: HashMap<PathBuf, String> = project(&misuse)
            .into_iter()
            .map(|(p, text)| {
                let joined = p.iter().fold(PathBuf::new(), |acc, part| acc.join(part));
                (joined, text)
            })
            .collect();
        for files in [project(&misuse), native] {
            let r = analyze_project(&files, Platform::Cloudflare);
            assert!(!r.diagnostics.is_empty(), "the misuse is reported");
            for d in &r.diagnostics {
                let path = d.path.as_deref().unwrap_or_default();
                assert!(!path.contains('\\'), "a `/` path, not {path:?}");
                assert_eq!(path, "src/app/web.bynk");
                // Resolved against the file's own text, so a real line.
                assert!(d.line > 1, "line/col from web.bynk: {}", d.line);
            }
            // The `"hi"` literal still types next to the misuse.
            let offset = misuse.find("\"hi\"").unwrap() + 1;
            let r = hover_project(&files, "src/app/web.bynk", offset, Platform::Cloudflare);
            assert_eq!(
                r.ty.as_deref(),
                Some("String"),
                "hover finds the file under its `/` key"
            );
        }
    }

    #[test]
    fn slash_key_joins_components_with_slashes() {
        let joined = Path::new("src").join("app").join("web.bynk");
        assert_eq!(slash_key(&joined), PathBuf::from("src/app/web.bynk"));
        assert_eq!(
            slash_key(Path::new("./src/a.bynk")),
            PathBuf::from("src/a.bynk")
        );
        #[cfg(windows)]
        assert_eq!(
            slash_key(Path::new(r"src\app/web.bynk")),
            PathBuf::from("src/app/web.bynk")
        );
        // On Unix a `\` is part of a file name, not a separator.
        #[cfg(unix)]
        assert_eq!(
            slash_key(Path::new(r"src/a\b.bynk")),
            PathBuf::from(r"src/a\b.bynk")
        );
    }

    #[test]
    fn project_files_parses_the_wire_map() {
        let files = project_files(r#"{"bynk.toml":"x","src/a.bynk":"y"}"#);
        assert_eq!(
            files.get(Path::new("src/a.bynk")).map(String::as_str),
            Some("y")
        );
        assert!(project_files("not json").is_empty());
    }
}
