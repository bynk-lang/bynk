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

use bynk_check::expr_types::type_at_offset;
use bynk_check::firstparty::Platform;
use bynk_check::locals::locals_at;
use bynk_emit::project::{
    AttributedError, BuildTarget, analyse_in_memory, analyse_in_memory_with_types,
    compile_in_memory,
};
use bynk_ide::completion;
use bynk_syntax::CompileError;

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
                let files = js
                    .artefacts
                    .docs
                    .into_iter()
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
    let line_prefix = source[..offset].rsplit('\n').next().unwrap_or("");
    let mut items: Vec<CompletionCandidate> = completion::complete(line_prefix, source, None)
        .into_iter()
        .map(to_candidate)
        .collect();

    // ADR 0093 D3: in-scope locals/params, alongside keywords/constructors at
    // a keyword or expression position — the same two disjoint positions
    // `bynk-lsp`'s handler merges locals into.
    if completion::is_keyword_position(line_prefix)
        || completion::is_expression_position(line_prefix)
    {
        let analysis = analyse_in_memory_with_types(source, BuildTarget::Bundle, platform);
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
        let analysis = analyse_in_memory_with_types(&rewritten, BuildTarget::Bundle, platform);
        if let Some(ty) = type_at_offset(&analysis.expr_types, recv_offset, &analysis.ty_intern) {
            items = completion::value_member_candidates(ty, &analysis.ty_intern, source, None)
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
}
