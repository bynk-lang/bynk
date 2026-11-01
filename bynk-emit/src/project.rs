//! Multi-file project compilation (v0.3 §3.2 and §3.3, v0.4 §3.5).
//!
//! A "project" is a directory tree of `.bynk` source files. The dotted name
//! of a commons or context (e.g., `bynk.time`, `commerce.orders`) maps to a
//! path under the project root — either a single file (`bynk/time.bynk`) or
//! a directory of files all sharing the same header (`bynk/time/*.bynk`).
//!
//! v0.4: each file is one of two kinds — commons or context. Both kinds share
//! the same multi-file directory machinery; they differ in body content
//! (contexts have `consumes`/`exports`, types are nominally per-context), in
//! visibility (contexts export only the types listed), and in TypeScript
//! emission (contexts re-brand types from used commons).
//!
//! Compilation proceeds in two passes:
//!   1. **Discover and parse** every `.bynk` file. Group by qualified name
//!      and kind. Build a global symbol table where each unit contributes
//!      its declarations.
//!   2. **Resolve, type-check, and emit** each unit with full visibility of
//!      the units it transitively `uses` or `consumes`. Two passes keep
//!      `uses` cycles trivial — there is no order-of-evaluation, only
//!      declarative mixin.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::emitter;
use bynk_check::actors::ActorDecl;
use bynk_check::check_pipeline::{self, prepare_unit_check_ctx};
use bynk_check::checker;
use bynk_check::checker::{ExprId, TyId, Types};
use bynk_check::expr_types::ExprTypeSink;
use bynk_check::firstparty::{self, Platform};
use bynk_check::hints::HintSink;
use bynk_check::index::RefSink;
use bynk_check::locals::LocalsSink;
use bynk_check::project_model::{
    self, AdapterBinding, FnDecl, TypeDecl, UnitInfo, Visibility, handler_cross_caps,
    resolve_consume_prefix,
};
use bynk_check::requirements::RequirementSink;
use bynk_check::resolver::{self, MethodTable as ResolverMethodTable, ResolvedCommons};
use bynk_ir::CapRefIr;
use bynk_ir::EventSubscriberShape;
use bynk_ir::FnSig;
use bynk_lower::{
    lower_attached_fn_sig_ir_from_types, lower_handler_given_ir, lower_provider_given_ir,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::lexer;
use bynk_syntax::parser;
use bynk_ts::{TsBindingName, TsDecl, TsExpr, TsLit, TsParam, TsProgram, TsStmt, TsType};

// P4.0 (#1113): discovery, the unit graph, path resolution, and cross-unit
// consistency checks moved to `bynk-project` — this crate is now a
// dependent, not an owner, of that code (`design/tracks/project-model.md`
// §6 row P4.0). P4.1 (#1115): `symbols` and the per-file `context_checks`
// subset of `validate` moved to `bynk-check` for the same reason — this
// crate reaches them as `bynk_check::symbols`/`bynk_check::context_checks`
// now. P5.0-P5.3 (`design/tracks/semantics-in-the-checker.md` §6) emptied
// `validate` out the same way, project-wide check by project-wide check,
// including the reconciliation half of `schema_registry` (now
// `bynk_check::schema_registry`). P5.4 moved `tests_emit`'s checking half
// (target/participant resolution, `stub` resolution, case/property body
// type-checking) to `bynk_check::test_suites` the same way — `tests_emit`
// stays here as a caller of it, holding only TypeScript emission plus the
// two functions' unchanged public shape. P5.5 (§5, §6): `validate` reached
// empty (its last occupant, `check_platform_lock`, left at P5.3) and its
// remaining diagnostic-emitting sites — the `bynk.secrets.computed_name`
// warning and the `bynk.project.schema_registry_corrupt` construction, both
// outside the seven-category accounting — relocated too (§3.2's "eighth
// site" and §9's open risk respectively); `validate.rs` and its module
// declaration are deleted, this track's own completion criterion (§5).
// Pipeline-driving types (`diagnostics`'s `Mode`/`ErrorSink`/
// `ProjectAnalysis`/`ProjectFailure`) stay here too.
mod diagnostics;
mod schema_registry;
mod tests_emit;
mod worker_bindings;

use bynk_check::symbols::*;
use bynk_project::discovery::*;
use bynk_project::paths::*;
use diagnostics::*;
use tests_emit::*;

// External facade: items referenced as `crate::project::X` from outside this
// module (emitter, main, lib) must stay reachable at that path.
pub use bynk_check::project_model::BuildTarget;
pub use bynk_check::schema_registry::{AgentVersions, MigrationStep};
pub use bynk_check::symbols::{FileDeclIndex, UnitTable};
// #1883, slice 1b: the `assets/` folder helpers, for the driver and testkit.
pub use bynk_project::assets;
pub use bynk_project::{
    AttributedError, LabelFile, ProjectPaths, ProjectPathsError, Roots, SchemaLock, TestTarget,
    UnitKind, WorkersConfig, Workspace, check_manifest, check_manifest_str, dependency_trees,
    discover_project_files, is_workspace_root, load_workspace, try_read_project_paths,
    try_read_project_paths_with, try_read_test_targets, try_read_workers_config,
    try_read_workers_config_with, worker_dir_name, worker_handlers_output_path,
    worker_handlers_source_path,
};
pub use diagnostics::{ContextBoundaryInfo, ContextSequenceInfo, ProjectAnalysis, ProjectFailure};

/// A project's output, as a keyed set of typed documents (R7.8, #1309) —
/// replaces the old `ProjectOutput.files: Vec<CompiledFile>` outright, not
/// alongside it ("Done when": `CompiledFile`/`ProjectOutput::files` are
/// gone, not aliased). Keyed by output path (project-root-relative), so the
/// sibling-path lookup `bynk-driver::output` needs for `.map`/
/// `.bynkdbg.json` files is a direct map operation instead of a linear scan,
/// and so a `wrangler.toml` document carries its real [`crate::emitter::
/// toml_doc::TomlDocument`] all the way to the write boundary instead of
/// being stringified at construction (Decision E).
pub struct Artefacts {
    pub docs: BTreeMap<PathBuf, Document>,
    /// #1883, slice 0 (the binary-safe emission path the 2026-10-10 comment
    /// names as a prerequisite): static-asset files, as raw bytes, keyed the
    /// same way as `docs` (output-root-relative). Every key lies under a
    /// Worker's [`WORKER_ASSETS_DIR`] ([`is_worker_asset_path`]).
    ///
    /// A sibling map rather than a `Document::Bytes` variant: every
    /// `Document` has a text rendering ([`Document::text`], which golden
    /// fixtures, `bynk-strip` and `bynk-wasm` all call), and an asset such as
    /// a PNG has none. Keeping bytes out of `Document` means no caller of
    /// `text()` has to decide what a binary file's text is. `bynk-strip`
    /// passes this map through untouched; `bynk-driver::output::write_output`
    /// writes and reconciles it alongside `docs`.
    ///
    /// Filled by a Workers build (slice 1b, `build_output`) with each Worker's
    /// files and `_headers`; always empty for a Bundle build.
    pub assets: BTreeMap<PathBuf, Vec<u8>>,
    /// #2321: the WebAssembly modules adapter bindings include
    /// (`binding "…" includes ["./x.wasm"]`), as raw bytes keyed by
    /// output-root-relative path, beside the binding that loads them. Module
    /// files are code, not assets: never served, and not under a Worker's
    /// assets directory. Each has a generated loader shim at
    /// [`wasm_shim_path`] in `docs` (the Workers form); a host that runs the
    /// output on Node writes [`node_wasm_shim`] over it.
    pub modules: BTreeMap<PathBuf, Vec<u8>>,
}

/// #2321: the generated loader beside an included module, whose default export
/// is the compiled `WebAssembly.Module`. `adapters/x.wasm` → `adapters/x.wasm.ts`.
pub fn wasm_shim_path(module: &Path) -> PathBuf {
    let mut name = module.as_os_str().to_os_string();
    name.push(".ts");
    PathBuf::from(name)
}

/// The Workers form of the loader: Wrangler compiles `./x.wasm` to a
/// `WebAssembly.Module` at deploy (`CompiledWasm` rule in `wrangler.toml`).
fn workers_wasm_shim(module: &Path) -> String {
    let file = module
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!(
        "// Generated by bynkc — do not edit by hand.\nimport module from \"./{file}\";\n\nexport default module;\n"
    )
}

/// The Node form of the loader (#2321, decision B): the module's bytes inlined
/// and compiled at load, so it needs no file beside the compiled output (`tsc`
/// copies only TypeScript and JSON) and no Node typings. `bynk test` writes
/// this over the Workers form at [`wasm_shim_path`].
pub fn node_wasm_shim(bytes: &[u8]) -> String {
    format!(
        "// Generated by bynkc — do not edit by hand.\nconst bytes = Uint8Array.from(atob(\"{}\"), (c) => c.charCodeAt(0));\n\nexport default await WebAssembly.compile(bytes);\n",
        base64_encode(bytes)
    )
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// #2321: whether `ctx`'s `consumes` closure (through adapter-to-adapter
/// `consumes` too) reaches an adapter whose binding includes a module.
fn worker_reaches_wasm_module(
    ctx: &str,
    unit_consumes: &HashMap<String, Vec<String>>,
    kinds: &BTreeMap<String, UnitKind>,
    adapter_bindings: &HashMap<String, AdapterBinding>,
) -> bool {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = vec![ctx];
    while let Some(unit) = stack.pop() {
        if !seen.insert(unit) {
            continue;
        }
        if adapter_bindings
            .get(unit)
            .is_some_and(|b| !b.includes.is_empty())
        {
            return true;
        }
        // Another context is a Service Binding away, not in this Worker.
        if unit != ctx && kinds.get(unit) == Some(&UnitKind::Context) {
            continue;
        }
        stack.extend(
            unit_consumes
                .get(unit)
                .into_iter()
                .flatten()
                .map(String::as_str),
        );
    }
    false
}

/// Ambient type for the Workers loader's `import module from "./x.wasm"`.
const WASM_MODULES_DTS: &str = "// Generated by bynkc — do not edit by hand.\ndeclare module \"*.wasm\" {\n  const module: WebAssembly.Module;\n  export default module;\n}\n";

/// #1883, slice 0: the name of the directory, inside a Worker's own
/// `workers/<dashes>/` output directory, that holds its static assets — the
/// directory the generated `wrangler.toml`'s `[assets] directory` names
/// (`emitter::wrangler::assets_block`). Separate from the Worker's own output so the compiled Worker and
/// its source maps are never served as public assets.
///
/// The compiler owns everything under it: `bynk-driver::output::write_output`
/// prunes any file there that the current build did not produce, whatever
/// its extension (an asset keeps its source's extension, and the platform's
/// `_headers` file has none). Shared by the producer and the pruner through
/// [`worker_assets_dir`] / [`is_worker_asset_path`] so the two can't drift.
pub const WORKER_ASSETS_DIR: &str = "assets";

/// #1883, slice 0: `workers/<dashes>/<WORKER_ASSETS_DIR>`, the output-root-
/// relative directory holding one Worker's static assets.
pub fn worker_assets_dir(dashes: &str) -> PathBuf {
    PathBuf::from("workers")
        .join(dashes)
        .join(WORKER_ASSETS_DIR)
}

/// #1883, slice 0: whether `rel` (output-root-relative) names a file inside
/// some Worker's assets directory — `workers/<dashes>/<WORKER_ASSETS_DIR>/…`,
/// at any depth. The ownership rule `bynk-driver::output`'s reconciliation
/// applies regardless of extension.
///
/// Every component after the assets directory must be a plain name
/// (`Component::Normal`), not just present. Review of #1902: `components()`
/// keeps `..`, so checking only that the tail was non-empty accepted
/// `workers/x/assets/../../../../etc/x`, which the driver's `dir.join` would
/// then write outside the output root, where reconciliation can never see
/// it. Slice 1 derives these paths from source, so this is a security rule,
/// not tidiness.
pub fn is_worker_asset_path(rel: &Path) -> bool {
    use std::path::Component;
    let mut parts = rel.components();
    let mut tail_len = 0usize;
    matches!(parts.next(), Some(Component::Normal(w)) if w == "workers")
        && matches!(parts.next(), Some(Component::Normal(_)))
        && matches!(parts.next(), Some(Component::Normal(a)) if a == WORKER_ASSETS_DIR)
        && parts.all(|c| {
            tail_len += 1;
            matches!(c, Component::Normal(_))
        })
        && tail_len > 0
}

/// P4b (#2608): whether `rel` (output-root-relative) names a file in some
/// Worker's copied D1 migrations, `workers/<dashes>/migrations/…`, where
/// `bynk-driver::output`'s reconciliation prunes a `.sql` the build no longer
/// produces. Every component must be a plain name, as for
/// [`is_worker_asset_path`].
pub fn is_worker_migration_path(rel: &Path) -> bool {
    use std::path::Component;
    let mut parts = rel.components();
    let mut tail_len = 0usize;
    matches!(parts.next(), Some(Component::Normal(w)) if w == "workers")
        && matches!(parts.next(), Some(Component::Normal(_)))
        && matches!(parts.next(), Some(Component::Normal(m)) if m == emitter::wrangler::D1_MIGRATIONS_DIR)
        && parts.all(|c| {
            tail_len += 1;
            matches!(c, Component::Normal(_))
        })
        && tail_len > 0
}

/// One typed output document. No `String` at construction for TypeScript or
/// TOML content (R7.8) — `Json`/`Js`/`SourceMap`/`DebugSidecar` stay opaque
/// payloads because `bynk-emit` never re-parses them itself, not because
/// they're an escape hatch from the rule.
pub enum Document {
    /// A generated TypeScript module. Every `bynk-emit` construction site
    /// builds this today by wrapping its still-`String`-producing content in
    /// one `TsStmt::verbatim(VerbatimOrigin::NotYetConverted, ..)` — see that
    /// variant's own doc comment for why each site does this literally,
    /// rather than through one shared helper.
    Ts(bynk_ts::TsProgram),
    /// `wrangler.toml`, still as the real tree `emit_wrangler_toml` built —
    /// printed only at the `bynk-driver` write boundary.
    Toml(crate::emitter::toml_doc::TomlDocument),
    /// Generated JSON with no tree this compiler tracks yet (`package.json`,
    /// `tsconfig.json`, the contracts/secrets manifests).
    Json(String),
    /// Type-stripped JavaScript (`bynk-strip::strip_project_to_js`'s own
    /// output) — never produced by `bynk-emit` itself, which only ever
    /// builds `Ts`; kept distinct from `Ts` so a stripped artefact never
    /// round-trips back through the TypeScript printer/lint.
    Js(String),
    /// A source-map v3 document, split out of its originating file's own
    /// staged `source_map` at the sibling path `write_output` names it
    /// (`<file>.map`).
    SourceMap(String),
    /// The handler-label debug sidecar, split out the same way at
    /// `<file>.bynkdbg.json`.
    DebugSidecar(String),
    /// P4b (#2608): a `D1` migration, copied verbatim from the project's
    /// `migrations/` folder beside the Worker's `wrangler.toml`.
    Sql(String),
}

impl Document {
    /// This document's own text, whichever kind it is — for read-side
    /// callers that only need bytes (golden fixtures, `bynk-strip`'s own
    /// need to feed `Ts` content through `strip_types`, `bynk-wasm`'s
    /// JS-facing API, in-process test helpers). `Ts`/`Toml` print through
    /// the same printer `bynk-driver::output::write_document` writes from
    /// (R7.3/R7.6) — this is a rendering, not a second construction path;
    /// nothing here builds a `Document` from a `String`.
    pub fn text(&self) -> String {
        match self {
            Document::Ts(program) => bynk_ts::print(program, "", "", "").text,
            Document::Toml(doc) => crate::emitter::print_toml_document(doc),
            Document::Json(s)
            | Document::Js(s)
            | Document::SourceMap(s)
            | Document::DebugSidecar(s)
            | Document::Sql(s) => s.clone(),
        }
    }
}

/// One generated document, staged during `build_output`/`emit_unit` before
/// the final split into [`Artefacts`] — see `build_output`'s own tail.
struct StagedFile {
    output_path: PathBuf,
    source_map: Option<String>,
    debug_metadata: Option<String>,
    document: Document,
}

/// Result of compiling a project.
pub struct ProjectOutput {
    /// P7.6 (#1309): every generated document, typed (R7.8).
    /// `bynk-driver::output::write_output` writes from this.
    pub artefacts: Artefacts,
    /// v0.89 (ADR 0117): non-failing warnings emitted on a successful build —
    /// surfaced (the CLI prints them, the LSP shows them) but not gating.
    pub warnings: Vec<AttributedError>,
    /// Per-file source snapshots, keyed the same way `ProjectFailure::snapshots`
    /// is — lets a warning render with real file/line/col context (ariadne or
    /// `path:line:col:`) instead of the position-free `warning[category]: …`
    /// fallback a successful build previously had no way to avoid.
    pub snapshots: Vec<(PathBuf, String)>,
    /// #1772: the project or tree root the build was given, as the caller
    /// spelled it. `snapshots` and each error's `source_path` are keyed by
    /// identity path, relative to this root; a renderer shows
    /// `display_root.join(identity)`, the path as typed from the working
    /// directory. Empty for an in-memory build.
    pub display_root: PathBuf,
    /// v0.67: the test manifest — every discovered suite and case, retained at
    /// emit time so `bynkc test --no-run --format json` can render a discovery
    /// document without running the suite. Built from the same names + spans the
    /// runner would emit at `suite-begin`/`case`, so a discovery document
    /// reconciles cleanly against a later run's document (same suite name/kind,
    /// same case names). Ordered to match the runner (`emit_test_main`).
    pub discovered: Vec<DiscoveredSuite>,
    /// #1078: the reconciled `bynk.schema.lock` content, `Some` whenever
    /// `CompileOptions::schema_registry` was `SchemaLock::On` for this build
    /// — `bynk-emit` computes it but never writes it; a building caller
    /// (`bynkc compile`, `bynk dev`/`deploy`) persists it atomically, exactly
    /// the discipline `schema_registry.rs` used to implement itself. `None`
    /// when the registry was off, regardless of whether the content would
    /// have changed — the unchanged-content no-op lives in the writer, not
    /// here, so a caller that reconciles the same shape twice in a row still
    /// gets `Some` both times.
    pub schema_lock: Option<String>,
}

/// v0.67: a discovered test suite — one `test <target>` group (unit) or
/// `test integration "<suite>"` (integration). `name` + `kind` mirror exactly
/// what the NDJSON runner emits at `suite-begin` (kind `"unit"` carries the
/// joined target name; `"integration"` carries the bare suite name), so the
/// editor reconciles discovery and run documents to the same tree items.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredSuite {
    pub name: String,
    pub kind: &'static str,
    pub cases: Vec<DiscoveredCase>,
    /// #2138: the cases a remote build skipped (`--skip-unsupported`), and
    /// why. Empty for every other build.
    pub skipped: Vec<RemoteSkip>,
    /// #2138: under `--against dev`, the distinct `env` maps this suite's
    /// cases run with ([`RemoteTests::dev`]). Empty for every other build.
    pub envs: Vec<BTreeMap<String, String>>,
}

/// One discovered `test "<name>"` case. `location` points at the case-name
/// literal (a run *failure* instead points at the failing `assert`), giving the
/// editor click-through to the declaration before any run.
#[derive(Debug, Clone, PartialEq)]
pub struct DiscoveredCase {
    pub name: String,
    pub location: Option<TestLocation>,
    /// The case's effective tier (`"unit"`, `"integration"` or `"system"`: its own
    /// `as <tier>`, else the suite default, else `unit`), when it has one. A
    /// property or contract attack has none.
    pub tier: Option<&'static str>,
    /// `"property"` or `"history"` for a generative case (a `property`, a history
    /// property or a contract attack), whose draws depend on the run's seed and on
    /// its position among the module's generative cases. `None` for a `case`.
    pub generative: Option<&'static str>,
    /// A doc-comment example's case (#2461), set by the driver after a build that
    /// compiled such examples in; `false` from emission. A document marks it
    /// `"origin": "doc"`.
    pub doc: bool,
}

/// A project-root-relative `path:line:col` source location, structured. Line and
/// col are 1-indexed (the [`bynk_syntax::span::line_col`] convention).
#[derive(Debug, Clone, PartialEq)]
pub struct TestLocation {
    pub path: String,
    pub line: u32,
    pub col: u32,
}

// P4.1 (#1115): `AdapterBinding` and `BuildTarget` relocated to
// `bynk-check::project_model` alongside `phase_group`, which constructs
// them — see that module's own doc comment. `BuildTarget` is re-exported
// below at its old `crate::project::BuildTarget` path (part of this crate's
// public surface); `AdapterBinding` was never public here, so a plain `use`
// (above) is enough.

/// The extension emitted import specifiers use (`import … from "./x.<ext>"`).
///
/// `Js` is the default and the only shape for normal builds: Node's ESM
/// resolution and `tsc` require `.js` specifiers even though the sources are `.ts`. `Ts` is
/// the **debug build** (slice 2, ADR 0104): `bynkc test --inspect` runs the
/// emitted `.ts` directly under Node's line-preserving strip-only type-stripping,
/// where slice 1's source maps apply unchanged — but Node will not resolve a `.js`
/// specifier to the `.ts` on disk, so the debug build emits `.ts` specifiers.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum ImportExt {
    #[default]
    Js,
    Ts,
}

impl ImportExt {
    /// The bare extension string (`"js"` / `"ts"`), for `Path::with_extension`
    /// and specifier formatting.
    pub fn as_str(self) -> &'static str {
        match self {
            ImportExt::Js => "js",
            ImportExt::Ts => "ts",
        }
    }
}

/// Options for [`compile_project`]. Construct with [`CompileOptions::single`] or
/// [`CompileOptions::split`], then chain `.target(…)` / `.platform(…)` /
/// `.import_ext(…)` to override the bundle/default-platform/`.js` defaults.
#[derive(Clone)]
pub struct CompileOptions {
    pub target: BuildTarget,
    pub platform: Platform,
    pub roots: Roots,
    /// The import-specifier extension (slice 2). `Js` (default) for normal builds;
    /// `Ts` for the `bynkc test --inspect` debug build.
    pub import_ext: ImportExt,
    /// v0.115 (testing track slice 3, DECISION J): the build profile for function
    /// contracts. `true` (dev/test) emits the call-site guard around a contracted
    /// `fn`; `false` (release/deploy) strips it entirely for zero runtime cost.
    /// `bynkc test` and `--inspect` set it on; `bynkc compile` leaves it off.
    pub contracts: bool,
    /// #1821 (ADR 0147 D3): whether `suite`s are type-checked and emitted (the
    /// `tests/` modules and `tests/main.ts`). On by default; `bynkc compile`
    /// and the driver's dev/deploy build turn it off, so a suite never reaches
    /// the deployable. `bynkc test` leaves it on. Off strips suites after
    /// parsing: a file that does not parse still fails the build, since it
    /// cannot be told apart from production source.
    pub tests: bool,
    /// #57 (testing track): when `Some`, every file `roots` would otherwise
    /// discover on disk is instead read from here — keyed the same way
    /// `discovery::read_source`'s overlay is (a canonicalised absolute path,
    /// falling back to the literal path when the file has no on-disk
    /// counterpart to canonicalise). Filesystem discovery is skipped entirely;
    /// `roots` still supplies `src_root`/`tests_root` and their prefixes, so a
    /// `Roots::Split` project can drive both trees in-memory.
    ///
    /// #1077/#1081: `bynk-driver`'s `project_options`/`try_project_options`
    /// *do* set this now — the real CLI entry points walk and read every
    /// project file themselves and hand the result here, so `bynk-emit`
    /// itself no longer discovers or reads anything on disk for the CLI
    /// path. `bynkc`/the LSP still don't: `bynkc` routes through
    /// `bynk-driver`, and the LSP (`bynk_check::analysis::analyse_project`
    /// since P4.2) never goes through `CompileOptions` at all — it has its
    /// own, separate discovery path (#1079). `bynk-emit`'s own tests also
    /// set this, to exercise the full
    /// project pipeline (cross-context `uses`, multi-file layouts,
    /// workers-mode emission, …) without an on-disk fixture tree.
    pub sources: Option<HashMap<PathBuf, String>>,
    /// Events track, slice 3c (#980): reconcile against `bynk.schema.lock`.
    /// **Off by default** — `bynkc compile`'s directory branch and `bynk`'s
    /// deploy/dev build turn it on and write the reconciled lock back;
    /// `check`'s and `test`'s directory paths (#2078) turn it on read-only,
    /// so a non-additive change fails them too, and never write it. Every
    /// library/test caller (in-memory builds, `bynkc/tests/e2e.rs`'s
    /// in-place fixture compiles, `bynk-emit`'s own `sources`-driven tests,
    /// the LSP) leaves it off, so a compile never mutates a project tree it
    /// wasn't asked to.
    ///
    /// #1078: this carries the lock's pre-read content, not just an on/off
    /// switch — `bynk-emit` reads and writes no disk for this file (the same
    /// move #1077/#1081 already made for `.bynk` source content). See
    /// [`SchemaLock`]'s own doc for the read side; the reconciled content
    /// comes back out on [`ProjectOutput::schema_lock`] for the caller to
    /// persist.
    pub schema_registry: SchemaLock,
    /// #2554: `bynk.contracts.lock`'s pre-read content: the older contracts
    /// each callee also accepts. `None` (every library, test and LSP caller,
    /// and a project without the file) accepts none, so a callee answers only
    /// the contract it has. `bynk-driver`'s `try_project_options` reads it, as
    /// it reads the manifest, so every CLI build honours it; a build only ever
    /// reads it, and the driver writes it.
    pub contracts_lock: Option<String>,
    /// #1890: the project's extra Cloudflare compatibility flags, from
    /// `bynk.toml`'s `[workers] compatibility_flags`, in manifest order.
    /// A Workers build writes them into every `wrangler.toml` after
    /// [`crate::DEFAULT_COMPATIBILITY_FLAGS`]; a duplicate is dropped with a
    /// `bynk.project.duplicate_compatibility_flag` warning. Empty by default:
    /// `bynk-driver`'s `project_options`/`try_project_options` read the
    /// manifest and set it, as does `bynkc/tests/e2e.rs` for a fixture.
    pub compatibility_flags: Vec<String>,
    /// `bynk.toml`'s `[workers] workers_dev`: written into every
    /// `wrangler.toml` when set, left out when `None`.
    pub workers_dev: Option<bool>,
    /// #1883, slice 1b: the project's `assets/` folder — its files, keyed by
    /// normalised project-relative path (`assets/board.css`), listed by the
    /// caller, and a reader for their bytes
    /// ([`assets::AssetFolder`], usually `AssetFolder::on_disk`). The build
    /// reads only the files an `asset` declaration names, once each, through
    /// that reader, so `bynk-emit` never touches the disk and a large file
    /// nothing declares is never read. `bynk-driver`'s
    /// `project_options`/`try_project_options` and `bynk-testkit`'s
    /// `compile_options_*` set it.
    ///
    /// `None` means no filesystem was consulted (`compile_in_memory`, the
    /// playground). A check then skips the existence check; a build treats it
    /// as an empty folder, so an asset there is `bynk.asset.missing_file`
    /// rather than a URL made up for a file nobody read.
    pub asset_files: Option<assets::AssetFolder>,
    /// #2387, slice 2: the project's `embeds/` folder, listed by the caller
    /// with a reader for the bytes of only the files an `embed` declaration
    /// names ([`assets::AssetFolder::embeds_on_disk`]), exactly as
    /// [`CompileOptions::asset_files`] does for `assets/`. `None` means no
    /// filesystem was consulted: a check then skips the existence check, and a
    /// build treats it as an empty folder, so an embed there is
    /// `bynk.embed.missing_file` rather than text made up for a file nobody
    /// read.
    pub embed_files: Option<assets::EmbedFolder>,
    /// P4b (#2608): the project's `migrations/` folder, listed by the caller
    /// ([`assets::AssetFolder::migrations_on_disk`]). Every file in a declared
    /// `D1` database's directory is read, checked, copied beside the Worker's
    /// `wrangler.toml` and run on each test database. `None` means no
    /// filesystem was consulted.
    pub migration_files: Option<assets::AssetFolder>,
    /// #2138 (end-to-end testing track §3.5, §3.7): `Some` when `bynk test
    /// --against` runs the `system` suites against a real runtime. The build
    /// then emits only the `system` suites, routes their drivers to the
    /// target's URLs (`BYNK_TEST_TARGETS`) instead of the in-process harness,
    /// and refuses (or, under `--skip-unsupported`, skips) a case that uses
    /// what a remote run can't honour. `None` for every other build.
    pub remote_tests: Option<RemoteTests>,
}

/// #2138: how a remote test build treats a `system` case it can't run
/// against a real runtime ([`CompileOptions::remote_tests`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RemoteTests {
    /// `--skip-unsupported`: skip such a case, recording why
    /// ([`DiscoveredSuite::skipped`]), instead of refusing it.
    pub skip_unsupported: bool,
    /// The named target's declared `vars`, which each case's `env` must agree
    /// with (`bynk_check::test_env::remote_env_disagreement`).
    pub vars: BTreeMap<String, String>,
    /// The names of the named target's secrets, which a case's `env` may not
    /// set.
    pub secrets: Vec<String>,
    /// `--against dev`: the runner starts the Workers itself and applies each
    /// case's `env` to them, so `env` is not checked against `vars`; each
    /// suite instead reports the distinct `env` maps its cases run with
    /// ([`DiscoveredSuite::envs`]), one `bynk dev` per map.
    pub dev: bool,
    /// Under `dev`, build only the cases whose `env` is exactly this map: the
    /// group one `bynk dev` serves.
    pub only_env: Option<BTreeMap<String, String>>,
}

/// #2138: a `system` case a remote test build skipped, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteSkip {
    pub case: String,
    /// The `bynk.test.remote_unsupported_*` code.
    pub code: String,
    /// The reason, as the refusal would have worded it.
    pub reason: String,
}

impl CompileOptions {
    /// Single-root project (`src == tests`), bundle target, default platform.
    pub fn single(root: impl Into<PathBuf>) -> Self {
        Self {
            target: BuildTarget::Bundle,
            platform: Platform::default(),
            roots: Roots::Single(root.into()),
            import_ext: ImportExt::default(),
            contracts: false,
            tests: true,
            sources: None,
            schema_registry: SchemaLock::Off,
            contracts_lock: None,
            compatibility_flags: Vec::new(),
            workers_dev: None,
            asset_files: None,
            embed_files: None,
            migration_files: None,
            remote_tests: None,
        }
    }

    /// v0.9.1 split layout (source and test units in separate subdirectories
    /// under `project_root`), bundle target, default platform. Use this from
    /// `bynkc test` so its rooting matches `bynkc compile`'s.
    pub fn split(project_root: impl Into<PathBuf>, paths: ProjectPaths) -> Self {
        Self {
            target: BuildTarget::Bundle,
            platform: Platform::default(),
            roots: Roots::Split {
                project_root: project_root.into(),
                paths,
            },
            import_ext: ImportExt::default(),
            contracts: false,
            tests: true,
            sources: None,
            schema_registry: SchemaLock::Off,
            contracts_lock: None,
            compatibility_flags: Vec::new(),
            workers_dev: None,
            asset_files: None,
            embed_files: None,
            migration_files: None,
            remote_tests: None,
        }
    }

    /// #2138: a remote test build ([`CompileOptions::remote_tests`]).
    pub fn remote_tests(mut self, remote: Option<RemoteTests>) -> Self {
        self.remote_tests = remote;
        self
    }

    /// Select the build target. `Bundle` (default) is the v0.6+ single-bundle
    /// layout; `Workers` (v0.8) emits per-context Cloudflare Workers.
    pub fn target(mut self, target: BuildTarget) -> Self {
        self.target = target;
        self
    }

    /// Slice 2: select the import-specifier extension. `Ts` is the debug build
    /// for `bynkc test --inspect` (run the `.ts` directly under Node strip-only).
    pub fn import_ext(mut self, ext: ImportExt) -> Self {
        self.import_ext = ext;
        self
    }

    /// v0.115: enable the function-contract call-site guard (dev/test profile).
    /// `bynkc test` and `--inspect` call this; the deploy build leaves it off so
    /// contract checks never reach production (DECISION J).
    pub fn contracts(mut self, on: bool) -> Self {
        self.contracts = on;
        self
    }

    /// #1821 (ADR 0147 D3): whether to type-check and emit `suite`s. A build
    /// that produces a deployable (`bynkc compile`, `bynk dev`/`deploy`) turns
    /// it off; a suite is test-only.
    pub fn tests(mut self, on: bool) -> Self {
        self.tests = on;
        self
    }

    /// v0.17: select the deploy [`Platform`] (selects the `bynk` surface
    /// binding). The MVP ships `cloudflare` only.
    pub fn platform(mut self, platform: Platform) -> Self {
        self.platform = platform;
        self
    }

    /// #57 (testing track): supply every file in-memory instead of walking
    /// `roots` on disk — keyed the same way `discovery::read_source`'s
    /// overlay is (see the `sources` field's own doc).
    pub fn sources(mut self, sources: HashMap<PathBuf, String>) -> Self {
        self.sources = Some(sources);
        self
    }

    /// Events track, slice 3c (#980): turn on `bynk.schema.lock`
    /// reconciliation for this build, with its pre-read content (or
    /// verified-absent `None`, for a fresh project). See [`SchemaLock`] and
    /// the field's own doc for who calls this and why everyone else leaves
    /// it off.
    pub fn schema_registry(mut self, mode: SchemaLock) -> Self {
        self.schema_registry = mode;
        self
    }

    /// #2554: the contracts lock's pre-read content. See the field's own doc.
    pub fn contracts_lock(mut self, content: Option<String>) -> Self {
        self.contracts_lock = content;
        self
    }

    /// #1890: the project's extra Cloudflare compatibility flags, from
    /// `bynk.toml`'s `[workers]` table. See the field's own doc.
    pub fn compatibility_flags(mut self, flags: Vec<String>) -> Self {
        self.compatibility_flags = flags;
        self
    }

    /// The project's `[workers] workers_dev`. See the field's own doc.
    pub fn workers_dev(mut self, workers_dev: Option<bool>) -> Self {
        self.workers_dev = workers_dev;
        self
    }

    /// #1883, slice 1b: the listed `assets/` folder. See the field's own doc.
    pub fn asset_files(mut self, folder: assets::AssetFolder) -> Self {
        self.asset_files = Some(folder);
        self
    }

    /// #2387, slice 2: the listed `embeds/` folder. See the field's own doc.
    pub fn embed_files(mut self, folder: assets::EmbedFolder) -> Self {
        self.embed_files = Some(folder);
        self
    }

    /// P4b (#2608): the listed `migrations/` folder. See the field's own doc.
    pub fn migration_files(mut self, folder: assets::AssetFolder) -> Self {
        self.migration_files = Some(folder);
        self
    }
}

/// #57: turn `options.sources` into the `(overlay, discovered)` pair
/// `run_checks` expects — the caller-supplied file list, partitioned across
/// `trees` the same way a real walk would (single-tree: every file lands in
/// the first tree's list, matching `compile_in_memory`'s own convention).
/// Shared by [`compile_project`] and [`check_project`] so the two can't drift
/// on this.
///
/// #1077/#1081 review: sorts every partition. `sources`'s own key order is a
/// `HashMap`'s — unspecified, randomised per process — but `phase_parse`
/// walks each tree's file list in order to assign sequential `FileId`s and
/// `ExprId`s (embedded in emitted spans/source maps) and `run_checks` pushes
/// diagnostics in file order, both of which a real disk walk already
/// guaranteed via `discover_bynk_files`'s own `out.sort()`. Without this, a
/// `sources`-driven compile (the CLI's own path as of #1081) would silently
/// vary its diagnostic order and `FileId` assignment run to run.
///
/// R3.9 (#1113): partitions across every `trees` entry, not a hardcoded
/// primary/secondary pair — a path that doesn't start with any tree's root
/// (shouldn't happen for a well-formed `sources` map) falls back to the
/// *last* tree, matching the pre-R3.9 two-tree `partition`'s fallback (an
/// unmatched key landed in `tests_files`, the second/last half of the pair).
/// Falling back to the *first* tree instead — silently tried during this
/// slice's initial cut — would move an unmatched file into whichever tree
/// `check_file_directory_conflicts` and `identity_path` treat as primary,
/// changing its attribution with no diagnostic raised either way; matching
/// the old convention at least keeps this rare path's behaviour unchanged by
/// the crate move.
type Overlay = HashMap<PathBuf, String>;
/// One file list per `trees` entry — the same shape `phase_discovery` and
/// `run_checks`'s own `discovered` parameter already use.
type Discovered = Vec<Vec<PathBuf>>;

/// #2223: the trees a manifest-backed project compiles — its own
/// ([`Roots::trees`]) and then each path dependency's
/// ([`bynk_project::dependency_trees`]) — so the source map partitions a
/// dependency's files into their own tree rather than the last one.
fn project_trees(options: &CompileOptions) -> Vec<(PathBuf, PathBuf)> {
    let mut trees = options.roots.trees();
    if let Roots::Split { project_root, .. } = &options.roots {
        let empty = HashMap::new();
        let overlay = options.sources.as_ref().unwrap_or(&empty);
        trees.extend(bynk_project::dependency_trees(project_root, overlay));
    }
    trees
}

fn sources_to_discovered(
    sources: &HashMap<PathBuf, String>,
    trees: &[(PathBuf, PathBuf)],
) -> (Overlay, Option<Discovered>) {
    let mut buckets: Vec<Vec<PathBuf>> = vec![Vec::new(); trees.len()];
    let mut keys: Vec<PathBuf> = sources.keys().cloned().collect();
    keys.sort();
    for p in keys {
        let idx = trees
            .iter()
            .position(|(root, _)| p.starts_with(root))
            .unwrap_or(trees.len().saturating_sub(1));
        buckets[idx].push(p);
    }
    (sources.clone(), Some(buckets))
}

/// Compile a Bynk project, keeping error attribution + snapshots on failure
/// (so the CLI can render project errors with source context, ADR 0052). Use
/// `.map_err(ProjectFailure::flatten)` for the flattened `Vec<CompileError>`
/// shape.
pub fn compile_project(options: &CompileOptions) -> Result<ProjectOutput, ProjectFailure> {
    // T3.6b (R4.1): one table per build, shared across every unit compiled.
    let tys = &Arc::new(Types::new());
    let trees = project_trees(options);
    let excludes = options.roots.excludes();
    let (overlay, discovered) = match &options.sources {
        Some(sources) => sources_to_discovered(sources, &trees),
        None => (HashMap::new(), None),
    };
    let run = run_checks(
        &trees,
        options.target,
        options.platform,
        options.import_ext,
        Mode::Build,
        &overlay,
        &excludes,
        discovered,
        options.contracts,
        options.tests,
        &options.schema_registry,
        options.remote_tests.as_ref(),
        options.roots.project_root(),
        options.asset_files.as_ref(),
        options.embed_files.as_ref(),
        options.migration_files.as_ref(),
        matches!(options.roots, Roots::Split { .. }),
        tys,
    );
    // #1078: `bynk-emit` no longer writes `bynk.schema.lock` itself — the
    // reconciled content comes back on `ProjectOutput::schema_lock`
    // (`finish_build` populates it from `RunChecks::Checked`, only ever
    // constructed on the `Ok` path, i.e. only on a fully clean build — a
    // build that fails for any reason, including a schema mismatch
    // reconciliation itself just reported, produces `Err(ProjectFailure)`
    // instead and has no revised content for a caller to persist). The
    // caller (today, `bynk-driver`'s two wiring points) does the atomic
    // write.
    finish_build(
        run,
        options.import_ext,
        options.roots.project_root(),
        &options.compatibility_flags,
        options.workers_dev,
        options.contracts_lock.as_deref(),
    )
}

/// Result of [`check_project`]: every diagnostic from a non-bailing project
/// analysis — errors *and* warnings together (ADR 0117), unconditionally,
/// unlike [`ProjectOutput`]/[`ProjectFailure`] where `errors`/`warnings` is
/// picked by which variant the caller got. `bynk check`'s exit code is
/// decided by [`Self::has_errors`], not by whether this was reached at all.
pub struct ProjectCheck {
    pub errors: Vec<AttributedError>,
    pub snapshots: Vec<(PathBuf, String)>,
    /// #1772: the project or tree root the build was given, as the caller
    /// spelled it. `snapshots` and each error's `source_path` are keyed by
    /// identity path, relative to this root; a renderer shows
    /// `display_root.join(identity)`, the path as typed from the working
    /// directory. Empty for an in-memory build.
    pub display_root: PathBuf,
    /// State-migrations S4 (#2259): each agent class's lock version, oldest
    /// readable version and valid `migrate` steps, keyed `<unit>.<Agent>`.
    /// Empty without a lock, or when the check stopped before reconciling.
    pub agent_versions: HashMap<String, bynk_check::schema_registry::AgentVersions>,
}

impl ProjectCheck {
    /// Finding #64: `bynk check`'s exit-code gate is "does any error-severity
    /// diagnostic exist in the project" — not "did the pipeline reach the end
    /// without bailing", which is what `compile_project`'s `Result` encoded
    /// and why a `bynk.toml`-wide structural error anywhere silently hid every
    /// later diagnostic (including a test body's own type errors) from
    /// `bynk check`, even though the editor (via
    /// `bynk_check::analysis::analyse_project`, which shares this same
    /// non-bailing shape) still reported them.
    pub fn has_errors(&self) -> bool {
        self.errors
            .iter()
            .any(|ae| bynk_syntax::Severity::for_error(&ae.error) == bynk_syntax::Severity::Error)
    }
}

/// Check a project without building (finding #64) — never bails after
/// discovery (`Mode::Analyse`, the same mode `bynk_check::analysis::analyse_project`
/// runs for the editor), so a diagnostic anywhere in the project does not
/// suppress diagnostics elsewhere. `compile_project`'s `Mode::Build` bails at
/// the first structural error and returns only what it collected up to that
/// point — correct for `build`/`test`, which must not emit past a real
/// error, but wrong for `check`, whose only job is to report everything.
/// `bynk check`'s directory path calls this instead of `compile_project`.
pub fn check_project(options: &CompileOptions) -> ProjectCheck {
    // T3.6b (R4.1): one table per check, shared across every unit.
    let tys = &Arc::new(Types::new());
    let trees = project_trees(options);
    let excludes = options.roots.excludes();
    let (overlay, discovered) = match &options.sources {
        Some(sources) => sources_to_discovered(sources, &trees),
        None => (HashMap::new(), None),
    };
    let run = run_checks(
        &trees,
        options.target,
        options.platform,
        options.import_ext,
        Mode::Analyse,
        &overlay,
        &excludes,
        discovered,
        options.contracts,
        options.tests,
        // #2078: `bynk check` reconciles against the committed
        // `bynk.schema.lock` whenever the caller pre-read it
        // (`SchemaLock::On`), so a non-additive event change fails the check
        // the way it fails a build. Read-only by construction: a
        // `ProjectCheck` carries no reconciled content for anyone to write.
        &options.schema_registry,
        options.remote_tests.as_ref(),
        options.roots.project_root(),
        options.asset_files.as_ref(),
        options.embed_files.as_ref(),
        options.migration_files.as_ref(),
        matches!(options.roots, Roots::Split { .. }),
        tys,
    );
    let (errors, snapshots, agent_versions) = match run {
        RunChecks::Bailed {
            errors, snapshots, ..
        } => (errors, snapshots, HashMap::new()),
        RunChecks::Checked {
            errors,
            snapshots,
            agent_versions,
            ..
        } => (errors, snapshots, agent_versions),
    };
    ProjectCheck {
        // #1890: the manifest's duplicate-flag warnings, as a build reports.
        errors: errors
            .into_all()
            .into_iter()
            .chain(compatibility_flag_warnings(&options.compatibility_flags))
            .collect(),
        snapshots,
        display_root: options.roots.project_root().to_path_buf(),
        agent_versions,
    }
}

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
        true,
        &SchemaLock::Off,
        None,
        &root,
        None,
        None,
        None,
        false,
        tys,
    );
    finish_build(run, ImportExt::Js, Path::new(""), &[], None, None)
}

/// [`compile_in_memory`] for several files (#1830): each `(path, source)` pair
/// is one project file, its path relative to a single-tree root exactly as on
/// disk (`demo/shop.bynk` for `context demo.shop`, `tests/shop.bynk` for a
/// suite). Test-only, so a unit test can build a multi-unit project, or a
/// unit with its suite, without a fixture directory; reach it through
/// `crate::testkit`. `tests` keeps the suites, as `bynkc test` does; off is
/// the deployable build `bynkc compile` makes.
#[cfg(test)]
pub(crate) fn compile_files_in_memory(
    files: &[(&str, &str)],
    target: BuildTarget,
    platform: Platform,
    tests: bool,
) -> Result<ProjectOutput, ProjectFailure> {
    compile_files_in_memory_remote(files, target, platform, tests, None, None)
}

/// [`compile_files_in_memory`] as a remote test build (#2138,
/// [`CompileOptions::remote_tests`]).
#[cfg(test)]
pub(crate) fn compile_files_in_memory_remote(
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
#[cfg(test)]
pub(crate) fn compile_files_in_memory_locked(
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
        tests,
        &SchemaLock::Off,
        remote,
        &root,
        asset_files,
        None,
        None,
        false,
        tys,
    );
    finish_build(run, ImportExt::Js, Path::new(""), &[], None, contracts_lock)
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
/// [`ExprTypeSink::take_files`]), for a position→type query (#397, the
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
        true,
        &SchemaLock::Off,
        None,
        &root,
        None,
        None,
        None,
        false,
        tys,
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
/// than [`ProjectPaths::conventional`]'s disk probe. Non-`.bynk` entries other
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
        true,
        &SchemaLock::Off,
        None,
        &project_root,
        None,
        None,
        None,
        false,
        tys,
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

/// #1890: one `bynk.project.duplicate_compatibility_flag` warning per
/// duplicate in the manifest's `[workers] compatibility_flags` (`extra`),
/// shared by [`finish_build`] and [`check_project`] so `compile` and `check`
/// report the same thing. Attributed to `bynk.toml` by its root-relative
/// identity path, like every other [`AttributedError`]; the manifest is never
/// a snapshot, so renderers print that path (or none) rather than a source
/// excerpt, and the message names `bynk.toml` itself.
fn compatibility_flag_warnings(extra: &[String]) -> Vec<AttributedError> {
    let (_, duplicates) = crate::compatibility_flags(extra);
    duplicates
        .into_iter()
        .map(|flag| {
            let default = crate::DEFAULT_COMPATIBILITY_FLAGS.contains(&flag.as_str());
            bynk_project::duplicate_compatibility_flag_warning(
                &flag,
                default,
                PathBuf::from("bynk.toml"),
            )
        })
        .collect()
}

/// Assemble a finished [`ProjectOutput`] (or a [`ProjectFailure`]) from a
/// [`RunChecks`] result — the shared tail of `compile_project` and
/// `compile_in_memory`.
///
/// #1890: `compatibility_flags` is the manifest's `[workers]` list
/// ([`CompileOptions::compatibility_flags`]). It is merged with the defaults
/// once here, for every Worker, and each duplicate it held becomes one
/// [`compatibility_flag_warnings`] warning, whatever the build target: the
/// duplicate is a fact about the manifest.
fn finish_build(
    run: RunChecks,
    import_ext: ImportExt,
    display_root: &Path,
    compatibility_flags: &[String],
    workers_dev: Option<bool>,
    // #2554: the pre-read `bynk.contracts.lock`; `None` is a project without
    // one, which accepts no older contract.
    contracts_lock: Option<&str>,
) -> Result<ProjectOutput, ProjectFailure> {
    // #2554: an unreadable contracts lock fails the build, as an unreadable
    // schema lock does: reading it as empty would have a callee silently stop
    // accepting the callers it was deployed to keep answering.
    let contracts_lock = match bynk_project::contracts_lock::parse(contracts_lock, display_root) {
        Ok(lock) => lock,
        Err(message) => {
            return Err(ProjectFailure {
                errors: vec![bynk_project::contracts_lock::corrupt_error(message)],
                snapshots: Vec::new(),
                display_root: display_root.to_path_buf(),
            });
        }
    };
    let (flags, _) = crate::compatibility_flags(compatibility_flags);
    let flag_warnings = compatibility_flag_warnings(compatibility_flags);
    match run {
        RunChecks::Bailed {
            errors, snapshots, ..
        } => Err(ProjectFailure {
            // ADR 0117: a failed build still renders any warnings it produced
            // (the sink yields errors then warnings).
            errors: errors.into_all().into_iter().chain(flag_warnings).collect(),
            snapshots,
            display_root: display_root.to_path_buf(),
        }),
        RunChecks::Checked {
            errors, snapshots, ..
        } if !errors.is_empty() => Err(ProjectFailure {
            errors: errors.into_all().into_iter().chain(flag_warnings).collect(),
            snapshots,
            display_root: display_root.to_path_buf(),
        }),
        RunChecks::Checked {
            errors,
            snapshots,
            parsed,
            compiled,
            runnable_tests,
            integration_outputs,
            integration_runnables,
            groups,
            kinds,
            unit_consumes,
            unit_consumes_aliases,
            unit_tables,
            unit_consumed_context_types,
            unit_callees,
            unit_event_subscriber_shapes,
            unit_uses,
            unit_flattened,
            adapter_bindings,
            npm_deps,
            target,
            schema_registry,
            asset_bytes,
            agent_versions,
            d1_migrations,
            ..
        } => {
            let mut out = build_output(
                parsed,
                compiled,
                runnable_tests,
                integration_outputs,
                integration_runnables,
                groups,
                kinds,
                unit_consumes,
                unit_consumes_aliases,
                unit_tables,
                &unit_consumed_context_types,
                unit_callees,
                unit_event_subscriber_shapes,
                unit_uses,
                unit_flattened,
                adapter_bindings,
                npm_deps,
                target,
                import_ext,
                &flags,
                workers_dev,
                asset_bytes.as_ref(),
                &agent_versions,
                &contracts_lock,
                &d1_migrations,
            );
            // ADR 0117: surface non-failing warnings on the successful build
            // (errors is empty here — the guard arm above caught any).
            // `build_output` put the contracts lock's warnings here (#2554).
            let lock_warnings = std::mem::take(&mut out.warnings);
            out.warnings = errors.into_warnings();
            out.warnings.extend(flag_warnings);
            out.warnings.extend(lock_warnings);
            out.snapshots = snapshots;
            out.display_root = display_root.to_path_buf();
            // #1078: the reconciled registry, if this build had one on —
            // bynk-emit computes it, the caller persists it.
            out.schema_lock = schema_registry.map(|reg| schema_registry::serialize(&reg));
            Ok(out)
        }
    }
}

// P4.1 (#1115): `normalize_service_defaults`/`inject_service_defaults`
// relocated to `bynk-check::project_model` (called from both `run_checks`
// here and the new `bynk-check`-native analysis entry point, ahead of
// `phase_group` in both).

/// v0.54 (#655): whether a context's services declare an `on call … by c: Caller`
/// handler, whose emitted `deps` carries the calling context's qualified name as
/// its `CallerId` identity (ADR 0092); in bundle mode the compose root supplies
/// that name to `__makeSurface`, mirroring the `X-Bynk-Caller` header a Worker
/// reads at its entry. Delegates to the *same*
/// [`any_service_binds_caller`](crate::emitter::any_service_binds_caller) the
/// emitter's `emit_make_surface` calls, so the compose root and the surface can
/// never disagree on which providers take the extra `__caller` argument.
fn context_binds_caller(table: &UnitTable) -> bool {
    crate::emitter::any_service_binds_caller(table.services.values(), &table.actors)
}

// P4.1 (#1115): `record_analyse_types` relocated to
// `bynk-check::check_pipeline` (called from `check_file_core`'s own
// error-path exits, plus every clean-path caller — `check_unit_files`'s
// `Mode::Analyse` branch below and the new entry point's own).

// P4.1 (#1115): the whole discovery->parse->group->resolve pipeline
// (`phase_discovery` through `phase_file_index`/`assemble_unit_info`, plus
// per-unit symbol composition — `compose_unit_symbols`/
// `merge_consumed_exports`/`collect_unit_methods`) relocated to
// `bynk-check::project_model` — see that module's own doc comment for why
// (the same `extract, don't duplicate` move `bynk-project` itself was, P4.0).
// `run_checks`, below, is now a caller of `project_model::phase_*` instead of
// owning this logic inline.

/// v0.119 (ADR 0155): the agents a `for all run: History[Agent]` property drives,
/// scanned across every test suite in the project. `emit_agent` gates the
/// exported `__bynkDriveHistory_<Agent>` driver on membership, so a non-targeted
/// agent's emission is unchanged.
fn collect_history_target_agents(parsed: &[ParsedFile]) -> HashSet<String> {
    parsed
        .iter()
        .flat_map(|pf| pf.history_target_agent_names())
        .map(String::from)
        .collect()
}

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
struct EmitUnitCtx {
    imported_methods: HashMap<String, Vec<FnSig>>,
    /// The workers-mode-rewritten view is the only one `emit_unit` reads —
    /// the pre-rewrite table is an intermediate of computing it, not exposed
    /// separately.
    imported_decl_paths_emit: HashMap<String, HashMap<String, PathBuf>>,
    exports_for_consumed: HashMap<String, HashMap<String, Visibility>>,
    file_decl_index: FileDeclIndex,
    /// #1820: a context split across files, built for Workers. Each file is
    /// emitted as its own module ([`worker_file_source_path`]) and
    /// `handlers.ts` re-exports them ([`emit_worker_handlers_barrel`]), rather
    /// than every file being written to `handlers.ts` in turn, each
    /// overwriting the last. `file_decl_index` then names each declaration's
    /// module, so one file imports what it uses from another.
    split_worker: bool,
}

fn build_emit_unit_ctx(
    name: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
    hidden_types: &BTreeMap<String, String>,
    target: BuildTarget,
    tys: &Arc<Types>,
    split_worker: bool,
) -> EmitUnitCtx {
    let info = &unit_info[name];
    // v0.132.1 (#481): gather the attached methods of every `uses`-imported type
    // (one level, matching the symbol-table merge). `emit_context_rebrands`
    // forwards these onto the consumer's rebranded const so a call like
    // `Cents.fromInt(n)` type-checks. Sorted by method name for deterministic
    // emission (the resolver stores instance/static methods in `HashMap`s).
    //
    // P6.18: each method's own `params`/`return_type` now resolve to a real
    // `TyId` (`bynk_lower::lower_attached_fn_sig_ir_from_types`) against the
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
        for (type_name, mt) in &used.table.methods {
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
            (
                t.clone(),
                unit_info
                    .get(t)
                    .map(|i| i.exports.clone())
                    .unwrap_or_default(),
            )
        })
        .collect();

    // In workers mode, rewrite imported_decl_paths for consumed
    // contexts to point at the consumed Worker's handlers.ts.
    let mut imported_decl_paths_emit = imported_decl_paths.clone();
    if matches!(target, BuildTarget::Workers) {
        for (unit, decls) in imported_decl_paths.iter() {
            let target_kind = unit_info.get(unit).map(|i| i.kind);
            if target_kind == Some(UnitKind::Context) {
                let handlers_path = worker_handlers_source_path(unit);
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
        let module = |p: &mut PathBuf| *p = worker_file_source_path(name, p);
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
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_unit(
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
    agent_deps_plan: Option<&AgentDepsPlan>,
    compiled: &mut Vec<StagedFile>,
    schema_effective_versions: &SchemaVersions,
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
        worker_file_source_path(name, &pf.source_path())
    } else if workers_mode && kind == UnitKind::Context {
        worker_handlers_source_path(name)
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
    let mut extra_import_lines: Vec<String> = agent_deps_plan
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
        extra_import_lines.push(format!(
            "import {{ render as __bynkLocaleRender, renderArg as __bynkRenderArg }} from \"{import}\";\n\
             import type {{ LocaleTag as __bynkLocaleTag, Message as __bynkMessage, MessageArg as __bynkMessageArg }} from \"{types_import}\";"
        ));
    }

    let agent_called_contexts: Vec<String> =
        emitter::agent_cross_context_targets(&info.table, |c| cross_context_info.resolve_prefix(c))
            .into_iter()
            .collect();
    let emit_ctx = EmitProjectCtx {
        source_path: emit_source_path,
        identity_path: pf.identity_path().to_string_lossy().into_owned(),
        commons_name: name.to_string(),
        file_decl_index: unit_ctx.file_decl_index.clone(),
        imported_from: imported_from.clone(),
        imported_from_kind: imported_from_kind.clone(),
        imported_decl_paths: unit_ctx.imported_decl_paths_emit.clone(),
        unit_kind: kind,
        owning_context: owning_context_for_emit.clone(),
        exports_for_consumed: unit_ctx.exports_for_consumed.clone(),
        imported_methods: unit_ctx.imported_methods.clone(),
        cross_context: cross_context_info,
        target,
        local_agents: info.table.agents.keys().cloned().collect(),
        agent_given_deps: agent_deps_plan.map(|p| p.exprs.clone()).unwrap_or_default(),
        extra_import_lines,
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
        history_target_agents: history_target_agents.clone(),
        asset_served: asset_served.clone(),
        embed_text: embed_text.clone(),
        runtime_use: Default::default(),
    };
    // v0.72: the map's `source` is the absolute path the compiler read the file
    // from, so an editor breakpoint set on the real `.bynk` resolves to the same
    // path the debugger loads (project-relative would resolve against the output
    // `.ts`'s directory — the wrong place). Synthetic units fall back to relative.
    let source_name = pf.map_source_name();
    // #1486: `emit_project` now returns the real `TsProgram` (printed once
    // at the write boundary via `Document::text`) plus the source map
    // `bynk_ts::print` already computed while building it — no more
    // `Verbatim`-wrapping a pre-rendered string.
    // #1927: a `uses`-imported type this unit hides behind its own value is
    // spelled as an inline import type while the module is emitted
    // (`emitter::type_renames`).
    let hidden_type_spellings: HashMap<String, String> =
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
                let spec = emitter::cross_commons_import_specifier_for_path(
                    &emit_ctx.source_path,
                    &path,
                    import_ext,
                );
                let spelling = format!("import(\"{spec}\").{type_name}");
                (type_name, spelling)
            })
            .collect();
    let _spellings = emitter::type_renames::enter(hidden_type_spellings);
    let (program, source_map) =
        emitter::emit_project(program, &emit_ctx, pf.source(), &source_name);
    // Slice 3: the handler-label sidecar for this unit (ADR 0105) — names stack
    // frames by their Bynk operation. `None` for units with no handlers.
    let debug_metadata = emitter::collect_handler_labels(typed);
    let output_path = if unit_ctx.split_worker {
        ts_output_path(&worker_file_source_path(name, &pf.source_path()))
    } else if workers_mode && kind == UnitKind::Context {
        worker_handlers_output_path(name)
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
fn check_unit_files(
    name: &str,
    kind: UnitKind,
    // #1710: each unit's declarations recovery skipped (`phase_parse`).
    broken: &project_model::BrokenDeclNames,
    indices: &[usize],
    parsed: &[ParsedFile],
    unit_info: &BTreeMap<String, UnitInfo>,
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
    agent_deps_plan: Option<&AgentDepsPlan>,
    history_target_agents: &HashSet<String>,
    asset_served: &BTreeMap<String, String>,
    embed_text: &BTreeMap<String, String>,
    mode: Mode,
    policy: &Arc<bynk_check::stability::StabilityPolicy>,
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
    );
    let first_emitted = compiled.len();
    let check_ctx = prepare_unit_check_ctx(
        name,
        kind,
        broken,
        unit_info,
        combined_types,
        imported_from_kind,
        reached_types,
        policy,
    );

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
    errors.extend_attributed_by_span(parsed, plan_errors);
    if plan_failed || mode == Mode::Analyse {
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
        // two boolean-ish questions — a real `String`/`Arc` cost on a large
        // project with nothing reading the rest yet. Widen this filter (or
        // drop it) the moment a future reader needs a different variant.
        unit_callees.extend(program.program().callees.iter().filter_map(|(id, c)| {
            let keep = match c {
                bynk_check::checker::Callee::Cross { .. } => true,
                bynk_check::checker::Callee::Capability { cap, op, .. } => {
                    cap == "Events" && op == "emit"
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
        // `bynk_lower::lower_event_subscriber_shapes_ir`, an excluded file.
        event_subscriber_shapes.extend(bynk_lower::lower_event_subscriber_shapes_ir(&program));
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
            agent_deps_plan,
            compiled,
            schema_effective_versions,
        );
    }
    if mode == Mode::Build && split_worker {
        let barrel = emit_worker_handlers_barrel(
            name,
            indices,
            parsed,
            &compiled[first_emitted..],
            import_ext,
        );
        compiled.push(barrel);
    }
}

/// #1820: `handlers.ts` for a context split across files, on Workers. It
/// re-exports each file's module, so the Worker's entry point, its composition
/// root and every consuming context see the context as one module, as they do
/// a single-file context's `handlers.ts`. Like a multi-file unit's test barrel
/// (`emit_commons_barrel`), it also defines the merged helpers each file
/// generates for itself (`merged_unit_helpers`), which `export *` alone
/// would leave ambiguous: only `__resetAgents`, since a Worker's files export
/// no surface.
///
/// Each file also emits its own copy of what its declarations need from
/// outside it (a boundary type's codecs, a `uses`d commons type's rebrand), so
/// two files can both export one name; the barrel re-exports each such name
/// from one module of `emitted`, this unit's (`duplicated_export_reexports`).
fn emit_worker_handlers_barrel(
    name: &str,
    indices: &[usize],
    parsed: &[ParsedFile],
    emitted: &[StagedFile],
    import_ext: ImportExt,
) -> StagedFile {
    let barrel_loc = worker_handlers_source_path(name);
    let module_of = |file: &Path| worker_file_source_path(name, file);
    let mut modules: Vec<PathBuf> = indices
        .iter()
        .map(|&i| module_of(&parsed[i].source_path()))
        .collect();
    modules.sort();
    modules.dedup();
    let mut stmts = vec![TsStmt::comment(
        "Generated by bynkc — do not edit by hand.",
        None,
    )];
    for module in &modules {
        let spec =
            emitter::cross_commons_import_specifier_for_path(&barrel_loc, module, import_ext);
        stmts.push(TsStmt::decl(TsDecl::ReExportAll { from: spec }, None));
    }
    let (merged, defined) = merged_unit_helpers(
        name,
        indices,
        parsed,
        &barrel_loc,
        import_ext,
        module_of,
        false,
    );
    stmts.extend(duplicated_export_reexports(
        &modules,
        emitted,
        &defined,
        &barrel_loc,
        import_ext,
    ));
    stmts.extend(merged);
    StagedFile {
        output_path: worker_handlers_output_path(name),
        document: Document::Ts(TsProgram { stmts }),
        source_map: None,
        debug_metadata: None,
    }
}

/// The outcome of the shared check pipeline (regions 1+2's shared work),
/// before whichever caller applies its own divergent exit — `compile_project`/
/// `compile_in_memory` project it into a `Result<ProjectOutput, ProjectFailure>`
/// via `finish_build`; `check_project`/`analyse_in_memory_with_types` read
/// straight off it instead. #1541 deleted the fifth of these five callers
/// (`analyse_project_with`, which alone read `hints`/`requirements`/`refs`
/// back out) along with those three fields — they're still populated during
/// checking (the shared `bynk_check` phase functions this file calls need the
/// sinks regardless of who reads them after), just no longer carried out on
/// this enum.
#[allow(clippy::large_enum_variant)]
enum RunChecks {
    /// Discovery/parse failed, or (build mode) the structural gate bailed:
    /// only diagnostics, no checked program. Index is not assembled here.
    Bailed {
        errors: ErrorSink,
        snapshots: Vec<(PathBuf, String)>,
        locals: LocalsSink,
        exprs: ExprTypeSink,
    },
    /// All phases ran (per-unit checks + tests + platform-lock done).
    Checked {
        errors: ErrorSink,
        snapshots: Vec<(PathBuf, String)>,
        locals: LocalsSink,
        exprs: ExprTypeSink,
        parsed: Vec<ParsedFile>,
        compiled: Vec<StagedFile>,
        runnable_tests: Vec<RunnableTest>,
        integration_outputs: Vec<StagedFile>,
        integration_runnables: Vec<RunnableTest>,
        groups: BTreeMap<String, Vec<usize>>,
        kinds: BTreeMap<String, UnitKind>,
        unit_uses: HashMap<String, Vec<String>>,
        unit_consumes: HashMap<String, Vec<String>>,
        unit_consumes_aliases: HashMap<String, HashMap<String, String>>,
        unit_tables: HashMap<String, UnitTable>,
        // #2252: per context, the types it takes from a consumed context —
        // the names whose codecs `emit_boundary_helpers` emits under the same
        // rule — so the entry validates one through that codec.
        unit_consumed_context_types: HashMap<String, HashSet<String>>,
        // #1187's slice 6 plumbing: each unit's own `Callee` classification,
        // merged across its files (`ExprId` is a single project-wide
        // counter, `project_model.rs`'s `next_expr_id`, so merging different
        // files' maps never collides) — checked, resolved data the pre-check
        // `unit_tables` above cannot carry. Exists so a later, project-wide
        // pass (`build_output`/`emit_composition_root`) can read an
        // already-resolved `Callee::Capability`/`Callee::Cross` instead of
        // re-deriving the same fact by walking raw AST method-call syntax —
        // `check_unit_files`'s own per-file `CheckedProgram` was previously
        // built and dropped before any such later pass ever ran. Filtered at
        // merge time (`check_unit_files`'s own `unit_callees.extend` call,
        // review of #1202) to only `Callee::Cross` and
        // `Callee::Capability{cap:"Events",op:"emit"}` — the two variants
        // `unit_table_uses_emit`/`called_cross_context_services` actually
        // read today; widen the filter (or drop it) the moment a future
        // reader needs a different variant, rather than paying to retain
        // every call site's full classification project-wide for the rest
        // of the build on spec.
        unit_callees: HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
        // P6.x (#1232): each unit's own declared event-subscriber service
        // shapes, merged across its files — see `EventSubscriberShape`'s own
        // doc comment. Threaded the same way as `unit_callees` immediately
        // above.
        unit_event_subscriber_shapes: HashMap<String, HashMap<String, EventSubscriberShape>>,
        unit_flattened: HashMap<String, HashMap<String, String>>,
        adapter_bindings: HashMap<String, AdapterBinding>,
        npm_deps: std::collections::BTreeMap<String, String>,
        target: BuildTarget,
        // Events track, slice 3c (#980): the reconciled registry document,
        // ready for `finish_build` to serialize onto
        // `ProjectOutput::schema_lock`. `None` when `schema_registry` was
        // `SchemaLock::Off` (#1078) — nothing for a caller to persist.
        schema_registry: Option<schema_registry::SchemaRegistry>,
        // #1883, slice 1b: the bytes of each declared asset file, by
        // normalised path ([`bynk_check::assets::read_declared_files`]), for
        // the Workers' files. `None` when no folder was listed.
        asset_bytes: Option<BTreeMap<String, Vec<u8>>>,
        // P4b (#2608): each `D1` database's migrations, copied beside its
        // Worker's `wrangler.toml`.
        d1_migrations: bynk_check::d1_migrations::D1Migrations,
        // State-migrations S4 (#2259): each agent class's reconciled versions
        // and valid steps, keyed `<unit>.<Agent>`, for `bynk migrate status`.
        // Empty when the build has no lock.
        agent_versions: HashMap<String, bynk_check::schema_registry::AgentVersions>,
    },
}

#[allow(clippy::too_many_arguments)]
fn run_checks(
    // R3.9 (#1113): one `(root, prefix)` pair per `Roots::trees` entry, not a
    // hardcoded primary/secondary pair — every `include` tree is walked, not
    // just the first one or two.
    trees: &[(PathBuf, PathBuf)],
    target: BuildTarget,
    platform: Platform,
    import_ext: ImportExt,
    mode: Mode,
    overlay: &HashMap<PathBuf, String>,
    // v0.113: absolute subtrees to skip during discovery (author `exclude` plus
    // the tool's `out`/`node_modules` caches). Empty for in-memory builds.
    excludes: &[PathBuf],
    // v0.108 (in-browser track, slice 3): when `Some`, the source files are
    // supplied directly, one file list per `trees` entry — and filesystem
    // discovery is skipped. The wasm/REPL entry feeds an in-memory
    // single-module project this way (the source itself rides in `overlay`);
    // `None` keeps the on-disk discovery walk for the CLI and the LSP.
    discovered: Option<Vec<Vec<PathBuf>>>,
    // v0.115: emit the function-contract call-site guard (dev/test profile).
    contracts: bool,
    // #1821 (ADR 0147 D3): check and emit `suite`s. Off for a deployable build.
    tests: bool,
    // Events track, slice 3c (#980): `On` turns on `bynk.schema.lock`
    // reconciliation, with its pre-read content; `Off` (every in-memory/
    // test/LSP caller) skips it entirely. See `CompileOptions::schema_registry`
    // and `SchemaLock`. #1078: no disk access here — the caller pre-reads.
    schema_registry: &SchemaLock,
    // #2138: a remote test build (`CompileOptions::remote_tests`); `None`
    // for every other build.
    remote_tests: Option<&RemoteTests>,
    // #1085 review: only for `schema_registry::parse`'s corruption message —
    // naming *which* project's lock file is corrupt, now that #1078 made
    // `bynk-emit` disk-free (and so path-blind) for this file.
    project_root: &Path,
    // #1883, slice 1b: the listed `assets/` folder
    // (`CompileOptions::asset_files`). `None` when no filesystem was
    // consulted: a check then skips the existence check, a build treats it as
    // empty.
    asset_files: Option<&assets::AssetFolder>,
    // #2387, slice 2: the listed `embeds/` folder
    // (`CompileOptions::embed_files`), with the same `None` meaning.
    embed_files: Option<&assets::EmbedFolder>,
    // P4b (#2608): the listed `migrations/` folder
    // (`CompileOptions::migration_files`). `None` when no filesystem was
    // consulted: nothing is checked, and no test database is migrated.
    migration_files: Option<&assets::AssetFolder>,
    // #2223: resolve `project_root`'s path dependencies and fold them in
    // (`bynk_check::packages::apply_graph`). Only a manifest-backed project
    // has any; an in-memory build never does.
    packages: bool,
    tys: &Arc<Types>,
) -> RunChecks {
    let mut errors = ErrorSink::new();
    // S3 (#1963): the project's `[unstable]` opt-in, read as the editor reads it
    // (`bynk_check::analysis::analyse_project`), so the two never disagree.
    let policy = Arc::new(bynk_check::stability::StabilityPolicy::for_project(
        Some(project_root),
        overlay,
    ));
    for err in policy.manifest_diagnostics() {
        errors.push_for(Some(Path::new(bynk_check::stability::MANIFEST)), err);
    }
    // v0.25 (ADR 0053): binding edges, recorded at the resolution sites — by
    // `check_file_core` and several `project_model::phase_*` resolution
    // calls below, all of which require this sink unconditionally.
    // `bynk_check::analysis::analyse_project` threads the same sinks through
    // its own call to those functions and does read its copy back out (into
    // `ProjectAnalysis.index`, via `assemble_index`). Here, #1541 deleted the
    // one caller (`analyse_project_with`) that drained this copy back out via
    // `RunChecks`, so it's populated and discarded — never surfaced by this
    // enum anymore.
    let mut refs = RefSink::new();
    // v0.27 (ADR 0056): inferred-type inlay hints, recorded at the checker's
    // binding sites. A sink (not part of the checker's Ok payload) so hints
    // survive the per-file error-`continue`s. Same #1541 residue as `refs`:
    // populated, never read back out of `RunChecks`.
    let mut hints = HintSink::new();
    let mut locals = LocalsSink::new();
    // v0.99: the capability-requirement ledger — recorded at the checker's
    // capability-consuming sites. Same #1541 residue as `refs`/`hints`.
    let mut requirements = RequirementSink::new();
    // v0.30.2 (ADR 0063): per-file expression types, so `.`-member completion
    // can type a receiver — on the error paths too, as best-effort partial
    // types (ADR 0094). Carried like `hints`.
    let mut exprs = ExprTypeSink::new();
    let mut snapshots: Vec<(PathBuf, String)> = Vec::new();

    // -- 1. Discovery (skipped when sources are supplied in memory). --
    let file_lists = match discovered {
        Some(files) => files,
        None => match project_model::phase_discovery(trees, excludes, &mut errors) {
            Ok(files) => files,
            Err(()) => {
                return RunChecks::Bailed {
                    errors,
                    snapshots,
                    locals,
                    exprs,
                };
            }
        },
    };
    // #1077/#1081 review: `no_sources`/file-directory-conflict checks run on
    // every tree's file list regardless of provenance — see
    // `check_discovered_files`'s own doc.
    if project_model::check_discovered_files(trees, &file_lists, &mut errors).is_err() {
        return RunChecks::Bailed {
            errors,
            snapshots,
            locals,
            exprs,
        };
    }

    // -- 2. Parse every file. --
    let (mut parsed, injected, broken) = match project_model::phase_parse(
        trees,
        &file_lists,
        overlay,
        &mut errors,
        &mut snapshots,
    ) {
        Ok(out) => out,
        Err(()) => {
            return RunChecks::Bailed {
                errors,
                snapshots,
                locals,
                exprs,
            };
        }
    };
    // -- 2a. #2223: fold the project's path dependencies in. --
    if packages {
        bynk_check::packages::apply_graph(
            Some(project_root),
            overlay,
            &mut parsed,
            tests,
            &mut errors,
        );
    }

    // -- 2b. Normalize service-level `by`/`given` defaults (v0.155). A service
    //        header default is injected into every handler that omits its own
    //        clause, so every downstream phase (grouping, checking, validation,
    //        emission) reads canonical handlers with no special-casing. `parsed`
    //        is indexed (not cloned) by later phases, so mutating it here reaches
    //        them all. The parsed AST that `bynk fmt` produces is untouched (it
    //        parses independently), so the terse inheriting source round-trips.
    project_model::normalize_service_defaults(&mut parsed);
    // #1821 (ADR 0147 D3): a deployable build strips every suite, here, right
    // after parsing, so no later pass (the reserved-namespace and
    // function-type checks walk every parsed unit) sees one: never
    // type-checked for the build, never emitted. A file holding a unit and
    // its suite is two parsed units, so the unit stays. A file that does not
    // parse at all has already failed above: it cannot be told apart from
    // production source.
    if !tests {
        parsed.retain(|pf| !matches!(pf.kind(), UnitKind::Test | UnitKind::Integration));
    }
    let parsed = parsed;

    // -- 3. Group by (name, kind) and validate per-directory consistency.
    //       P5.2 (`design/tracks/semantics-in-the-checker.md` §6):
    //       `phase_group` now also confines function types to non-boundary
    //       positions directly, at the point its old optional hook used to
    //       fire — see that function's own doc comment. --
    let (groups, kinds, test_groups, integration_groups, adapter_bindings, npm_deps) =
        project_model::phase_group(&parsed, trees, platform, &injected, overlay, &mut errors);
    // #2207 (packaging slice 1, decision D): `[package] exports` names only
    // commons and adapters this project declares — read as the editor reads it
    // (`bynk_check::analysis::analyse_project`).
    for err in bynk_check::exports::manifest_diagnostics(Some(project_root), overlay, &kinds) {
        errors.push_for(Some(Path::new(bynk_check::stability::MANIFEST)), err);
    }
    // -- 4. Build per-unit combined symbol tables. --
    let unit_tables = project_model::phase_symbol_tables(&groups, &kinds, &parsed, &mut errors);

    // -- 5. Resolve `uses` clauses (target must exist + be a commons). --
    let unit_uses = project_model::phase_resolve_uses(
        &groups,
        &kinds,
        &parsed,
        &unit_tables,
        &policy,
        &mut errors,
    );

    // -- 5b. Resolve `consumes` clauses (target must exist + be a context). --
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
    bynk_check::symbols::record_flattened_caps(&mut unit_tables, &unit_flattened);
    bynk_check::project_model::phase_resolve_derived(
        &groups,
        &kinds,
        &parsed,
        &mut unit_tables,
        &mut errors,
    );
    let unit_tables = unit_tables;

    // -- 5b'. Collect `consumes` aliases (v0.6 §3.1). Each consuming context
    //         has an alias map: alias → consumed-context qualified name.
    //         Detect alias-alias conflicts here; alias-vs-local-decl conflicts
    //         are checked once the local symbol tables are built (step 6+).
    let unit_consumes_aliases =
        project_model::phase_consumes_aliases(&groups, &kinds, &parsed, &unit_tables, &mut errors);

    // -- 5b''. v0.173 (ADR 0196 D1): warn where a `bynk.Secrets` read names its
    //          secret with a computed expression. P5.5
    //          (`design/tracks/semantics-in-the-checker.md` §6, §9): relocated
    //          to `bynk_check::project_model::phase_secrets_computed_name` —
    //          this is now a caller, not an owner, the same move as this
    //          function's neighbours above. See that function's own doc for
    //          why raising it here no longer reaches the editor by itself.
    project_model::phase_secrets_computed_name(
        target,
        &parsed,
        &groups,
        &kinds,
        &unit_flattened,
        &mut errors,
    );
    // #1822: on Workers, a required local capability needs a provider.
    project_model::phase_workers_unprovided_capabilities(
        target,
        &parsed,
        &groups,
        &kinds,
        &unit_tables,
        &mut errors,
    );
    // #2306: on Workers, a WebSocket `on open` and the agent it transfers to
    // must share a file.
    project_model::phase_workers_ws_open_split(
        target,
        &parsed,
        &groups,
        &kinds,
        &unit_tables,
        &mut errors,
    );
    // #2355 follow-up: on Workers, a capability whose provider constructs
    // another context's agents cannot be built by this context's Worker.
    project_model::phase_workers_provider_agent_cross_context(
        target,
        &parsed,
        &groups,
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_flattened,
        &mut errors,
    );

    // -- 5c. Detect `consumes` cycles. --
    project_model::phase_detect_consumes_cycles(&groups, &parsed, &unit_consumes, &mut errors);

    // -- 6. Name-conflict detection for uses imports (commons-only check). --
    project_model::phase_uses_name_conflicts(
        &unit_uses,
        &unit_tables,
        &parsed,
        &groups,
        &mut errors,
    );
    // -- 6.1. #2618: a public signature naming a private type. --
    project_model::phase_private_in_public(&parsed, &groups, &mut errors);

    // -- 6a0. #1883, slice 1b: every declared asset names a file in the
    //         `assets/` folder. A build with no folder read checks against an
    //         empty one; a check with none skips this. Then the declared files
    //         alone are read, through the folder's reader.
    let no_assets = BTreeSet::new();
    let available_assets: Option<&BTreeSet<String>> = match (asset_files, mode) {
        (Some(folder), _) => Some(folder.files()),
        (None, Mode::Build) => Some(&no_assets),
        (None, _) => None,
    };
    bynk_check::assets::phase_asset_files(&parsed, available_assets, &mut errors);
    let asset_bytes = asset_files
        .map(|folder| bynk_check::assets::read_declared_files(&parsed, folder, &mut errors));
    // -- 6a0'. #2387, slice 2: likewise every declared embed names a file in
    //          `embeds/`, then the declared files alone are read, as text.
    let no_embeds = BTreeSet::new();
    let available_embeds: Option<&BTreeSet<String>> = match (embed_files, mode) {
        (Some(folder), _) => Some(folder.files()),
        (None, Mode::Build) => Some(&no_embeds),
        (None, _) => None,
    };
    bynk_check::embeds::phase_embed_files(&parsed, available_embeds, &mut errors);
    let embed_text = embed_files
        .map(|folder| bynk_check::embeds::read_declared_embeds(&parsed, folder, &mut errors))
        .unwrap_or_default();
    // -- 6a0''. P4b (#2608): each `D1` instance's migrations directory,
    //           checked and read; a directory no instance owns is a warning.
    let d1_migrations = bynk_check::d1_migrations::phase_d1_migrations(
        migration_files,
        &bynk_check::d1_migrations::declared_databases(&kinds, &unit_tables),
        &mut errors,
    );
    // #1883, slice 2: fixed asset paths against each Worker's routes.
    bynk_check::assets::phase_asset_routes(&parsed, &groups, &kinds, &unit_uses, &mut errors);
    // A deployable Bundle build (no tests: `bynkc compile` without
    // `--target workers`) ships no files, so each asset's `.url` would 404:
    // say so. `bynkc test` also builds a Bundle, with tests, and stays quiet.
    if mode == Mode::Build && target == BuildTarget::Bundle && !tests {
        for pf in &parsed {
            for warning in bynk_check::assets::bundle_not_served_warnings(pf.items()) {
                errors.push_for(Some(&pf.identity_path()), warning);
            }
        }
    }

    // -- 6a'. message-bundles slice 1 (#859): messages-block legality,
    //         @reference cardinality, within-block duplicate codes, and the
    //         `uses bynk.locale` dependency. Runs here (not in phase_group)
    //         because it needs `unit_uses`, resolved just above.
    //
    //         P5.0 (#1128, `design/tracks/semantics-in-the-checker.md` §6):
    //         relocated to `bynk-check::project_model` alongside the rest of
    //         this pipeline (P4.1's own move) — this is now a caller, not an
    //         owner, the same way P4.0/P4.1 turned this function into a
    //         caller of `bynk-project`/`bynk-check`.
    project_model::phase_messages_bundles(&parsed, &groups, &kinds, &unit_uses, &mut errors);

    // -- 6a''. Locale capability track, slice 2 (#882): a context reaching
    //          two or more message-bundle commons while consuming `Locale`
    //          has no single bundle to negotiate against. P5.0: relocated,
    //          see above.
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
    //
    //           P5.1 (#1130, `design/tracks/semantics-in-the-checker.md` §6):
    //           relocated to `bynk-check::project_model`, same move as
    //           P5.0's neighbours above.
    project_model::phase_event_subscriptions(
        &parsed,
        &groups,
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_uses,
        &mut errors,
    );

    // -- 6b. Validate exports clauses (each name is a locally-declared type;
    //         no duplicates within or across opaque/transparent). --
    let exports_visibility = project_model::phase_validate_type_exports(
        &groups,
        &kinds,
        &parsed,
        &unit_tables,
        &mut errors,
        &mut refs,
    );

    // -- 6b'. Validate `exports capability { … }` clauses (v0.15 §4.1): each
    //          name must be a capability the context declares *and* provides. --
    project_model::phase_validate_capability_exports(
        &groups,
        &kinds,
        &parsed,
        &unit_tables,
        &mut errors,
        &mut refs,
    );

    // -- 6c. Validate that providers match their capabilities exactly. --
    project_model::phase_validate_providers(
        &unit_tables,
        &groups,
        &parsed,
        &broken,
        &mut errors,
        tys,
    );

    // -- 6d. Events track, slice 3c (#980): reconcile every event's shape
    //        against the committed schema registry. `schema_registry` is
    //        `SchemaLock::Off` for every in-memory/test/LSP/fixture caller
    //        (opt-in — see `CompileOptions::schema_registry`'s doc), in which
    //        case this is a no-op and every event falls back to today's
    //        `@schema(N)`-or-`1` behaviour. Must run before the per-unit loop
    //        below: `emit_unit` needs `schema_effective_versions` to mint the
    //        right `schemaVersion`, and by the time `RunChecks` reaches
    //        `finish_build` the TypeScript is already emitted. Only the
    //        *write* is deferred — to the caller, gated on a fully clean
    //        build (#1078: `bynk-emit` computes, never writes) — reconciliation
    //        itself happens here. P5.3: `reconcile` itself now lives in
    //        `bynk_check::schema_registry` (this crate is a caller, not an
    //        owner) — `SchemaRegistry` stays re-exported from this crate's
    //        own `schema_registry` module. P5.5: the corrupt-file diagnostic
    //        moved too — `bynk_check::schema_registry::parse_or_diagnose` is
    //        now this crate's caller-side of both the parse and the
    //        `bynk.project.schema_registry_corrupt` construction (§3.2's
    //        "eighth site").
    let mut schema_effective_versions = SchemaVersions::default();
    let mut schema_registry_doc: Option<schema_registry::SchemaRegistry> = None;
    if let SchemaLock::On { existing } = schema_registry {
        match bynk_check::schema_registry::parse_or_diagnose(existing.as_deref(), project_root) {
            Ok(existing_reg) => {
                let mut schema_errors: Vec<CompileError> = Vec::new();
                let (mut updated, effective) = bynk_check::schema_registry::reconcile(
                    &existing_reg,
                    &unit_tables,
                    &mut schema_errors,
                );
                errors.extend_for(None, schema_errors);
                // State-migrations S0 (#2204): agent state shapes, in the same
                // document. Attributed by span, so a refused change lands on
                // the agent's own file.
                let mut agent_errors: Vec<CompileError> = Vec::new();
                schema_effective_versions.agents = bynk_check::schema_registry::reconcile_agents(
                    &existing_reg,
                    &mut updated,
                    &unit_tables,
                    &unit_uses,
                    &mut agent_errors,
                );
                errors.extend_attributed_by_span(&parsed, agent_errors);
                schema_effective_versions.events = effective;
                schema_registry_doc = Some(updated);
            }
            Err(err) => {
                errors.push_for(None, err);
            }
        }
    }

    if !errors.is_empty() && mode == Mode::Build {
        return RunChecks::Bailed {
            errors,
            snapshots,
            locals,
            exprs,
        };
    }

    // -- 7. Build per-unit file index (which file declares which name). --
    let unit_file_index = project_model::phase_file_index(&groups, &parsed);

    // -- 7b (v0.29.4). Assemble the nine parallel per-unit maps into one record
    //          per unit. Driven by the `groups` keyset (the authority), so every
    //          group yields exactly one `UnitInfo` with all facets present. The
    //          producer maps are cloned, not moved, because the back half of the
    //          pipeline (tests, integration tests, platform-lock, composition
    //          root, the workers branch) still reads the originals.
    let unit_info = project_model::assemble_unit_info(
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

    // -- 8. For each unit, build the combined symbol space and run
    //       resolve+check per source file. --
    let mut compiled: Vec<StagedFile> = Vec::new();
    // #1187's slice 6 plumbing (see `RunChecks::Checked::unit_callees`'s own
    // doc comment) — one `Callee` map per unit, merged across that unit's
    // own files inside the loop below.
    let mut unit_callees: HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>> =
        HashMap::new();
    // P6.x (#1232): one `EventSubscriberShape` map per unit, merged across
    // that unit's own files inside the loop below — same shape as
    // `unit_callees` immediately above.
    let mut unit_event_subscriber_shapes: HashMap<String, HashMap<String, EventSubscriberShape>> =
        HashMap::new();
    let mut unit_consumed_context_types: HashMap<String, HashSet<String>> = HashMap::new();
    // #2719: the needs-context set, extended unit by unit in dependency order.
    let mut exec_plan = bynk_check::exec_plan::ExecPlan::default();

    // v0.119 (testing track slice 7, ADR 0155): a project-wide fold over every
    // parsed file, producing the identical `HashSet` regardless of which unit
    // or file is currently emitting — computed once here rather than once per
    // emitted file (`collect_history_target_agents` used to be called from
    // inside the per-file emit prologue).
    let history_target_agents = collect_history_target_agents(&parsed);
    // #1883, slice 1b: each declared asset's served URL path, by normalised
    // source path — what its `.url` constant holds. Empty when no folder was
    // read (a missing file is already `bynk.asset.missing_file`).
    let asset_served = asset_bytes
        .as_ref()
        .map(|files| bynk_check::assets::declared_served_paths(&parsed, files))
        .unwrap_or_default();

    // #1702 review: `uses` targets first, so a generic callee's compared type
    // parameters are known before an importer's calls are checked.
    bynk_check::checker::reset_compared_cache();
    // #2137: the contexts a `system` harness stands up as Workers; their
    // test-build `Clock` reads the harness clock. Empty in a deployable build,
    // which carries no suites.
    let harness_clock_contexts = if parsed.iter().any(|p| p.integration().is_some()) {
        let tables: HashMap<String, UnitTable> = unit_info
            .iter()
            .map(|(n, i)| (n.clone(), i.table.clone()))
            .collect();
        let consumes: HashMap<String, Vec<String>> = unit_info
            .iter()
            .map(|(n, i)| (n.clone(), i.consumes.clone()))
            .collect();
        harness_clock_contexts(&parsed, &tables, &consumes, &kinds)
    } else {
        HashSet::new()
    };
    for name in bynk_check::project_model::uses_first_order(unit_info.keys(), &unit_uses) {
        let info = &unit_info[name];
        let kind = info.kind;
        let indices = info.files.as_slice();
        let local_table = &info.table;
        // v0.24: skip resolve/check only when THIS group's composition
        // failed. In build mode the sink is empty here (the structural gate
        // bailed), so the delta equals the old global is_empty check; in
        // analyse mode one broken unit no longer suppresses every other
        // unit's semantic diagnostics.
        let group_error_baseline = errors.len();

        let (
            mut combined_types,
            combined_fns,
            mut combined_methods,
            mut imported_from,
            mut imported_from_kind,
        ) = project_model::compose_unit_symbols(name, local_table, &unit_info);
        let consumed_types = project_model::merge_consumed_exports(
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
        let reached_types = project_model::close_reachable_types(
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

        // #2252: the rule `emit_boundary_helpers` roots a consumed type's
        // codecs by (#2036): imported from a context, and not declared here.
        if kind == UnitKind::Context {
            let from_contexts = imported_from_kind
                .iter()
                .filter(|(n, k)| **k == UnitKind::Context && !local_names.contains(*n))
                .filter(|(n, _)| {
                    imported_from
                        .get(*n)
                        .and_then(|owner| unit_info.get(owner))
                        .is_some_and(|i| i.kind == UnitKind::Context)
                })
                .map(|(n, _)| n.clone())
                .collect();
            unit_consumed_context_types.insert(name.clone(), from_contexts);
        }

        let local_methods_for_type = project_model::collect_unit_methods(indices, &parsed);

        // Per-context view information for the emitter and checker.
        let owning_context_for_emit = if kind == UnitKind::Context {
            Some(name.clone())
        } else {
            None
        };

        // #527: workers contexts get a DO-side deps plan for their agents'
        // `given` capabilities (the wire cannot carry providers).
        let agent_deps_plan = if matches!(target, BuildTarget::Workers) && kind == UnitKind::Context
        {
            plan_agent_given_deps(
                name,
                &unit_info,
                &adapter_bindings,
                indices.len() > 1,
                harness_clock_contexts.contains(name),
            )
        } else {
            None
        };

        check_unit_files(
            name,
            kind,
            &broken,
            indices,
            &parsed,
            &unit_info,
            &combined_types,
            &combined_fns,
            &combined_methods,
            &local_names,
            &local_methods_for_type,
            &consumed_types,
            &imported_from,
            &imported_from_kind,
            &reached_types,
            &owning_context_for_emit,
            target,
            import_ext,
            contracts,
            agent_deps_plan.as_ref(),
            &history_target_agents,
            &asset_served,
            &embed_text,
            mode,
            &policy,
            &mut errors,
            &mut refs,
            &mut hints,
            &mut locals,
            &mut exprs,
            &mut requirements,
            &mut compiled,
            &schema_effective_versions,
            tys,
            unit_callees.entry(name.clone()).or_default(),
            unit_event_subscriber_shapes
                .entry(name.clone())
                .or_default(),
            &mut exec_plan,
        );
    }

    // #2719: every unit is checked, so the needs-context set is final. Stamp it
    // on each unit's table, which is what the emission gates for service
    // handlers (the Worker entry, `compose`, the Bundle surface, the test
    // harness) already hold.
    let unit_tables: HashMap<String, UnitTable> = {
        let plan = Arc::new(exec_plan);
        unit_tables
            .into_iter()
            .map(|(k, mut t)| {
                t.exec_plan = Arc::clone(&plan);
                (k, t)
            })
            .collect()
    };

    // v0.7: process test declarations. Each `test commerce.X` group resolves
    // its target, validates mocks against the target's capability/consumed-
    // context shapes, type-checks bodies with the target's privileged view,
    // and emits a per-target TypeScript test module under `tests/`.
    let mut test_errors: Vec<CompileError> = Vec::new();
    // v0.132: barrel output paths emitted so far, shared across the unit- and
    // integration-test passes so a multi-file commons imported by both is
    // aggregated into `out/<name>.ts` exactly once.
    let mut emitted_barrels: HashSet<PathBuf> = HashSet::new();
    // #1820: a context split across files emits a module per file and none at
    // `out/<name>.ts`, which the bundle's composition root imports it from.
    // Its barrel is part of the build, not only of a test build; a suite that
    // imports the context reuses it. On workers the context's barrel is its
    // Worker's `handlers.ts` (`emit_worker_handlers_barrel`), and its files
    // aren't at `out/<name>/`, so its path here is claimed and none is
    // emitted: the test passes below emit only a commons' barrel.
    for (name, kind) in &kinds {
        if *kind != UnitKind::Context {
            continue;
        }
        if mode == Mode::Build && target == BuildTarget::Bundle {
            if let Some(barrel) = emit_commons_barrel(
                name,
                &groups,
                &parsed,
                import_ext,
                &mut emitted_barrels,
                &compiled,
            ) {
                compiled.push(barrel);
            }
        } else if target == BuildTarget::Workers {
            emitted_barrels.insert(commons_dir_for(name).with_extension("ts"));
        }
    }
    // #1883, slice 3: the declared assets, for a case's `svc.GET(asset)`.
    let no_asset_bytes = BTreeMap::new();
    let test_asset_files = TestAssetFiles {
        served: &asset_served,
        bytes: asset_bytes.as_ref().unwrap_or(&no_asset_bytes),
    };
    // #1975: a user adapter's binding module, for a test that builds the
    // binding for real. A first-party adapter's binding reaches the platform
    // (the network, a KV namespace), so a test keeps its double. `bynk.crypto`
    // is the exception (K4): its binding is Web Crypto over the test's own
    // bytes, deterministic and I/O-free, so a test runs it for real and a fake
    // would check nothing.
    let test_binding_modules: HashMap<String, String> = adapter_bindings
        .iter()
        .filter(|(name, _)| {
            let first_party = bynk_check::firstparty::BYNK_UNIT;
            name.as_str() == bynk_check::firstparty::CRYPTO_UNIT
                || (name.as_str() != first_party
                    && !name
                        .strip_prefix(first_party)
                        .is_some_and(|rest| rest.starts_with('.')))
        })
        .map(|(name, b)| {
            (
                name.clone(),
                emitter::ts_specifier(&b.output_path.with_extension("")),
            )
        })
        .collect();
    let (test_outputs, runnable_tests) = process_tests(
        &test_groups,
        &parsed,
        &kinds,
        &unit_tables,
        &exports_visibility,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_uses,
        &unit_flattened,
        &groups,
        import_ext,
        contracts,
        &mut emitted_barrels,
        &test_asset_files,
        &d1_migrations,
        &test_binding_modules,
        &schema_effective_versions.events,
        &unit_callees,
        &mut test_errors,
        &mut refs,
        tys,
    );
    // #2138: a remote run runs only the `system` suites; the unit modules are
    // still written (a barrel they emit may be shared) but not listed.
    let runnable_tests = if remote_tests.is_some() {
        Vec::new()
    } else {
        runnable_tests
    };
    // #696/#1659: test-suite diagnostics are attributed to their owning file by
    // each error's span (`ErrorSink::extend_attributed_by_span`), so they render
    // with a file, line and span like every other diagnostic.
    errors.extend_attributed_by_span(&parsed, test_errors);

    compiled.extend(test_outputs);

    // v0.16: process integration tests. Each `test integration "name"` suite
    // validates its `wires` participants, type-checks each case body as a
    // cross-context call from a synthetic harness root that consumes every
    // participant, and emits a TypeScript module that stands the participants
    // up as in-process Workers and exercises the flow across the real wire.
    // #1959: each context's bindings, once: the integration harness binds them
    // on each participant's `env`, and the duplicate check below reads them.
    // Gated like that check: the KV input walks the provider graph.
    let event_subscribers =
        bynk_check::symbols::discover_event_subscribers(&unit_tables, &unit_consumes);
    let context_bindings = errors.is_empty().then(|| {
        worker_bindings::all_context_bindings(
            &kinds,
            &unit_tables,
            &unit_consumes,
            &unit_consumes_aliases,
            &unit_flattened,
            &unit_callees,
            &event_subscribers,
        )
    });
    let mut integration_errors: Vec<CompileError> = Vec::new();
    let (integration_outputs, integration_runnables) = process_integration_tests(
        &integration_groups,
        &parsed,
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_uses,
        &groups,
        &mut emitted_barrels,
        &test_asset_files,
        &d1_migrations,
        context_bindings.as_ref(),
        &event_subscribers,
        remote_tests,
        &mut integration_errors,
        &mut refs,
        tys,
    );
    // #696/#1659: integration-suite diagnostics, attributed the same way.
    errors.extend_attributed_by_span(&parsed, integration_errors);

    // v0.19 (decisions 0017/0024): platform-lock enforcement. A deployment
    // unit whose in-process closure reaches a platform-native capability is
    // locked to that platform; the selected `--platform` must match. Run only
    // on otherwise-clean programs: the closure walk recurses the provider
    // graph, whose acyclicity the earlier checks establish. P5.3: relocated
    // to `bynk_check::project_model::phase_platform_lock` — this crate is a
    // caller, not an owner.
    if errors.is_empty() {
        project_model::phase_platform_lock(
            target,
            platform,
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
    // On Workers, each Worker's binding names are unique. Gated the same way:
    // the KV input walks the same provider graph.
    if errors.is_empty()
        && let Some(bindings) = &context_bindings
    {
        worker_bindings::phase_duplicate_bindings(target, &parsed, &groups, bindings, &mut errors);
    }

    // v0.176 (#642): the `Bytes`-at-a-workers-boundary guard (ADR 0142 D8) is
    // retired here. It existed because the workers boundary carried its own
    // codec dispatch, which cast a `Bytes` to `JsonValue` on the way out while
    // base64-decoding it on the way in — so a `Bytes` mis-round-tripped, and a
    // diagnostic was better than silent corruption. That dispatch is gone: every
    // wire position now routes through `serialisation.rs`, whose `Bytes` arm
    // base64-encodes. The restriction has no remaining cause, and ADR 0142 D8's
    // deferral to "the roadmap's typed cross-context boundary fix" is discharged.

    RunChecks::Checked {
        errors,
        snapshots,
        locals,
        exprs,
        parsed,
        compiled,
        runnable_tests,
        integration_outputs,
        integration_runnables,
        groups,
        kinds,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        unit_tables,
        unit_consumed_context_types,
        unit_callees,
        unit_event_subscriber_shapes,
        unit_flattened,
        adapter_bindings,
        npm_deps,
        target,
        schema_registry: schema_registry_doc,
        asset_bytes,
        agent_versions: schema_effective_versions.agents,
        d1_migrations,
    }
}

/// `<name>.<suffix>`, matching `bynk-driver::output.rs`'s own sibling-name
/// derivation for `.map`/`.bynkdbg.json` files exactly (P7.6, #1309).
pub fn sibling_path(output_path: &Path, suffix: &str) -> PathBuf {
    let name = output_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    output_path.with_file_name(format!("{name}.{suffix}"))
}

/// Build-success tail (region 3): emit the composition/worker/runtime files
/// and assemble the final `ProjectOutput`. Reached only on build mode with a
/// clean error sink. Moved verbatim from the old pipeline; only the locals it
/// reads are now bound from the `Checked` variant.
#[allow(clippy::too_many_arguments)]
fn build_output(
    parsed: Vec<ParsedFile>,
    mut compiled: Vec<StagedFile>,
    mut runnable_tests: Vec<RunnableTest>,
    integration_outputs: Vec<StagedFile>,
    integration_runnables: Vec<RunnableTest>,
    groups: BTreeMap<String, Vec<usize>>,
    kinds: BTreeMap<String, UnitKind>,
    unit_consumes: HashMap<String, Vec<String>>,
    unit_consumes_aliases: HashMap<String, HashMap<String, String>>,
    unit_tables: HashMap<String, UnitTable>,
    unit_consumed_context_types: &HashMap<String, HashSet<String>>,
    unit_callees: HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
    // P6.x (#1232): see `EventSubscriberShape`'s own doc comment.
    unit_event_subscriber_shapes: HashMap<String, HashMap<String, EventSubscriberShape>>,
    // v0.177 (#643): needed to build each context's *own* combined type table,
    // so its contract hashes are computed from the same namespace a caller sees.
    unit_uses: HashMap<String, Vec<String>>,
    unit_flattened: HashMap<String, HashMap<String, String>>,
    adapter_bindings: HashMap<String, AdapterBinding>,
    npm_deps: std::collections::BTreeMap<String, String>,
    target: BuildTarget,
    import_ext: ImportExt,
    // #1890: the merged `wrangler.toml` flag list (`finish_build` merges it).
    compatibility_flags: &[String],
    workers_dev: Option<bool>,
    // #1883, slice 1b: the declared asset files' bytes; each Worker ships
    // the files its `uses` closure declares.
    asset_files: Option<&BTreeMap<String, Vec<u8>>>,
    // State-migrations S4 (#2259): each agent class's lock version, keyed
    // `<unit>.<Agent>`, for its Worker's `/_bynk/migrate/` route.
    agent_versions: &HashMap<String, bynk_check::schema_registry::AgentVersions>,
    // #2554: the older contracts each callee accepts, from `bynk.contracts.lock`.
    contracts_lock: &bynk_project::contracts_lock::ContractsLock,
    // P4b (#2608): each `D1` database's migrations.
    d1_migrations: &bynk_check::d1_migrations::D1Migrations,
) -> ProjectOutput {
    // #2554: the lock's warnings, returned on `ProjectOutput::warnings` for
    // `finish_build` to merge with the checker's.
    let mut lock_warnings: Vec<AttributedError> = Vec::new();
    // #1883, slice 1b: each declared asset's served path, and the files the
    // Workers below stage into their assets directories (empty for a Bundle
    // build: only a Worker serves files).
    let asset_served = asset_files
        .map(|files| bynk_check::assets::declared_served_paths(&parsed, files))
        .unwrap_or_default();
    let mut staged_assets: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();

    // #1655 (runtime-semantics track S6): a `--target workers` build writes the
    // workers layout (`workers/<ctx>/…`), but a *unit* test module imports the
    // bundle layout (`./../<ctx>.js`), so `tsc` over the output failed
    // (TS2307). Those modules had no consumer either: `bynkc test` compiles the
    // bundle target for them and overlays a workers compile *excluding*
    // `tests/`. A workers build therefore drops the unit modules. Integration
    // modules stand their participants up as real Workers and import this
    // layout, so they stay, and `tests/main.ts` runs just them.
    let workers = target == BuildTarget::Workers;
    if workers {
        compiled.retain(|f| !f.output_path.starts_with("tests"));
    }
    compiled.extend(integration_outputs);
    // The integration runnables follow the unit ones.
    let unit_count = runnable_tests.len();
    runnable_tests.extend(integration_runnables);

    // v0.67: the discovery manifest — built from the combined runnable set before
    // anything consumes it, so `--no-run --format json` lists suites/cases without
    // running. Ordered by the runner's sort key to match a run's suite order.
    // On `workers` it lists only what this build writes and `tests/main.ts` runs
    // (#1655, review of #1695): the integration suites. `bynkc test` reads the
    // manifest from its bundle compile, so the unit suites are still listed
    // there.
    let discovered = if workers {
        discovery_manifest(&runnable_tests[unit_count..])
    } else {
        discovery_manifest(&runnable_tests)
    };

    // v0.16: emit the combined top-level test runner once both passes are done,
    // so `tests/main.ts` aggregates unit and integration suites together (on
    // `workers`, the integration suites only: #1655, above).
    let main_tests = if workers {
        &runnable_tests[unit_count..]
    } else {
        &runnable_tests[..]
    };
    if !main_tests.is_empty() {
        let main_program = emit_test_main(main_tests, import_ext);
        compiled.push(StagedFile {
            output_path: PathBuf::from("tests/main.ts"),
            document: Document::Ts(main_program),
            source_map: None,
            debug_metadata: None,
        });
    }

    // v0.19 (decision 0025): does any context's in-process closure reach a
    // platform-native unit? Drives env threading (bundle) and the per-Worker
    // Env/`wrangler.toml` resource derivation (workers).
    let context_native = worker_bindings::contexts_native(
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_flattened,
        true,
    );
    // S4a (#1966): the same, counting only the bare resources, so a named
    // instance never brings in an unreachable bare `KV` namespace.
    let context_bare_native = worker_bindings::contexts_native(
        &kinds,
        &unit_tables,
        &unit_consumes,
        &unit_consumes_aliases,
        &unit_flattened,
        false,
    );

    // Events track, slice 0 (spine #936): project-wide "who subscribes to
    // what", built once and shared by both targets — Bundle mode's
    // `composeApp` dispatches in-process; Workers mode uses it to size each
    // publishing context's fan-out DO routing table and wrangler.toml
    // Service Bindings.
    let event_subscribers =
        bynk_check::symbols::discover_event_subscribers(&unit_tables, &unit_consumes);

    // #2137: the contexts a `system` harness stands up (test build only).
    let harness_clock_contexts =
        harness_clock_contexts(&parsed, &unit_tables, &unit_consumes, &kinds);

    match target {
        BuildTarget::Bundle => {
            // v0.6 §6.3: emit a composition root when the project has at
            // least one context that consumes another context's service
            // surface. The compose file imports each context, instantiates
            // its providers, assembles its deps (capabilities + cross-
            // context surfaces), and exports the top-level service surface.
            if let Some(compose_program) = emit_composition_root(
                &groups,
                &kinds,
                &unit_consumes,
                &unit_consumes_aliases,
                &unit_tables,
                &unit_callees,
                &unit_event_subscriber_shapes,
                &adapter_bindings,
                &unit_flattened,
                // D1: thread `env` through composeApp only when a native
                // resource is consumed, so native-free programs are
                // byte-identical to v0.18 output.
                !context_native.is_empty(),
                &event_subscribers,
            ) {
                compiled.push(StagedFile {
                    output_path: PathBuf::from("compose.ts"),
                    document: Document::Ts(compose_program),
                    source_map: None,
                    debug_metadata: None,
                });
            }
        }
        BuildTarget::Workers => {
            // v0.8 §2.3: per-Worker entry point, compose.ts, and wrangler
            // configuration. One Worker per context.
            for (ctx_name, kind) in &kinds {
                if *kind != UnitKind::Context {
                    continue;
                }
                let Some(table) = unit_tables.get(ctx_name) else {
                    continue;
                };
                let dashes = worker_dir_name(ctx_name);
                let consumes_targets = unit_consumes.get(ctx_name).cloned().unwrap_or_default();
                let aliases = unit_consumes_aliases
                    .get(ctx_name)
                    .cloned()
                    .unwrap_or_default();
                // v0.177 (#643): the callee's own view of each of its `on call`
                // contracts, hashed from *its own* namespace — the same table
                // (`combined_types_for`) a caller reaches through
                // `consumed_types[ctx_name]`, so the two sides cannot disagree.
                let own_types = bynk_check::symbols::contract_types_for(
                    ctx_name,
                    &unit_tables,
                    &unit_uses,
                    &unit_consumes,
                )
                .0;
                let own_contracts = bynk_check::contract::own_contract_hashes(table, &own_types);
                // #2554: the older contracts this callee also accepts, each
                // re-checked against the contract it has now.
                let accepted = bynk_check::contract_compat::own_accepted(
                    table,
                    &own_types,
                    |service| contracts_lock.accepted(ctx_name, service),
                    bynk_project::contracts_lock::ACCEPTED_CAP,
                );
                for (service, why) in &accepted.stale {
                    lock_warnings.push(bynk_project::contracts_lock::stale_warning(
                        ctx_name, service, why,
                    ));
                }
                for (service, count) in &accepted.oversized {
                    lock_warnings.push(bynk_project::contracts_lock::oversized_warning(
                        ctx_name, service, *count,
                    ));
                }
                let binding_modules: HashMap<String, String> = adapter_bindings
                    .iter()
                    .map(|(n, b)| {
                        (
                            n.clone(),
                            emitter::ts_specifier(&b.output_path.with_extension("js")),
                        )
                    })
                    .collect();
                let flattened = unit_flattened.get(ctx_name).cloned().unwrap_or_default();
                // v0.19 (C1): this Worker needs the KV namespace binding when
                // its in-process closure reaches the cloudflare adapter.
                let needs_kv = worker_bindings::needs_kv(&context_bare_native, ctx_name);
                // Locale capability track, slice 2 (#882, Decision A): real
                // negotiation is Cloudflare-only — `BuildTarget::Workers`
                // isn't itself restricted to `Platform::Cloudflare`, so a
                // hypothetical `--target workers --platform node` project
                // must not try to pass 3 args to Node's still-0-arg
                // `LocaleProvider`.
                let is_cloudflare_binding = adapter_bindings
                    .get(firstparty::BYNK_UNIT)
                    .is_some_and(|b| {
                        b.output_path.file_name()
                            == Some(std::ffi::OsStr::new(
                                firstparty::Platform::Cloudflare.bynk_binding_filename(),
                            ))
                    });
                let bundle = bynk_check::symbols::detect_context_message_bundle(
                    ctx_name, &unit_uses, &groups, &kinds, &parsed,
                );
                let locale_bundle_info = match &bundle {
                    bynk_check::symbols::ContextMessageBundle::One(info)
                        if is_cloudflare_binding =>
                    {
                        Some(info)
                    }
                    _ => None,
                };
                // #1187's slice 6 plumbing: computed once, reused by every
                // Workers-target emitter below that needs it.
                let ctx_uses_emit = unit_table_uses_emit(table, unit_callees.get(ctx_name));
                // #1944: every binding this Worker declares, once — its
                // `Env` interface and its `wrangler.toml` are both built from
                // it, and the duplicate check read the same list.
                let own_event_routes =
                    worker_bindings::own_event_routes(ctx_name, &event_subscribers);
                // What this context calls on each dependency (#1961): its
                // Service Bindings, and below, its contracts manifest.
                let called = called_cross_context_services(
                    table,
                    &consumes_targets,
                    unit_callees.get(ctx_name),
                );
                let called_contexts = worker_bindings::called_contexts(
                    table,
                    &consumes_targets,
                    Some(&aliases),
                    &kinds,
                    unit_callees.get(ctx_name),
                );
                let bindings = worker_bindings::context_bindings(
                    table,
                    &called_contexts,
                    &own_event_routes,
                    needs_kv,
                    ctx_uses_emit,
                );
                // #2719: the context's handlers that hand work to the execution
                // context, from the checker's needs-context set.
                let ctx_uses_exec = table
                    .services
                    .values()
                    .any(|s| s.handlers.iter().any(|h| table.exec_plan.handler_needs(h)));
                let (compose_ts, needs_locale_request) = emitter::emit_worker_compose(
                    ctx_name,
                    table,
                    &consumes_targets,
                    &aliases,
                    &unit_tables,
                    &binding_modules,
                    &flattened,
                    &unit_consumes,
                    &unit_consumes_aliases,
                    &unit_flattened,
                    locale_bundle_info,
                    import_ext,
                    ctx_uses_emit,
                    &bindings,
                    harness_clock_contexts.contains(ctx_name),
                    ctx_uses_exec,
                );
                // #2136: a context a `system` suite targets gets the test
                // build's scheduled-result hook. A deployable build strips
                // suites (`tests` off), so it never does.
                let system_driven = parsed
                    .iter()
                    .filter_map(ParsedFile::integration)
                    .any(|d| d.target.joined() == *ctx_name);
                // #2252: the types this context takes from the contexts it
                // consumes, recorded when its handlers were checked.
                let no_consumed_types = HashSet::new();
                let consumed_types = unit_consumed_context_types
                    .get(ctx_name)
                    .unwrap_or(&no_consumed_types);
                // State-migrations S4 (#2259 Decision D): the context's
                // versioned agent classes, which its Worker's signed
                // `/_bynk/migrate/` route can wake.
                let mut migratable: Vec<(String, i64)> = agent_versions
                    .iter()
                    .filter_map(|(class, v)| {
                        let (unit, agent) = class.rsplit_once('.')?;
                        (unit == ctx_name && table.agents.contains_key(agent))
                            .then(|| (agent.to_string(), v.version))
                    })
                    .collect();
                migratable.sort();
                let entry_ts = emitter::emit_worker_entry(
                    ctx_name,
                    table,
                    &own_contracts,
                    &accepted.hashes,
                    needs_locale_request,
                    ctx_uses_emit,
                    system_driven,
                    consumed_types,
                    &migratable,
                    ctx_uses_exec,
                );
                // P6.x cutover slice 2 (#1191): collected here, not inside
                // `emit_wrangler_toml` itself, so that function's own file
                // needs no raw-AST match. P6.46 (#1137): the walk itself
                // relocated to `bynk_check::symbols::cron_and_queue_triggers`
                // — a pure function of `table`, `project.rs`'s own remaining
                // AST contact here was incidental to where the loop happened
                // to be written, not structural.
                let (crons, queues) = bynk_check::symbols::cron_and_queue_triggers(table);
                let mut wrangler_doc = emitter::emit_wrangler_toml(
                    ctx_name,
                    &bindings,
                    &crons,
                    &queues,
                    compatibility_flags,
                    workers_dev,
                    &emitter::ClassTombstones::of(table),
                    d1_migrations,
                );
                // P4b (#2608): each `D1` database's migrations, beside the
                // `wrangler.toml` whose `migrations_dir` names them.
                for binding in &bindings {
                    if !matches!(
                        binding.source,
                        emitter::wrangler::BindingSource::D1Database { .. }
                    ) {
                        continue;
                    }
                    let database = emitter::wrangler::d1_database_name(&dashes, &binding.name);
                    for file in d1_migrations.get(&database).into_iter().flatten() {
                        compiled.push(StagedFile {
                            output_path: PathBuf::from(format!(
                                "workers/{dashes}/{}/{database}/{}",
                                emitter::wrangler::D1_MIGRATIONS_DIR,
                                file.name
                            )),
                            document: Document::Sql(file.sql.clone()),
                            source_map: None,
                            debug_metadata: None,
                        });
                    }
                }
                // #1883, slice 1b: the static files this Worker serves — its
                // own and those of every commons its `uses` closure reaches —
                // under its assets directory, with `_headers` and the
                // `[assets]` block. A Worker with none gets neither.
                let deployed = bynk_check::assets::deployed_assets(
                    ctx_name,
                    &unit_uses,
                    &unit_consumes,
                    &unit_tables,
                    &asset_served,
                );
                if !deployed.is_empty() {
                    let dir = worker_assets_dir(&dashes);
                    for (served, source) in &deployed {
                        let bytes = asset_files
                            .and_then(|files| files.get(source))
                            .cloned()
                            .unwrap_or_else(|| {
                                panic!(
                                    "bynk internal error: deployed asset `{source}` was never read"
                                )
                            });
                        staged_assets.insert(dir.join(served.trim_start_matches('/')), bytes);
                    }
                    staged_assets.insert(
                        dir.join("_headers"),
                        emitter::wrangler::asset_headers().into_bytes(),
                    );
                    wrangler_doc.push_block(emitter::wrangler::assets_block());
                }
                // L7 (#2414): an `EdgeCache` instance turns Workers Cache on.
                if table.derived.values().any(|d| d.is_edge_cache()) {
                    wrangler_doc.push_block(emitter::wrangler::edge_cache_block());
                }
                // #2321: a Worker whose adapter closure reaches an included
                // WebAssembly module gets the explicit `CompiledWasm` rule.
                if worker_reaches_wasm_module(ctx_name, &unit_consumes, &kinds, &adapter_bindings) {
                    wrangler_doc.push_block(emitter::wrangler::wasm_rule_block());
                }
                // Arc C slice 4 (#1323): `emit_worker_entry` returns a real
                // `TsProgram` directly — the construction site's own
                // `Verbatim`/`NotYetConverted` wrap is gone, matching the
                // precedent `events_fanout.rs`'s and `workers.rs`'s own
                // construction sites already set.
                compiled.push(StagedFile {
                    output_path: PathBuf::from(format!("workers/{dashes}/index.ts")),
                    document: Document::Ts(entry_ts),
                    source_map: None,
                    debug_metadata: None,
                });
                // Events track, slice 0: this context's fan-out Durable
                // Object — emitted only when it actually publishes (mirrors
                // `emit_worker_compose`'s own `unit_table_uses_emit` gate on
                // `deps.__eventsDispatch`, so the two never disagree about
                // whether `env.EVENTS_FANOUT` is real).
                if ctx_uses_emit {
                    // Arc C's own first real conversion slice (#1317):
                    // `emit_events_fanout_do` returns a real `TsProgram`
                    // directly — the first `bynk-emit` construction site
                    // that reaches `Document::Ts` with no `Verbatim` wrap.
                    let fanout_program =
                        emitter::emit_events_fanout_do(ctx_name, &own_event_routes);
                    compiled.push(StagedFile {
                        output_path: PathBuf::from(format!("workers/{dashes}/events_fanout.ts")),
                        document: Document::Ts(fanout_program),
                        source_map: None,
                        debug_metadata: None,
                    });
                }
                // Arc C slice 3 (#1321): `emit_worker_compose` returns a
                // real `TsProgram` directly — the second `bynk-emit`
                // construction site (after `events_fanout.ts`'s own,
                // #1317) that reaches `Document::Ts` with no `Verbatim`
                // wrap.
                compiled.push(StagedFile {
                    output_path: PathBuf::from(format!("workers/{dashes}/compose.ts")),
                    document: Document::Ts(compose_ts),
                    source_map: None,
                    debug_metadata: None,
                });
                compiled.push(StagedFile {
                    output_path: PathBuf::from(format!("workers/{dashes}/wrangler.toml")),
                    document: Document::Toml(wrangler_doc),
                    source_map: None,
                    debug_metadata: None,
                });
                // v0.172 (ADR 0195 D5): the secret names this Worker's handlers
                // will read from `env`, for `deploy` to check before it pushes.
                // Emitted from the same seams the entry lowers, so the two
                // cannot describe different Workers.
                //
                // v0.173 (ADR 0196): plus the literal `bynk.Secrets` names it
                // reads, and whether that list is everything. The walk is here
                // rather than in the checker because it needs `unit_flattened`
                // to answer *whose* `Secrets` this is (D4) and the warning sink
                // to say when it cannot know a name — and `bynk-check` has
                // neither. Absent when there is nothing at all to say (D5).
                // The warnings half is dropped here: `run_checks` already raised
                // it, on the analyse path the editor shares.
                // v0.177 (#643): the contract hashes this context's entry
                // enforces, written where `deploy` can read them — so a skew is
                // refused at the push rather than discovered by live traffic.
                let own_types =
                    bynk_check::symbols::combined_types_for(ctx_name, &unit_tables, &unit_uses);
                // What this context expects of each dependency — the same hash
                // it stamps at each call site, computed the same way: in the
                // *dependency's* namespace, from the dependency's own table.
                //
                // Only the services this context actually **calls**. Recording
                // everything the dependency provides would refuse a deploy over a
                // service this caller never touches — a skew its runtime check
                // could never fire on. See `called_cross_context_services`.
                let mut expects: std::collections::BTreeMap<
                    String,
                    std::collections::BTreeMap<String, String>,
                > = std::collections::BTreeMap::new();
                let mut expects_shapes: std::collections::BTreeMap<
                    String,
                    std::collections::BTreeMap<String, String>,
                > = std::collections::BTreeMap::new();
                for (dep, services) in &called {
                    let Some(dep_table) = unit_tables.get(dep) else {
                        continue;
                    };
                    let dep_types = bynk_check::symbols::contract_types_for(
                        dep,
                        &unit_tables,
                        &unit_uses,
                        &unit_consumes,
                    )
                    .0;
                    let all = bynk_check::contract::own_contract_hashes(dep_table, &dep_types);
                    let hashes: std::collections::BTreeMap<String, String> = all
                        .into_iter()
                        .filter(|(svc, _)| services.contains(svc))
                        .collect();
                    if !hashes.is_empty() {
                        // #2485: the structure behind each hash, from the same
                        // dependency table.
                        let dep_shapes: std::collections::BTreeMap<String, String> =
                            bynk_check::contract_compat::own_contract_shapes(dep_table, &dep_types)
                                .into_iter()
                                .filter(|(svc, _)| hashes.contains_key(svc))
                                .collect();
                        expects_shapes.insert(dep.clone(), dep_shapes);
                        expects.insert(dep.clone(), hashes);
                    }
                }
                // I4 (#1970): what each declared foreign caller's generated
                // client embeds, recorded beside `provides` so `deploy` can
                // hold it against the caller's pin.
                let foreign = emitter::client::foreign_contracts(table, &own_types);
                if let Some(manifest) = emitter::contracts::emit_contracts_manifest(
                    &bynk_check::contract::own_contract_hashes(table, &own_types),
                    &expects,
                    &foreign,
                    &emitter::contracts::Shapes {
                        provides: bynk_check::contract_compat::own_contract_shapes(
                            table, &own_types,
                        ),
                        expects: expects_shapes,
                        accepts: accepted.hashes.clone(),
                    },
                ) {
                    compiled.push(StagedFile {
                        output_path: PathBuf::from(format!(
                            "workers/{dashes}/{}",
                            emitter::contracts::CONTRACTS_MANIFEST
                        )),
                        document: Document::Json(manifest),
                        source_map: None,
                        debug_metadata: None,
                    });
                }

                // I4 (#1970): the typed TypeScript client a foreign caller
                // imports, when the context declares one.
                if let Some(client) = emitter::client::emit_client(ctx_name, table, &own_types) {
                    compiled.push(StagedFile {
                        output_path: PathBuf::from(format!(
                            "workers/{dashes}/{}",
                            emitter::client::CLIENT_FILE
                        )),
                        document: Document::Ts(client),
                        source_map: None,
                        debug_metadata: None,
                    });
                }

                let reads = emitter::secrets::secret_reads(
                    ctx_name,
                    &unit_tables,
                    &unit_consumes,
                    &unit_consumes_aliases,
                    &unit_flattened,
                );
                if let Some(manifest) = emitter::emit_secrets_manifest(table, &reads) {
                    compiled.push(StagedFile {
                        output_path: PathBuf::from(format!(
                            "workers/{dashes}/{}",
                            emitter::secrets::SECRETS_MANIFEST
                        )),
                        document: Document::Json(manifest),
                        source_map: None,
                        debug_metadata: None,
                    });
                }
            }
        }
    }

    // v0.17: copy each adapter binding verbatim into the output, beside the
    // adapter's emitted interface module, so compose's import resolves and the
    // `tsc` gate checks the `implements` contract.
    let mut binding_names: Vec<&String> = adapter_bindings.keys().collect();
    binding_names.sort();
    for name in binding_names {
        let b = &adapter_bindings[name];
        compiled.push(StagedFile {
            output_path: b.output_path.clone(),
            document: Document::Ts(bynk_ts::TsProgram {
                stmts: vec![bynk_ts::TsStmt::verbatim(
                    bynk_ts::VerbatimOrigin::NotYetConverted,
                    emitter::runtime_files::rewrite_runtime_imports(&b.content),
                    None,
                )],
            }),
            source_map: None,
            debug_metadata: None,
        });
    }

    // #2321: each included WebAssembly module — its bytes beside the binding
    // (a module file, never served) and the loader shim the binding imports.
    let mut staged_modules: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    for b in adapter_bindings.values() {
        for inc in &b.includes {
            staged_modules.insert(inc.output_path.clone(), inc.bytes.clone());
            compiled.push(StagedFile {
                output_path: wasm_shim_path(&inc.output_path),
                document: Document::Ts(bynk_ts::TsProgram {
                    stmts: vec![bynk_ts::TsStmt::verbatim(
                        bynk_ts::VerbatimOrigin::NotYetConverted,
                        workers_wasm_shim(&inc.output_path),
                        None,
                    )],
                }),
                source_map: None,
                debug_metadata: None,
            });
        }
    }
    if !staged_modules.is_empty() {
        compiled.push(StagedFile {
            output_path: PathBuf::from("wasm-modules.d.ts"),
            document: Document::Ts(bynk_ts::TsProgram {
                stmts: vec![bynk_ts::TsStmt::verbatim(
                    bynk_ts::VerbatimOrigin::NotYetConverted,
                    WASM_MODULES_DTS.to_string(),
                    None,
                )],
            }),
            source_map: None,
            debug_metadata: None,
        });
    }

    // `package.json`: always `"type": "module"` (#2148), so the emitted
    // tsconfig's `module: NodeNext` checks and builds the output as the ES
    // modules it is, under Node's ESM rules; plus the adapter-declared npm
    // dependencies when there are any (v0.17).
    compiled.push(StagedFile {
        output_path: PathBuf::from("package.json"),
        document: Document::Json(render_package_json(&npm_deps)),
        source_map: None,
        debug_metadata: None,
    });

    // Runtime (ADR 0615): exactly the modules the emitted files import, closed
    // over the modules' own imports, under `__bynk_runtime/` at the root of
    // `out/` so every file's relative specifier resolves to it.
    // `tsconfig.json` is also at the root so `tsc -p out/tsconfig.json`
    // discovers every `.ts` file in the tree.
    let mut reached = BTreeSet::new();
    for f in &compiled {
        if let Document::Ts(program) = &f.document {
            emitter::runtime_files::reached_in(program, &mut reached);
        }
    }
    for (path, source) in emitter::runtime_files::closure_files(&reached) {
        compiled.push(StagedFile {
            output_path: path,
            document: Document::Ts(bynk_ts::TsProgram {
                stmts: vec![bynk_ts::TsStmt::verbatim(
                    bynk_ts::VerbatimOrigin::NotYetConverted,
                    source,
                    None,
                )],
            }),
            source_map: None,
            debug_metadata: None,
        });
    }
    compiled.push(StagedFile {
        output_path: PathBuf::from("tsconfig.json"),
        document: Document::Json(emitter::emit_tsconfig()),
        source_map: None,
        debug_metadata: None,
    });

    // `Artefacts.docs` is a `BTreeMap`, so it iterates in `output_path` order
    // naturally (Decision D, #1309) — no explicit sort needed here the way
    // the old `Vec<CompiledFile>` (sorted by `source_path`) required.
    let mut docs: BTreeMap<PathBuf, Document> = BTreeMap::new();
    // #2554: lock entries for a context the project no longer has.
    for (context, services) in contracts_lock.contexts() {
        if kinds.get(context) != Some(&UnitKind::Context) {
            for service in services.keys() {
                lock_warnings.push(bynk_project::contracts_lock::stale_warning(
                    context,
                    service,
                    "the context is no longer in the project",
                ));
            }
        }
    }

    for f in compiled {
        if let Some(sm) = &f.source_map {
            docs.insert(
                sibling_path(&f.output_path, "map"),
                Document::SourceMap(sm.clone()),
            );
        }
        if let Some(dbg) = &f.debug_metadata {
            docs.insert(
                sibling_path(&f.output_path, "bynkdbg.json"),
                Document::DebugSidecar(dbg.clone()),
            );
        }
        docs.insert(f.output_path, f.document);
    }
    ProjectOutput {
        artefacts: Artefacts {
            docs,
            assets: staged_assets,
            modules: staged_modules,
        },
        discovered,
        // Populated by `compile_project` from the run's warning sink (ADR 0117);
        // until then, the contracts lock's own (#2554).
        warnings: lock_warnings,
        // Populated by `finish_build` from the same `RunChecks::Checked` this
        // whole `ProjectOutput` was built from.
        snapshots: Vec::new(),
        display_root: PathBuf::new(),
        // Likewise (#1078) — `Some` only when the registry was on.
        schema_lock: None,
    }
}

// P5.3 review (#1133): `resolve_consume_prefix` and `handler_cross_caps` used
// to have their own copies here, byte-identical to `bynk-check::project_model`'s
// (neither builds TypeScript, so neither had the codegen coupling that keeps
// `instantiate_provider_ts_expr`/`native_platforms_of_context` below in this
// crate) — deleted, every call site repointed at
// `project_model::{resolve_consume_prefix, handler_cross_caps}`.

/// v0.19 (decision 0017): the native platforms a context's **in-process
/// closure** commits it to: every unit whose provider its compose would
/// instantiate — local providers' `given` recursion plus the capabilities its
/// handlers reference — mapped through [`firstparty::platform_of`]. Each
/// platform carries an exemplar unit for the diagnostic message. Service
/// `consumes` edges (RPC under `workers`) do not contribute — only the
/// provider-instantiation walk, which is in-process by construction.
#[allow(clippy::too_many_arguments)]
fn native_platforms_of_context(
    ctx: &str,
    table: &UnitTable,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    // S4a (#1966): count derived capabilities' base units. `true` for what
    // threads `env`; `false` for whether the *bare* platform resource (the
    // `KV` namespace) is used, which a named instance alone must not imply.
    with_derived: bool,
) -> std::collections::BTreeMap<Platform, String> {
    // Arc F slice 2 (#1452): repointed at the tree-native twin — only the
    // `referenced` side effect matters here, the returned `TsExpr` (vs. the
    // old `String`) is discarded either way.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let namespaces = if with_derived {
        ProviderNamespaces::Bundle
    } else {
        ProviderNamespaces::BareResources
    };
    for cap in table.providers.keys() {
        let _ = instantiate_provider_ts_expr(
            ctx,
            cap,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            namespaces,
            None,
            None,
            &mut referenced,
        );
    }
    let consumed = unit_consumes.get(ctx).cloned().unwrap_or_default();
    let aliases = unit_consumes_aliases.get(ctx).cloned().unwrap_or_default();
    let flattened = unit_flattened.get(ctx).cloned().unwrap_or_default();
    for (key, cctx) in handler_cross_caps(table, &consumed, &aliases, &flattened) {
        let _ = instantiate_provider_ts_expr(
            &cctx,
            &key,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            namespaces,
            None,
            None,
            &mut referenced,
        );
    }
    if with_derived {
        referenced.extend(table.derived.values().map(|d| d.base_unit.clone()));
    }
    let mut out = std::collections::BTreeMap::new();
    for unit in referenced {
        if let Some(p) = bynk_check::firstparty::platform_of(&unit) {
            out.entry(p).or_insert(unit);
        }
    }
    out
}

/// #527: the DO-side deps plan for one workers context. Capability providers
/// cannot cross the DO wire (`{ args, deps }` is JSON — a provider's methods
/// die in serialisation), so the generated Durable Object reconstructs its
/// agents' `given` deps *inside* the DO from the same wiring compose uses.
#[derive(Debug, Default, Clone)]
pub struct AgentDepsPlan {
    /// agent name → TS object-literal expression for its `given` deps
    /// (e.g. `{ Clock: new bynk__binding.ClockProvider() }`).
    pub exprs: HashMap<String, String>,
    /// Import lines the expressions need in `handlers.ts` (binding modules,
    /// other Workers' handlers). Same relative depth as `compose.ts`.
    pub imports: Vec<String>,
}

/// Build the [`AgentDepsPlan`] for context `name`, or `None` when no local
/// agent has `given` capabilities.
///
/// `split`: the context spans several files, each emitted as its own module
/// under `handlers/` (#1820, [`worker_file_source_path`]). The imports are
/// then one directory deeper, and this context's own providers are no longer
/// in the agent's module, so they are reached through `handlers.ts` like
/// another context's. That import is circular (`handlers.ts` re-exports the
/// agent's module), which is safe: the expressions are evaluated in the
/// Durable Object's fetch dispatch, never while the modules load.
fn plan_agent_given_deps(
    name: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
    adapter_bindings: &HashMap<String, AdapterBinding>,
    split: bool,
    // #2137: whether a `system` harness stands this context up (test build).
    harness_clock: bool,
) -> Option<AgentDepsPlan> {
    let info = unit_info.get(name)?;
    info.table.agents.values().next()?;
    let unit_tables: HashMap<String, UnitTable> = unit_info
        .iter()
        .map(|(n, i)| (n.clone(), i.table.clone()))
        .collect();
    let unit_consumes: HashMap<String, Vec<String>> = unit_info
        .iter()
        .map(|(n, i)| (n.clone(), i.consumes.clone()))
        .collect();
    let unit_consumes_aliases: HashMap<String, HashMap<String, String>> = unit_info
        .iter()
        .map(|(n, i)| (n.clone(), i.aliases.clone()))
        .collect();
    let unit_flattened: HashMap<String, HashMap<String, String>> = unit_info
        .iter()
        .map(|(n, i)| (n.clone(), i.flattened.clone()))
        .collect();

    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut exprs: HashMap<String, String> = HashMap::new();
    // P6.35 (design/tracks/the-ir.md §6a): no explicit `&AgentDecl` annotation
    // needed — the loop body's own `a.handlers` field access below already
    // pins the element type through inference.
    let mut agents: Vec<_> = info.table.agents.iter().collect();
    agents.sort_by_key(|(n, _)| (*n).clone());
    for (agent, a) in agents {
        // #1187's slice 6 (Agent/Service given wiring): reads bynk-emit::ir's
        // own CapRefIr (lower_handler_given_ir — a standalone reader mirroring
        // lower_provider_given_ir, #1200) instead of walking the raw AST
        // CapRef directly. `caps` stays keyed by bare name (declaration-order-
        // first dedup across every handler), just storing CapRefIr instead of
        // CapRef.
        let mut caps: std::collections::BTreeMap<String, CapRefIr> =
            std::collections::BTreeMap::new();
        for h in &a.handlers {
            for g in lower_handler_given_ir(h) {
                // Events track, slice 0 (spine #936): see the matching skip
                // in `handler_cross_caps` — no `EventsProvider` exists for
                // compose (or a synthesised DO's own reconstructed deps) to
                // build.
                if g.name == "Events"
                    && info.flattened.get(g.name.as_str()).map(String::as_str) == Some("bynk")
                {
                    continue;
                }
                caps.entry(g.name.clone()).or_insert(g);
            }
        }
        if caps.is_empty() {
            continue;
        }
        // Arc F slice 2 (#1452): `instantiate_provider_ts_expr` — the
        // tree-native twin `emit_worker_compose`/`emit_composition_root`
        // already use (#1321/#1327) — builds the same `new {ns}.{Class}(...)`
        // shape as a real `bynk_ts::TsExpr`, byte-identical once printed
        // (both share every recursive call/text-building step below the
        // top level; only this per-agent object wrapper is new). Boundary-
        // prints once, here, into `exprs: HashMap<String, String>` — the
        // struct's own shape and `emit_agent`'s `TsExpr::Ident(expr.clone())`
        // caller (`emitter/emit.rs`) both stay unchanged, the narrower of
        // the two options the issue named.
        let entries: Vec<(String, bynk_ts::TsExpr)> = caps
            .iter()
            .map(|(key, g)| {
                let target_ctx = match &g.context {
                    Some(p) => resolve_consume_prefix(p, &info.consumes, &info.aliases)
                        .unwrap_or_else(|| name.to_string()),
                    None => info
                        .flattened
                        .get(key.as_str())
                        .cloned()
                        .unwrap_or_else(|| name.to_string()),
                };
                let expr = instantiate_provider_ts_expr(
                    &target_ctx,
                    key,
                    &unit_tables,
                    &unit_consumes,
                    &unit_consumes_aliases,
                    &unit_flattened,
                    ProviderNamespaces::Workers { harness_clock },
                    Some("env"),
                    None,
                    &mut referenced,
                );
                (key.clone(), expr)
            })
            .collect();
        exprs.insert(
            agent.clone(),
            bynk_ts::print_expr(&bynk_ts::TsExpr::object(entries)),
        );
    }
    if exprs.is_empty() {
        return None;
    }
    // Providers of *this* context live in the same module (`handlers.ts`), so
    // their compose-namespace prefix drops. Not for a split context: there
    // they are imported from `handlers.ts` like any other context's (`../`
    // up from `handlers/` to the Worker's own directory, then back down).
    let self_ns = format!("handlers_{}.", name.replace('.', "_"));
    if !split {
        for e in exprs.values_mut() {
            *e = e.replace(&self_ns, "");
        }
        referenced.remove(name);
    }
    let up = if split { "../" } else { "" };
    let mut imports = Vec::new();
    for u in &referenced {
        let ns = u.replace('.', "_");
        if let Some(b) = adapter_bindings.get(u) {
            let module = crate::emitter::ts_specifier(&b.output_path.with_extension("js"));
            imports.push(format!(
                "import * as {ns}__binding from \"{up}../../{module}\";"
            ));
        } else {
            let dir = worker_dir_name(u);
            imports.push(format!(
                "import * as handlers_{ns} from \"{up}../{dir}/handlers.js\";"
            ));
        }
    }
    Some(AgentDepsPlan { exprs, imports })
}

/// v0.15: build the TypeScript expression instantiating the provider of
/// capability `cap` declared in `provider_ctx`, recursively wiring its `given`
/// dependencies — local sibling providers and cross-context capability
/// providers alike. Stateless providers, so fresh instances per use are fine.
///
/// v0.18 (spec §4.5/§5.1): a *bare* `given` name resolves through the
/// provider's own unit's flattened-capability map (`Fetch` → `bynk`), falling
/// back to the unit itself; an *external* provider's deps are built the same
/// way and passed to the binding class constructor by name. Every unit whose
/// namespace the expression references is recorded in `referenced_units` so
/// the caller can emit the matching imports (the transitive given-closure).
///
/// Locale capability track, slice 2 (#882, Decision C): the three extra
/// constructor arguments `LocaleProvider` receives when its composing
/// context has a uniquely-detected message bundle — the JS expressions
/// themselves (an identifier for `request`, and the two cross-commons-
/// imported bundle constants), not raw data, since they're spliced directly
/// into the generated `new bynk__binding.LocaleProvider(...)` call.
pub(crate) struct LocaleNegotiationArgs {
    pub(crate) request_expr: String,
    pub(crate) declared_locales_expr: String,
    pub(crate) reference_locale_expr: String,
}

/// Where [`instantiate_provider_ts_expr`]'s expression finds each unit's
/// provider classes: the module that constructs it imports each unit under a
/// different namespace.
#[derive(Clone, Copy)]
pub(crate) enum ProviderNamespaces<'a> {
    /// The bundle's `compose.ts`: `{ns}`.
    Bundle,
    /// No module: [`native_platforms_of_context`]'s walk for the *bare*
    /// platform resources a context reaches (whether its Worker declares the
    /// `KV` namespace). As `Bundle`, except that a derived capability's
    /// instance (S4a, #1966) is not followed to its base: a named instance has
    /// its own binding and does not imply the bare one.
    BareResources,
    /// A Worker's `compose.ts` or `handlers.ts`: `handlers_{ns}`.
    ///
    /// #2137: `harness_clock` is set in the test build of a context a
    /// `system` harness stands up. Its first-party `Clock` then reads the
    /// harness clock the test module puts on `env.__bynkClock`, falling back to
    /// the real provider; a deployable build carries no suites, so it never
    /// sets it.
    Workers { harness_clock: bool },
    /// A test module (#1863): `__ns_{ns}`, the test scaffold's own.
    ///
    /// `prebuilt` names, by `(unit, capability)`, the local that holds a
    /// capability the test deps build at the top level: a provider's `given`
    /// that resolves to one is that local, not a fresh construction, so a
    /// `stub` overlaid on it reaches every provider built over it (#1864
    /// review). Each one so used is recorded in `used`.
    ///
    /// #1975: `bindings` names, by adapter, the binding module a test module
    /// imports as `{ns}__binding` when it builds that adapter's external
    /// provider for real (an adapter suite, or an `integration` case); an
    /// adapter absent from it is the placeholder. `double` answers a
    /// capability that has a deterministic test double (`__bynkTest_<Cap>()`)
    /// with that double, recording it, so a real binding's own `given` takes
    /// the tier's double rather than the platform's real provider.
    Test {
        prebuilt: &'a HashMap<(String, String), String>,
        used: &'a std::cell::RefCell<BTreeSet<(String, String)>>,
        bindings: Option<&'a HashMap<String, String>>,
        double: &'a dyn Fn(&str, &str) -> Option<bynk_ts::TsExpr>,
    },
}

/// `new {ns}.{class}({args})` as a real [`bynk_ts::TsExpr::New`] node.
fn new_call_ts_expr(ns: &str, class: &str, args: Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::New {
        callee: Box::new(bynk_ts::TsExpr::Member {
            object: Box::new(bynk_ts::TsExpr::Ident(ns.to_string())),
            property: class.to_string(),
        }),
        args,
    }
}

/// `namespaces` selects the namespace convention ([`ProviderNamespaces`]): a
/// bodied provider's class lives in `{ns}` under the bundle root,
/// `handlers_{ns}` in a Worker compose and `__ns_{ns}` in a test module;
/// external (binding) classes are `{ns}__binding` in the first two, and a
/// test module has none. When `env_ident` is set (workers), env-taking
/// first-party providers receive it as a constructor argument.
///
/// Locale capability track, slice 2 (#882): `locale_negotiation`, when
/// `Some`, is threaded to exactly the `(bynk, LocaleProvider)` pair, the same
/// way `env_ident` is threaded to `provider_takes_env`'s pairs — a small,
/// closed set of first-party providers that need ambient, request-scoped
/// construction data no ordinary `given` clause could express.
///
/// Originally a `TsExpr`-returning twin *alongside* a `String`-returning
/// `instantiate_provider_expr` (the same "structural converter added
/// alongside the `String` one" pattern Decision B already uses for
/// `TypeRef -> TsType`) — #1321 (Arc C slice 3): `emitter::workers::
/// emit_worker_compose` now builds a real `TsProgram` directly, and its own
/// cross-context capability-provider `const {key} = {expr};` lines need a
/// real `TsExpr`, not a `String` to splice. `workers.rs`'s own
/// `emit_worker_compose` (Workers mode, `workers_ns: true`) and
/// `emit_composition_root`'s own Bundle-mode `compose.ts` (#1327, Arc C
/// slice 6, `workers_ns: false`) called this twin from the start. Originally
/// hardcoded `workers_ns = true` (the only mode `emit_worker_compose`'s own
/// call site used at the time) — #1327 restored the `workers_ns: bool`
/// parameter its then-`String`-returning sibling always had, matching that
/// signature exactly, once a second real caller needed `false`.
/// Arc F slice 2 (#1452): `plan_agent_given_deps`/`native_platforms_of_context`
/// (below in this file) repointed here too — `instantiate_provider_expr`
/// itself had no callers left and is deleted; its parameter-contract prose
/// and body rationale comments (below) moved here rather than being lost.
#[allow(clippy::too_many_arguments)]
pub(crate) fn instantiate_provider_ts_expr(
    provider_ctx: &str,
    cap: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    namespaces: ProviderNamespaces<'_>,
    env_ident: Option<&str>,
    locale_negotiation: Option<&LocaleNegotiationArgs>,
    referenced_units: &mut BTreeSet<String>,
) -> bynk_ts::TsExpr {
    // S4a (#1966): a derived capability (`capability Sessions = Kv`) is its
    // base's provider bound to the instance's own Worker binding: the base's
    // expression (`new bynk_cloudflare__binding.WorkersKv(env)`) with the
    // binding name as one more argument. Every composer (a Worker's compose,
    // the bundle's `composeApp`, an agent's deps, a provider's `given`) builds
    // an instance through here.
    if let Some(inst) = unit_tables
        .get(provider_ctx)
        .and_then(|t| t.derived.get(cap))
    {
        if matches!(namespaces, ProviderNamespaces::BareResources) {
            return bynk_ts::TsExpr::Ident("undefined".to_string());
        }
        let mut expr = instantiate_provider_ts_expr(
            &inst.base_unit,
            &inst.base_cap,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            namespaces,
            env_ident,
            locale_negotiation,
            referenced_units,
        );
        // L7 (#2414): an `EdgeCache` instance has no binding: it purges
        // through the entry's `ctx`, which a Worker's compose takes as `exec`.
        // Anywhere else (the bundle) there is no edge cache, and the provider
        // built without one purges nothing.
        if inst.is_edge_cache() {
            if let (ProviderNamespaces::Workers { .. }, bynk_ts::TsExpr::New { args, .. }) =
                (namespaces, &mut expr)
            {
                args.push(bynk_ts::TsExpr::Ident("exec".to_string()));
            }
            return expr;
        }
        // Review of #1968: anything but a construction to append the binding
        // to would quietly read the default `KV`, so two instances would share
        // a namespace. The one other shape is a test module's placeholder for
        // a provider it cannot build, which has no binding to carry.
        match &mut expr {
            bynk_ts::TsExpr::New { args, .. } => args.push(bynk_ts::TsExpr::Lit(
                bynk_ts::TsLit::Str(inst.binding.clone()),
            )),
            bynk_ts::TsExpr::As { .. } if matches!(namespaces, ProviderNamespaces::Test { .. }) => {
            }
            _ => panic!(
                "derived capability `{cap}`: base `{}.{}` is not built by a construction, so binding `{}` has nowhere to go",
                inst.base_unit, inst.base_cap, inst.binding
            ),
        }
        return expr;
    }
    let ns = provider_ctx.replace('.', "_");
    let bodied_ns = match namespaces {
        ProviderNamespaces::Bundle | ProviderNamespaces::BareResources => ns.clone(),
        ProviderNamespaces::Workers { .. } => format!("handlers_{ns}"),
        ProviderNamespaces::Test { .. } => crate::emitter::emit::test_scaffold_ns(provider_ctx),
    };
    let provider = unit_tables
        .get(provider_ctx)
        .and_then(|t| t.providers.get(cap));
    // #1863: an external provider is the same placeholder as a capability
    // with no provider, unless (#1975) the test module builds this adapter's
    // binding for real; a capability with a test double takes the double.
    if let ProviderNamespaces::Test {
        bindings, double, ..
    } = namespaces
        && provider.is_none_or(|p| p.external)
        && !(provider.is_some() && bindings.is_some_and(|b| b.contains_key(provider_ctx)))
    {
        if let Some(d) = double(provider_ctx, cap) {
            return d;
        }
        return bynk_ts::TsExpr::As {
            expr: Box::new(bynk_ts::TsExpr::As {
                expr: Box::new(bynk_ts::TsExpr::Ident("undefined".to_string())),
                ty: bynk_ts::TsType::named("unknown"),
            }),
            ty: bynk_ts::TsType::named("never"),
        };
    }
    referenced_units.insert(provider_ctx.to_string());
    let Some(provider) = provider else {
        return new_call_ts_expr(&bodied_ns, cap, vec![]);
    };
    // Build the by-name deps object from the provider's `given`, if any.
    // #1187's Provider given/deps-wiring slice: reads bynk_ir's own CapRefIr
    // (lower_provider_given_ir — a standalone reader of the `given` clause,
    // never a full provider IR; see that function's own doc comment) instead
    // of walking the raw AST CapRef directly.
    let given: Vec<CapRefIr> = lower_provider_given_ir(provider);
    let deps_obj: Option<bynk_ts::TsExpr> = if given.is_empty() {
        None
    } else {
        let consumed = unit_consumes.get(provider_ctx).cloned().unwrap_or_default();
        let aliases = unit_consumes_aliases
            .get(provider_ctx)
            .cloned()
            .unwrap_or_default();
        let flattened = unit_flattened
            .get(provider_ctx)
            .cloned()
            .unwrap_or_default();
        let deps: Vec<(String, bynk_ts::TsExpr)> = given
            .iter()
            .map(|g| {
                let target_ctx = match &g.context {
                    Some(p) => resolve_consume_prefix(p, &consumed, &aliases)
                        .unwrap_or_else(|| provider_ctx.to_string()),
                    None => flattened
                        .get(&g.name)
                        .cloned()
                        .unwrap_or_else(|| provider_ctx.to_string()),
                };
                if let ProviderNamespaces::Test { prebuilt, used, .. } = namespaces {
                    let key = (target_ctx.clone(), g.name.clone());
                    if let Some(local) = prebuilt.get(&key) {
                        used.borrow_mut().insert(key);
                        return (g.name.clone(), bynk_ts::TsExpr::Ident(local.clone()));
                    }
                }
                let expr = instantiate_provider_ts_expr(
                    &target_ctx,
                    &g.name,
                    unit_tables,
                    unit_consumes,
                    unit_consumes_aliases,
                    unit_flattened,
                    namespaces,
                    env_ident,
                    locale_negotiation,
                    referenced_units,
                );
                (g.name.clone(), expr)
            })
            .collect();
        Some(bynk_ts::TsExpr::object(deps))
    };
    // #2355: a bodied provider whose ops construct a local agent takes a
    // `deps` even with no `given`, and on `workers` that `deps` carries the
    // Worker's `env` for the agent factory (`emit::emit_provider`).
    let deps_obj = if !provider.external
        && bynk_check::project_model::provider_constructs_agent(provider, |n| {
            unit_tables
                .get(provider_ctx)
                .is_some_and(|t| t.agents.contains_key(n))
        }) {
        let mut entries = match deps_obj {
            Some(bynk_ts::TsExpr::Object { entries, .. }) => entries,
            Some(other) => unreachable!("a provider's deps is an object literal: {other:?}"),
            None => Vec::new(),
        };
        if matches!(namespaces, ProviderNamespaces::Workers { .. }) {
            // Every Workers composer has the Worker's `env` in scope; leaving it
            // out would fault at `this.deps.env` at runtime instead.
            let env = env_ident.expect("a Workers composer passes its `env`");
            entries.push(bynk_ts::TsObjectEntry::Prop(
                "env".to_string(),
                bynk_ts::TsExpr::Ident(env.to_string()),
            ));
        }
        Some(bynk_ts::TsExpr::Object {
            entries,
            multiline: false,
        })
    } else {
        deps_obj
    };
    let mut args: Vec<bynk_ts::TsExpr> = deps_obj.into_iter().collect();
    // v0.18/v0.19: env-taking first-party providers (the bynk surface's
    // SecretsProvider; bynk.cloudflare's WorkersKv) receive the Worker `env`
    // explicitly — decisions 0021/0025. Keyed by (unit, class).
    if provider.external
        && bynk_check::firstparty::provider_takes_env(provider_ctx, &provider.provider_name.name)
        && let Some(env) = env_ident
    {
        args.push(bynk_ts::TsExpr::Ident(env.to_string()));
    }
    // Locale capability track, slice 2 (#882, Decision C): only the
    // `(bynk, LocaleProvider)` pair ever receives these — every other
    // provider's construction is unaffected since every other call site
    // passes `None`.
    if provider.external
        && provider_ctx == bynk_check::firstparty::BYNK_UNIT
        && provider.provider_name.name == "LocaleProvider"
        && let Some(loc) = locale_negotiation
    {
        args.push(bynk_ts::TsExpr::Ident(loc.request_expr.clone()));
        args.push(bynk_ts::TsExpr::Ident(loc.declared_locales_expr.clone()));
        args.push(bynk_ts::TsExpr::Ident(loc.reference_locale_expr.clone()));
    }
    let class = &provider.provider_name.name;
    // v0.17: an external (adapter) provider's class lives in the binding module,
    // not the adapter's interface module — instantiate it from the binding
    // namespace (`<adapter>__binding`, imported by the composition root).
    if provider.external {
        let built = new_call_ts_expr(&format!("{ns}__binding"), class, args);
        // #2137: in the test build of a `system` participant, the first-party
        // `Clock` reads the harness clock when the harness supplies one.
        if let (
            ProviderNamespaces::Workers {
                harness_clock: true,
            },
            Some(env),
        ) = (namespaces, env_ident)
            && provider_ctx == bynk_check::firstparty::BYNK_UNIT
            && cap == "Clock"
        {
            return harness_clock_or(env, &format!("{ns}__binding.{class}"), built);
        }
        built
    } else {
        new_call_ts_expr(&bodied_ns, class, args)
    }
}

/// #2137: `(env as unknown as { __bynkClock?: <class> }).__bynkClock ?? <built>`
/// — the harness clock a `system` test puts on a participant's `env`, else
/// the real provider.
fn harness_clock_or(env: &str, class: &str, built: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    use bynk_ts::{TsBinaryOp, TsExpr, TsType, TsTypeMember};
    let slot = TsExpr::As {
        expr: Box::new(TsExpr::As {
            expr: Box::new(TsExpr::Ident(env.to_string())),
            ty: TsType::named("unknown"),
        }),
        ty: TsType::Object(vec![TsTypeMember::optional_prop(
            "__bynkClock",
            TsType::named(class),
        )]),
    };
    TsExpr::Binary {
        op: TsBinaryOp::NullishCoalescing,
        left: Box::new(TsExpr::Member {
            object: Box::new(slot),
            property: "__bynkClock".to_string(),
        }),
        right: Box::new(built),
    }
}

/// #2137: every context a `system` suite's harness stands up as a Worker (its
/// participants and their event subscribers,
/// `test_suites::infer_harness_participants`).
fn harness_clock_contexts(
    parsed: &[ParsedFile],
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    kinds: &BTreeMap<String, UnitKind>,
) -> HashSet<String> {
    let systems: Vec<String> = parsed
        .iter()
        .filter_map(ParsedFile::integration)
        .map(|d| d.target.joined())
        .collect();
    if systems.is_empty() {
        return HashSet::new();
    }
    let subscribers = bynk_check::symbols::discover_event_subscribers(unit_tables, unit_consumes);
    systems
        .iter()
        .flat_map(|t| {
            bynk_check::test_suites::infer_harness_participants(
                t,
                unit_consumes,
                kinds,
                &subscribers,
            )
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
/// Events track, slice 0 (spine #936): does any handler in this unit emit —
/// the `UnitTable`-level analogue of `emitter::commons_uses_emit`, needed
/// here because compose works from the project-wide `UnitTable` map, not a
/// single unit's `TypedCommons`. #1187's slice 6 plumbing: reads the
/// checker's own already-resolved `Callee::Capability{cap:"Events",
/// op:"emit"}` (`Events.emit[...]` dispatches through the ordinary
/// capability-call path, `bynk-check/src/checker/calls.rs`) instead of
/// `emitter::block_uses_emit`'s bare-`Ident("Events")`-receiver name match.
/// `callees` is `None` only defensively (a unit whose own check never ran) —
/// every call site this function actually reaches has already certified
/// (review of #1202: traced live, confirmed unreachable on the build path
/// today). A silent `false` here disables four emission gates at once (no
/// fan-out DO, no `dispatchToEventsFanout` import, no `EVENTS_FANOUT`
/// binding, no `__eventsDispatch` field) with no diagnostic — `debug_assert`
/// makes that invariant enforced, not just documented, so a future caller
/// that violates it fails loudly in tests rather than shipping a publishing
/// context that silently drops every emitted event.
///
/// `emitter::block_uses_emit` — the per-*handler* twin deciding
/// `emit_service`/`emit_agent`'s own `deps.__eventsDispatch` *parameter*
/// threading — reads the same resolved `Callee` now too (its own doc
/// comment has the story: the two checks briefly disagreed on a
/// locally-shadowed `Events` type between this function converting and
/// that one following, confirmed by a fixture that failed `tsc --strict` in
/// between, `1204_events_emit_shadowed_by_local_type`), so the two stay in
/// agreement on every input, not just the ones existing fixtures cover.
pub(crate) fn unit_table_uses_emit(
    table: &UnitTable,
    callees: Option<&HashMap<ExprId, bynk_check::checker::Callee>>,
) -> bool {
    let Some(callees) = callees else {
        debug_assert!(
            false,
            "unit_table_uses_emit: no Callee map for a checked unit"
        );
        return false;
    };
    let mut found = false;
    emitter::walk_unit_table_bodies(table, &mut |e| {
        if !found
            && matches!(
                callees.get(&e.id),
                Some(bynk_check::checker::Callee::Capability { cap, op, .. })
                    if cap == "Events" && op == "emit"
            )
        {
            found = true;
        }
    });
    found
}

// -- Small tree-construction helpers (#1327) ------------------------------
//
// Mirrors `workers.rs`'s/`workers_entry.rs`'s/`tests_emit.rs`'s own local
// helper sets (#1321/#1323/#1325) — this file's own private set, not
// shared, matching this track's own established per-file scoping.

fn ident(s: impl Into<String>) -> TsExpr {
    TsExpr::Ident(s.into())
}

fn str_lit(s: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Str(s.into()))
}

fn member(object: TsExpr, property: impl Into<String>) -> TsExpr {
    TsExpr::Member {
        object: Box::new(object),
        property: property.into(),
    }
}

fn call(callee: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(callee),
        args,
    }
}

fn method_call(object: TsExpr, method: &str, args: Vec<TsExpr>) -> TsExpr {
    call(member(object, method), args)
}

fn const_(name: impl Into<String>, init: TsExpr) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.into()), None, init, None)
}

/// Sort key for a `{ns}Deps` entry, mirroring the pre-conversion code's own
/// `Vec<String>::sort()` over the fully rendered `"{key}: {value}"` text
/// rather than the bare key alone (review of #1328: `deps_entries.sort_by(|a,
/// b| a.0.cmp(&b.0))` is NOT equivalent — it silently reorders whenever one
/// key is a strict prefix of another, e.g. `Db`/`Db2`). The old text sort's
/// tie-break, for a key that is a strict prefix of another, was whatever byte
/// immediately follows it in the longer key compared against the shorter
/// key's own literal `':'` — since capability names are `[A-Za-z][A-Za-z0-9_
/// ]*`, only a digit-suffixed prefix collision (`'0'`-`'9'` are all below
/// `':'`, 0x3A) flips the order: `Db2: ...` sorted before `Db: ...` because
/// `'2'` (0x32) < `':'` (0x3A). Appending `:` to each bare key reproduces
/// that exact tie-break without rendering each entry's value just to sort
/// it — every other key relationship (no shared prefix, or a
/// letter/underscore-suffixed prefix, both above `':'`) is unaffected, since
/// the comparison never reaches the appended `:`.
fn deps_entry_sort_key(key: &str) -> String {
    format!("{key}:")
}

#[allow(clippy::too_many_arguments)]
fn emit_composition_root(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_callees: &HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
    // P6.x (#1232): see `EventSubscriberShape`'s own doc comment. Read by
    // `wants_envelope` below instead of walking a *different, already-
    // consumed* unit's raw `UnitTable` directly.
    unit_event_subscriber_shapes: &HashMap<String, HashMap<String, EventSubscriberShape>>,
    adapter_bindings: &HashMap<String, AdapterBinding>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    // v0.19 (decision 0025, D1): when the program's closure reaches a
    // platform-native unit, composeApp takes an optional `env` and threads it
    // to env-taking first-party providers. A bundle on Cloudflare is a single
    // Worker with `env` at its entry; native-free programs emit the v0.18
    // no-parameter signature unchanged.
    thread_env: bool,
    // Events track, slice 0 (spine #936): the project-wide subscriber table,
    // computed once by the caller (Workers mode needs the same table for its
    // own per-Worker fan-out wiring, so it is shared rather than rebuilt).
    event_subscribers: &BTreeMap<(String, String), Vec<(String, String)>>,
) -> Option<TsProgram> {
    // Identify contexts that consume something whose surface has services.
    let mut needs_compose = false;
    for (name, targets) in unit_consumes {
        if !targets.is_empty()
            && let Some(UnitKind::Context) = kinds.get(name)
        {
            for t in targets {
                if let Some(other) = unit_tables.get(t)
                    && !other.services.is_empty()
                {
                    needs_compose = true;
                }
            }
        }
    }
    // v0.15: also compose when a context uses a consumed context's capability
    // (in a handler or in a provider's `given`) — the consumer must instantiate
    // the provided capability's provider locally.
    if !needs_compose {
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
            if !handler_cross_caps(table, &consumed, &aliases, &flattened).is_empty()
                || table.providers.values().any(|p| {
                    p.given.iter().any(|g| {
                        g.is_cross_context()
                            // v0.18: a bare given flattened from `consumes U
                            // { Cap }` is cross-unit too — its provider lives
                            // in the consumed unit.
                            || (g.prefix().is_none() && flattened.contains_key(g.key()))
                    })
                })
                // Events track, slice 0 (spine #936): a context whose
                // handlers emit needs its own `__eventsDispatch` closure
                // built by compose (§ `discover_event_subscribers`) even
                // when it consumes nothing and no other context consumes
                // it — `Events` is filtered out of `handler_cross_caps`
                // (there is no `EventsProvider`), so without this check a
                // publish-only context would never get a compose entry and
                // its service would simply never be called.
                || unit_table_uses_emit(table, unit_callees.get(name))
                // S4a (#1966): a derived capability's instance is its base's
                // platform provider, built by compose like a flattened one.
                || !table.derived.is_empty()
            {
                needs_compose = true;
                break;
            }
        }
    }
    if !needs_compose {
        return None;
    }

    let mut contexts: Vec<&String> = groups
        .keys()
        .filter(|n| kinds.get(*n) == Some(&UnitKind::Context))
        .collect();
    contexts.sort();

    // The composeApp body is built first so the provider expressions can
    // record every unit namespace they reference (v0.18: an external
    // provider's `given` may pull in *another* adapter's binding — the
    // transitive given-closure — which must then be imported).
    let mut referenced_units: BTreeSet<String> = BTreeSet::new();

    let compose_params: Vec<TsParam> = if thread_env {
        vec![TsParam {
            name: "env".to_string(),
            ty: Some(TsType::named("unknown")),
            optional: true,
        }]
    } else {
        Vec::new()
    };
    let env_ident = if thread_env { Some("env") } else { None };

    // Build each context's deps and surface in dependency-respecting order:
    // a context that consumes another must come after the consumed context,
    // so its `surface` field can reference the already-built surface.
    let mut ordered: Vec<String> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    fn visit(
        node: &str,
        unit_consumes: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        out: &mut Vec<String>,
    ) {
        if visited.contains(node) {
            return;
        }
        visited.insert(node.to_string());
        if let Some(targets) = unit_consumes.get(node) {
            for t in targets {
                visit(t, unit_consumes, visited, out);
            }
        }
        out.push(node.to_string());
    }
    for c in &contexts {
        visit(c, unit_consumes, &mut visited, &mut ordered);
    }

    let mut body: Vec<TsStmt> = Vec::new();

    for ctx_name in &ordered {
        if kinds.get(ctx_name.as_str()) != Some(&UnitKind::Context) {
            continue;
        }
        let Some(table) = unit_tables.get(ctx_name.as_str()) else {
            continue;
        };
        // A context's deps object exists only to feed its `__makeSurface`; a
        // capability-only context (no services) needs neither (v0.15).
        if table.services.is_empty() {
            continue;
        }
        let ns = ctx_name.replace('.', "_");

        let mut deps_entries: Vec<(String, TsExpr)> = table
            .providers
            .keys()
            .map(|cap| {
                let expr = instantiate_provider_ts_expr(
                    ctx_name,
                    cap,
                    unit_tables,
                    unit_consumes,
                    unit_consumes_aliases,
                    unit_flattened,
                    ProviderNamespaces::Bundle,
                    env_ident,
                    None, // Bundle mode has no inbound request (Decision A)
                    &mut referenced_units,
                );
                (cap.clone(), expr)
            })
            .collect();
        // v0.15: cross-context capabilities used directly by handlers become
        // top-level deps fields, instantiated from the providing context.
        {
            let consumed = unit_consumes
                .get(ctx_name.as_str())
                .cloned()
                .unwrap_or_default();
            let aliases = unit_consumes_aliases
                .get(ctx_name.as_str())
                .cloned()
                .unwrap_or_default();
            let flattened = unit_flattened
                .get(ctx_name.as_str())
                .cloned()
                .unwrap_or_default();
            for (key, cctx) in handler_cross_caps(table, &consumed, &aliases, &flattened) {
                let expr = instantiate_provider_ts_expr(
                    &cctx,
                    &key,
                    unit_tables,
                    unit_consumes,
                    unit_consumes_aliases,
                    unit_flattened,
                    ProviderNamespaces::Bundle,
                    env_ident,
                    None, // Bundle mode has no inbound request (Decision A)
                    &mut referenced_units,
                );
                deps_entries.push((key, expr));
            }
        }
        // S4a (#1966): each derived capability's instance.
        let mut derived: Vec<&String> = table.derived.keys().collect();
        derived.sort();
        for cap in derived {
            let expr = instantiate_provider_ts_expr(
                ctx_name,
                cap,
                unit_tables,
                unit_consumes,
                unit_consumes_aliases,
                unit_flattened,
                ProviderNamespaces::Bundle,
                env_ident,
                None,
                &mut referenced_units,
            );
            deps_entries.push((cap.clone(), expr));
        }
        // Events track, slice 0 (spine #936): a context whose handlers emit
        // gets an `__eventsDispatch` closure built here — Bundle/node mode
        // has no isolate boundary to cross, so this dispatches in-process
        // directly into each subscriber's own `on event` handler (`.event`,
        // the object method `emit_service` gives `HandlerKind::Event`),
        // reusing whatever deps that subscriber context already builds in
        // this same loop (referenced by name; the arrow function body isn't
        // evaluated until well after every `const ...Deps` in `composeApp`
        // has run, so declaration order here doesn't matter). A publisher
        // with no subscribers still gets the field — its type is required —
        // just with an empty switch.
        if unit_table_uses_emit(table, unit_callees.get(ctx_name)) {
            let mut cases = String::new();
            for name in table.events.keys() {
                let Some(subs) = event_subscribers.get(&(ctx_name.clone(), name.clone())) else {
                    continue;
                };
                // ADR 0284: subscriber failure isolation — one subscriber's
                // throw is caught and logged, not left to abort delivery to
                // its siblings or propagate into the already-committed
                // publishing handler. Mirrors the Cloudflare fan-out DO's own
                // per-subscriber try/catch (`emit_events_fanout_do`) so the
                // two targets agree on this guarantee, not just on delivery.
                let calls: Vec<String> = subs
                    .iter()
                    .map(|(sub_ctx, sub_svc)| {
                        let sub_ns = sub_ctx.replace('.', "_");
                        // Events track, slice 2 (spine #936): the envelope
                        // is only forwarded to a subscriber that declared
                        // the optional second `env: EventEnvelope`
                        // parameter — a subscriber that kept `on event(e:
                        // E)` sees no change to its call at all. Slice 4
                        // (#985): also forwarded when the subscriber's
                        // protocol carries a `via schema(N)` clause, even if
                        // undeclared — `emit_service` inserts a synthetic
                        // `env` parameter in that case, and needs the value
                        // to line up positionally.
                        let wants_envelope = unit_event_subscriber_shapes
                            .get(sub_ctx)
                            .and_then(|m| m.get(sub_svc))
                            .is_some_and(|shape| shape.two_param_handler || shape.schema_dispatch);
                        // P7.2: deferred, not narrowed — a first attempt used the
                        // event's own bare name directly (`EventDecl::as_type_decl`
                        // keys the synthetic `TypeDecl` on it), on the theory that
                        // an event doubles as a named type. It broke real
                        // `tsc --strict` fixtures ("Cannot find name") — the bare
                        // name isn't necessarily in scope at this dispatch site
                        // (cross-context: publisher and subscriber are different
                        // units), the same qualification problem
                        // `ts_type_ref_qualified_ts_type` exists to solve for
                        // handler wrappers elsewhere in this crate. Needs the same
                        // kind of scoped qualification, not a bare name — more
                        // than a same-line fix.
                        let call_args = if wants_envelope {
                            "ev.payload as any, ev.envelope".to_string()
                        } else {
                            "ev.payload as any".to_string()
                        };
                        format!(
                            "try {{ await {sub_ns}.{sub_svc}.event({call_args}, {sub_ns}Deps); }} catch (e) {{ globalThis.console.error(\"EventsFanout delivery failed\", {{ event: ev.type, service: {sub_svc:?}, error: String(e) }}); }}"
                        )
                    })
                    .collect();
                cases.push_str(&format!("case {name:?}: {{ {} break; }} ", calls.join(" ")));
            }
            // Decision B (#1327): the closure's own body is a genuine block
            // statement (`for`/`switch`/`try`-`catch` nested), not an
            // expression. #1435 later gave `TsExpr::Arrow` a real block-body
            // shape (`TsArrowBody::Block`) — but that variant's own printer
            // reuses the compact-statement-list renderer `TsStmtKind::
            // InlineBlock` already shares (one physical line, semicolon-
            // separated top-level statements), which cannot flatten a nested
            // `for`/`switch` onto one line the way this closure's own real
            // content needs; new "flatten every nested statement to one
            // line" printer machinery for that would still be disproportionate
            // to what the 3 real fixtures reaching this closure need. Kept as
            // the same nested `format!` calls as before conversion, just fed
            // into a real `Arrow` node's `body` as one opaque `TsExpr::Ident`
            // — the same "opaque text carrier" precedent `workers.rs`'s/
            // `workers_entry.rs`'s own `claim_predicate_to_js` output already
            // uses.
            let dispatch_body =
                format!("{{ for (const ev of events) {{ switch (ev.type) {{ {cases}}} }} }}");
            let arrow = TsExpr::Arrow {
                params: vec![TsParam {
                    name: "events".to_string(),
                    ty: Some(TsType::named_with_args(
                        "globalThis.Array",
                        vec![TsType::named(crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE)],
                    )),
                    optional: false,
                }],
                is_async: true,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(ident(dispatch_body)))),
            };
            deps_entries.push(("__eventsDispatch".to_string(), arrow));
        }
        // #2683: a `call` handler that calls `within` (or sends) reads
        // `deps.__exec`. The bundle has no platform execution context, so what
        // it is handed is kept alive by the process; a fault in it, with no
        // handler left to raise it to, is logged.
        if table.services.values().any(|s| {
            s.handlers
                .iter()
                .any(|h| crate::emitter::call_handler_needs_exec(&table.exec_plan, h))
        }) {
            deps_entries.push((
                "__exec".to_string(),
                ident("{ waitUntil(promise: globalThis.Promise<unknown>): void { promise.catch((e) => globalThis.console.error(\"an abandoned effect failed\", String(e))); } }"),
            ));
        }
        deps_entries.sort_by_cached_key(|(k, _)| deps_entry_sort_key(k));

        let mut surface_entries: Vec<(String, TsExpr)> = Vec::new();
        if let Some(targets) = unit_consumes.get(ctx_name.as_str()) {
            let aliases = unit_consumes_aliases
                .get(ctx_name.as_str())
                .cloned()
                .unwrap_or_default();
            let mut alias_for: HashMap<String, String> = HashMap::new();
            for (alias, target) in &aliases {
                alias_for.insert(target.clone(), alias.clone());
            }
            let mut sorted_targets = targets.clone();
            sorted_targets.sort();
            for t in &sorted_targets {
                let Some(other) = unit_tables.get(t) else {
                    continue;
                };
                if other.services.is_empty() {
                    continue;
                }
                let surface_key = alias_for
                    .get(t)
                    .cloned()
                    .unwrap_or_else(|| t.rsplit('.').next().unwrap_or(t.as_str()).to_string());
                let t_ns = t.replace('.', "_");
                // v0.54 (#655): a consumed context with an `on call … by c: Caller`
                // handler needs the *caller's* qualified name (this context) threaded
                // into that handler's deps as its `CallerId` identity (ADR 0092). The
                // shared `{t_ns}Surface` (built for the top-level entry with the
                // provider's own name) would carry the wrong caller, so build a
                // per-consumer surface instead. A caller-free provider keeps the
                // shared instance — byte-unchanged.
                let entry = if context_binds_caller(other) {
                    method_call(
                        ident(t_ns.clone()),
                        "__makeSurface",
                        vec![ident(format!("{t_ns}Deps")), str_lit(ctx_name.as_str())],
                    )
                } else {
                    ident(format!("{t_ns}Surface"))
                };
                surface_entries.push((surface_key, entry));
            }
        }
        if !surface_entries.is_empty() {
            deps_entries.push(("surface".to_string(), TsExpr::object(surface_entries)));
        }
        // #1327: the pre-conversion `format!("  const {ns}Deps = {{ {} }};",
        // deps_entries.join(", "))` template always has a space on each side
        // of its `{}` slot — with zero entries that literally produces
        // `"{  }"` (a *double* space), not the tight `"{}"` the ordinary
        // single-line `TsExpr::Object` empty-entries shortcut renders — the
        // same real, reachable quirk `workers.rs`'s own conversion (#1321)
        // found and carried for its own `deps` object, reachable here too (a
        // services-having context with no providers, no cross-caps, no
        // emit, and no consumed-service surface — `98_cross_context_call_
        // with_alias` and 6 other real fixtures hit exactly this).
        let deps_init = if deps_entries.is_empty() {
            ident("{  }")
        } else {
            TsExpr::object(deps_entries)
        };
        body.push(const_(format!("{ns}Deps"), deps_init));
        if !table.services.is_empty() {
            // The top-level entry addresses the context directly; there is no
            // calling context, so a `by c: Caller` handler reached this way reads
            // the context's own qualified name (a stable, non-empty `CallerId`
            // within the single-trust-domain bundle).
            let mut make_surface_args = vec![ident(format!("{ns}Deps"))];
            if context_binds_caller(table) {
                make_surface_args.push(str_lit(ctx_name.as_str()));
            }
            body.push(const_(
                format!("{ns}Surface"),
                method_call(ident(ns.clone()), "__makeSurface", make_surface_args),
            ));
        }
    }

    // #1327: the pre-conversion code unconditionally wrote one blank line
    // between the last `const ...Deps`/`const ...Surface` and the `return`
    // (`out.push('\n')`, run once regardless of how many contexts the loop
    // above actually pushed) — `TsStmtKind::Blank` (#1323) is the tree's own
    // equivalent, needed here since the printer's own "blank line between
    // top-level declarations" policy only separates entries in
    // `TsProgram.stmts` itself, not statements inside one function body.
    body.push(TsStmt::blank(None));

    // Export per-context surfaces under a top-level object.
    let mut return_entries: Vec<(String, TsExpr)> = Vec::new();
    for ctx_name in &contexts {
        let Some(table) = unit_tables.get(ctx_name.as_str()) else {
            continue;
        };
        if table.services.is_empty() {
            continue;
        }
        let ns = ctx_name.replace('.', "_");
        let key = ctx_name.rsplit('.').next().unwrap_or(ctx_name.as_str());
        return_entries.push((key.to_string(), ident(format!("{ns}Surface"))));
    }
    body.push(TsStmt::return_stmt(
        Some(TsExpr::multiline_object(return_entries)),
        None,
    ));

    // Assemble the header now that the body has recorded which units its
    // provider expressions reference.
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynkc — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::comment("composition root", None));

    // Import every context as a namespace.
    for ctx_name in &contexts {
        let dir = emitter::ts_specifier(&commons_dir_for(ctx_name));
        let ns = ctx_name.replace('.', "_");
        program.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: ns,
                from: format!("./{dir}.js"),
            },
            None,
        ));
    }
    // v0.17: import each consumed adapter's binding module — the external
    // provider classes live there, not in the adapter's interface module.
    // v0.18: plus every adapter the provider expressions referenced through
    // the transitive given-closure (an adapter's external provider may depend
    // on another adapter's capability, spec §4.5).
    let mut consumed_adapters: Vec<String> = unit_consumes
        .iter()
        .filter(|(name, _)| kinds.get(*name) == Some(&UnitKind::Context))
        .flat_map(|(_, targets)| targets.iter().cloned())
        .chain(referenced_units.iter().cloned())
        .filter(|t| adapter_bindings.contains_key(t))
        .collect();
    consumed_adapters.sort();
    consumed_adapters.dedup();
    for adapter in &consumed_adapters {
        let ns = adapter.replace('.', "_");
        let module =
            emitter::ts_specifier(&adapter_bindings[adapter].output_path.with_extension("js"));
        program.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: format!("{ns}__binding"),
                from: format!("./{module}"),
            },
            None,
        ));
    }

    program.push(TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::Function {
            name: "composeApp".to_string(),
            generics: Vec::new(),
            params: compose_params,
            return_type: None,
            body,
            is_async: false,
            inline: false,
        })),
        None,
    ));

    Some(program)
}

// -- internals --

/// Context passed to the emitter so it can resolve cross-file and
/// cross-unit references into TypeScript import statements.
/// The schema registry's reconciled versions, threaded from `run_checks` to
/// each unit's emission: events keyed `<unit>.<Event>` (#980), agents keyed
/// `<unit>.<Agent>` (state-migrations S1, #2211). Both empty when the build
/// has no lock.
#[derive(Debug, Default)]
pub(crate) struct SchemaVersions {
    pub events: HashMap<String, i64>,
    pub agents: HashMap<String, bynk_check::schema_registry::AgentVersions>,
}

pub(crate) struct EmitProjectCtx {
    /// Source path of the file being emitted (relative to project root).
    pub source_path: PathBuf,
    /// The file's project-relative identity path, as the test runner reports it in
    /// an `expect`'s location. A violated declaration's location is built from it (#2272).
    pub identity_path: String,
    /// Joined name of the commons or context this file belongs to.
    pub commons_name: String,
    /// Which file declares each name in the local unit.
    pub file_decl_index: FileDeclIndex,
    /// For each imported name, the joined name of the unit it came from.
    pub imported_from: HashMap<String, String>,
    /// For each imported name, the kind (commons vs context) of the source unit.
    pub imported_from_kind: HashMap<String, UnitKind>,
    /// For each imported unit, the file path that declares each name.
    pub imported_decl_paths: HashMap<String, HashMap<String, PathBuf>>,
    /// What kind of unit this is.
    pub unit_kind: UnitKind,
    /// For contexts: this context's qualified name (used as the brand for
    /// rebranded mixed-in types and exported types).
    pub owning_context: Option<String>,
    /// For contexts: exports of each consumed context (so the emitter knows
    /// which names to import and how).
    pub exports_for_consumed: HashMap<String, HashMap<String, Visibility>>,
    /// For contexts: full cross-context information (consumed contexts,
    /// aliases, consumed services and types). Mirrors what the resolver
    /// and checker see (v0.6).
    pub cross_context: resolver::CrossContextInfo,
    /// v0.8 build target. Workers mode reroutes cross-context calls through
    /// Service Bindings and adds per-Worker entry/composition artefacts.
    pub target: BuildTarget,
    /// Agent names declared in this unit. The body lowering uses this set
    /// to recognise `Agent(key)` construction and `agent_instance.method(...)`
    /// dispatch.
    pub local_agents: HashSet<String>,
    /// #527: for each local agent with `given` capabilities, the TS
    /// expression building those deps DO-side (workers contexts only; the DO
    /// wire cannot carry providers). Consumed by `emit_agent`'s fetch branch.
    pub agent_given_deps: HashMap<String, String>,
    /// #527: extra import lines `handlers.ts` needs for the expressions above.
    pub extra_import_lines: Vec<String>,
    /// #527: for each local agent, each `on call` method's `given` capability
    /// list. The lowering records which agent methods a handler body calls so
    /// the handler's emitted deps *type* carries the callee's capabilities —
    /// the runtime deps value (built by compose) always did.
    pub agent_method_givens: HashMap<String, HashMap<String, Vec<CapRefIr>>>,
    /// #1961: the consumed contexts the context's agents call
    /// ([`emitter::agent_cross_context_targets`]), unit-wide. A handler that
    /// constructs an agent names them in its `deps.env` type.
    pub agent_called_contexts: Vec<String>,
    /// v0.47: the context's actor declarations (merged across files), keyed by
    /// name. Used to resolve a handler's Bearer verification seam in `emit.rs`
    /// regardless of which file declares the actor.
    pub actors: HashMap<String, ActorDecl>,
    /// Events slice 3b (#978): each locally-declared event's resolved
    /// `@schema(N)` version (or `1` if absent), merged across files the same
    /// way `actors` is above — `Events.emit[E]`'s lowering site only has
    /// `E`'s bare name (the turbofish type argument), never its declaration,
    /// so this is threaded down to `ModuleCtx`/`LowerCtx` rather than
    /// re-derived from the per-file synthetic `Commons` `lower.rs` otherwise
    /// sees (which would silently miss an event declared in a sibling file).
    pub event_schema_versions: HashMap<String, i64>,
    /// State-migrations S1 (#2211): each locally-declared agent's lock
    /// `version` and oldest readable stored version, keyed by agent name.
    /// Empty when the build has no lock (`SchemaLock::Off`), in which case the
    /// agent emits no version logic at all (Decision A).
    pub agent_schema_versions: HashMap<String, bynk_check::schema_registry::AgentVersions>,
    /// v0.17: consumed unit names that are adapters. An adapter is not a Worker,
    /// so in workers mode its capability types are imported from its root module
    /// (`<adapter>.ts`), not from a per-Worker `handlers.ts`.
    pub consumed_adapters: HashSet<String>,
    /// #2045: the types of consumed *contexts* (adapters aside) that carry
    /// attached methods. A call to one (`b.big()` lowers to `Batch.big(b)`) runs
    /// the callee's own code from its companion object, so that name is a real
    /// value import; every other consumed context type imports type-only.
    pub consumed_types_with_methods: HashSet<String>,
    /// Slice 2: the extension emitted import specifiers use (`.js` default; `.ts`
    /// for the `bynkc test --inspect` debug build). Consulted by `runtime_import_for`
    /// and the sibling/cross-commons specifier helpers.
    pub import_ext: ImportExt,
    /// v0.115 (testing track slice 3): emit the function-contract call-site guard
    /// (dev/test profile). Stripped in the deploy build for zero runtime cost.
    pub contracts: bool,
    /// v0.119 (testing track slice 7, ADR 0155): agent names a `for all run:
    /// History[Agent]` property in this project drives. Only these agents gain the
    /// exported `__bynkDriveHistory_<Agent>` test-support driver — every other
    /// agent's emission is byte-for-byte unchanged.
    pub history_target_agents: HashSet<String>,
    /// #1883, slice 1b: each declared asset's served URL path, keyed by its
    /// normalised source path (`assets::declared_served_paths`) — the value
    /// of the asset's `.url`.
    pub asset_served: BTreeMap<String, String>,
    /// #2387, slice 2: each declared embed's text, keyed by its normalised
    /// source path (`embeds::read_declared_embeds`) — the value of the
    /// embed's constant.
    pub embed_text: BTreeMap<String, String>,
    /// v0.132.1 (#481): for a context, the user-defined attached methods of each
    /// `uses`-imported refined/opaque type, keyed by the type's name and sorted
    /// by method name. The context's own `TypedCommons` merges the imported
    /// *types* but not their fn items, so `emit_context_rebrands` reads this to
    /// forward `Cents.fromInt(…)` and friends onto the rebranded const. Empty
    /// for commons units and for contexts with no such imports.
    ///
    /// P6.18: each entry is a resolved [`FnSig`] (the declaring unit's own
    /// `params`/`return_ty`, already `TyId`-typed), not a raw `FnDecl` —
    /// see [`build_emit_unit_ctx`]'s own doc comment for why.
    pub imported_methods: HashMap<String, Vec<FnSig>>,
    /// Which conditional `runtime.ts` helpers this file's emission referenced.
    ///
    /// Unlike every field above, this is an **output**, not an input: emission
    /// writes it (through `&self`, via interior mutability) and the header /
    /// import post-pass reads it back. It rides on the context because the
    /// producers — the `Bytes` kernel in `lower`, the boundary codecs in
    /// `serialisation`, the ICU formatters in `emit` — already receive `&ctx`,
    /// so no other signature has to change to carry the fact up.
    ///
    /// One `EmitProjectCtx` is built per emitted file, immediately before its
    /// `emit_project` call, so the flags cannot leak between files. Replaces a
    /// substring scan of the generated text; see `emitter::runtime_use`.
    pub runtime_use: crate::emitter::RuntimeUse,
}

impl EmitProjectCtx {
    /// #2045: on `workers`, the types this unit imports from a consumed
    /// *context* (not an adapter): each is another Worker's, so this module
    /// names it in type position only and builds its sum variants inline.
    pub(crate) fn consumed_context_types(&self) -> HashSet<String> {
        if self.target != BuildTarget::Workers || self.unit_kind != UnitKind::Context {
            return HashSet::new();
        }
        self.imported_from
            .iter()
            .filter(|(n, owner)| {
                self.imported_from_kind.get(n.as_str()) == Some(&UnitKind::Context)
                    && !self.consumed_adapters.contains(owner.as_str())
            })
            .map(|(n, _)| n.clone())
            .collect()
    }

    pub fn commons_path(name: &str) -> PathBuf {
        commons_dir_for(name)
    }
}

#[allow(dead_code)]
fn _ensure_components_used(_p: &Path) {
    let _ = Component::CurDir;
}

/// v0.177 (#643, review of #658): the cross-context services a context actually
/// **calls**, as `consumed context → service names`.
///
/// This is not the same as "every service the dependency provides", and the
/// difference is the difference between a gate that reports what it *knows* is
/// skewed and one that reports what merely *differs*. If `payment` provides
/// `authorise` and `refund`, `orders` calls only `authorise`, and `refund`'s
/// contract changed, then recording `refund` in `orders`'s `expects` would refuse
/// `deploy --context orders` over a service `orders` never touches and whose
/// runtime check could never fire. ADR 0200 Decision E rejects a per-*context*
/// hash for exactly this reason — that it becomes a deployment tax — and a
/// per-context *gate* over per-service hashes would reintroduce it one layer up.
///
/// So the manifest's `expects` mirrors the runtime check's granularity: one entry
/// per call site, discovered the same way the lowering discovers it — an ident
/// chain on the receiver that resolves to a consumed context.
fn called_cross_context_services(
    table: &UnitTable,
    consumed: &[String],
    // #1187's slice 6 plumbing: reads the checker's own already-resolved
    // `Callee::Cross { unit, service }` (`RunChecks::Checked::unit_callees`'s
    // own doc comment has the full grounding) instead of re-deriving
    // cross-context-ness by flattening a receiver's own ident chain and
    // string-matching it against `consumed`/`aliases` — the identical
    // resolution `CrossContextInfo::resolve_prefix` already did once, at
    // check time, per call site. `consumed` stays, purely as the cheap
    // early-out below: an empty `consumes` list means no `Callee::Cross`
    // could exist in this unit's own bodies regardless, so skip the walk.
    callees: Option<&HashMap<ExprId, bynk_check::checker::Callee>>,
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    let mut out: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    if consumed.is_empty() {
        return out;
    }
    // See `unit_table_uses_emit`'s own matching `debug_assert` (review of
    // #1202) — `consumed` non-empty means this unit certified with a real
    // `consumes`, so `callees` missing here is the same "invariant broke a
    // thousand lines away" case, just silently thinning the contracts
    // manifest's `expects` instead of silently disabling emission.
    let Some(callees) = callees else {
        debug_assert!(
            false,
            "called_cross_context_services: no Callee map for a checked unit with a non-empty \
             consumes list"
        );
        return out;
    };
    emitter::walk_unit_table_bodies(table, &mut |e| {
        if let Some(bynk_check::checker::Callee::Cross { unit, service }) = callees.get(&e.id) {
            out.entry(unit.clone()).or_default().insert(service.clone());
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Review of #1904: the `_headers` glob must cover every served path, or
    /// every asset silently loses its immutable cache lifetime. Both come
    /// from `SERVED_PREFIX`; this pins that they still agree.
    #[test]
    fn the_headers_glob_covers_every_served_path() {
        let headers = crate::emitter::wrangler::asset_headers();
        let glob = headers.lines().next().expect("a path line");
        let prefix = glob.strip_suffix('*').expect("a prefix glob");
        for source in ["assets/board.css", "assets/css/a.min.js", "assets/LICENSE"] {
            let served = bynk_check::assets::served_path(source, b"x");
            assert!(served.starts_with(prefix), "{served} not under {glob}");
            let staged = worker_assets_dir("w").join(served.trim_start_matches('/'));
            assert!(
                is_worker_asset_path(&staged),
                "{} is owned",
                staged.display()
            );
        }
    }

    /// P4b (#2608): `.sql` files are pruned only inside a Worker's copied
    /// migrations, never elsewhere in an output directory.
    #[test]
    fn is_worker_migration_path_matches_only_files_inside_a_worker_migrations_dir() {
        assert!(is_worker_migration_path(Path::new(
            "workers/shop/migrations/shop-orders/0001_create.sql"
        )));
        for p in [
            "workers/shop/migrations",
            "workers/shop/schema.sql",
            "migrations/shop-orders/0001_create.sql",
            "workers/shop/migrations/../../../x.sql",
        ] {
            assert!(!is_worker_migration_path(Path::new(p)), "{p}");
        }
    }

    /// #1883, slice 0: the ownership rule `write_output`'s reconciliation
    /// prunes by. Only files *inside* a Worker's assets directory qualify —
    /// not the directory itself, not a same-named directory at another depth,
    /// and not a sibling of it in the Worker's own output.
    #[test]
    fn is_worker_asset_path_matches_only_files_inside_a_worker_assets_dir() {
        let owned = [
            "workers/compat-board/assets/_headers",
            "workers/compat-board/assets/_assets/board.3f9c2a.css",
        ];
        for p in owned {
            assert!(is_worker_asset_path(Path::new(p)), "{p} is owned");
        }
        let not_owned = [
            "workers/compat-board/assets",
            "workers/compat-board/wrangler.toml",
            "workers/compat-board/notes.css",
            "workers/assets/x.css",
            "assets/x.css",
            "compat/assets/x.css",
            "workers/compat-board/sub/assets/x.css",
            // Review of #1902: `components()` keeps `..`, and `dir.join`
            // would then write outside the output root entirely.
            "workers/compat-board/assets/../../../../etc/cron.d/x",
            "workers/compat-board/assets/_assets/../x.css",
            "workers/compat-board/assets/..",
        ];
        for p in not_owned {
            assert!(!is_worker_asset_path(Path::new(p)), "{p} is not owned");
        }
        assert_eq!(
            worker_assets_dir("compat-board").join("_headers"),
            PathBuf::from(owned[0]),
            "the producer's directory and the pruner's rule agree"
        );
    }

    /// Review of #1328: the pre-conversion code sorted the fully rendered
    /// `"{key}: {value}"` text, so a key that is a strict prefix of another
    /// (`Db`/`Db2`) sorted by the shorter key's own `':'` losing to the
    /// longer key's next byte, a digit — `Db2: ...` sorted before `Db:
    /// ...`. A bare-key sort (`"Db" < "Db2"`) gets this backwards.
    /// `deps_entry_sort_key` must reproduce the original order exactly.
    #[test]
    fn deps_entry_sort_key_reproduces_the_pre_conversion_full_text_sort_order() {
        let mut keys = vec!["Db2".to_string(), "Db".to_string()];
        keys.sort_by_cached_key(|k| deps_entry_sort_key(k));
        assert_eq!(
            keys,
            vec!["Db2".to_string(), "Db".to_string()],
            "Db2 must sort before Db, matching the old full-text sort"
        );

        // A letter/underscore-suffixed prefix collision is unaffected —
        // matches plain key ordering both before and after this fix.
        let mut keys = vec!["Db_pool".to_string(), "Db".to_string()];
        keys.sort_by_cached_key(|k| deps_entry_sort_key(k));
        assert_eq!(keys, vec!["Db".to_string(), "Db_pool".to_string()]);

        // No shared prefix at all: ordinary alphabetical order, unaffected.
        let mut keys = vec!["Queue".to_string(), "Cache".to_string()];
        keys.sort_by_cached_key(|k| deps_entry_sort_key(k));
        assert_eq!(keys, vec!["Cache".to_string(), "Queue".to_string()]);
    }

    /// Regression (code review of #1114): a `sources` key that matches none
    /// of `trees`'s roots (shouldn't happen for a well-formed map — see
    /// `sources_to_discovered`'s own doc) must fall back to the *last* tree,
    /// matching the pre-R3.9 two-tree `partition`'s fallback, not silently
    /// switch to the first.
    #[test]
    fn sources_to_discovered_unmatched_key_falls_back_to_the_last_tree() {
        let trees = vec![
            (PathBuf::from("/proj/src"), PathBuf::from("src")),
            (PathBuf::from("/proj/tests"), PathBuf::from("tests")),
        ];
        let mut sources = HashMap::new();
        sources.insert(PathBuf::from("/proj/src/a.bynk"), "commons a\n".to_string());
        sources.insert(
            PathBuf::from("/elsewhere/stray.bynk"),
            "commons stray\n".to_string(),
        );
        let (_, discovered) = sources_to_discovered(&sources, &trees);
        let buckets = discovered.expect("a sources map always yields Some(Discovered)");
        assert_eq!(buckets[0], vec![PathBuf::from("/proj/src/a.bynk")]);
        assert_eq!(
            buckets[1],
            vec![PathBuf::from("/elsewhere/stray.bynk")],
            "an unmatched key must land in the last tree, not the first"
        );
    }

    /// Content-ownership track (#1086) slice 4: this crate's own
    /// `#[cfg(test)]` module can't depend on the cross-crate `bynk-testkit`
    /// (that crate depends on `bynk-emit` — a cyclic dev-dependency, the
    /// same class of issue slice 3 found for `bynk-ide`). Mirrors
    /// `bynk-testkit::compile_options_split` in-crate instead, directly
    /// against this crate's own `Roots`/`discover_project_files` — no second
    /// resolution to drift from the first. Keyed by the literal discovered
    /// path, not canonicalised, matching `bynk-testkit`'s own convention
    /// (canonicalising broke a project-consistency check the hard way in
    /// slice 3).
    /// Content-ownership track (#1086) slice 5: this crate's own tests can't
    /// depend on `bynk-testkit` (cyclic — `bynk-testkit` depends on
    /// `bynk-emit`), so its handful of sites that build a `Roots` and need
    /// real disk content for it mirror `bynk-testkit`'s own read, in-crate.
    fn read_disk_sources(roots: &Roots) -> HashMap<PathBuf, String> {
        discover_project_files(roots)
            .into_iter()
            .filter_map(|p| {
                let content = std::fs::read_to_string(&p).ok()?;
                Some((p, content))
            })
            .collect()
    }

    fn compile_options_split_with_sources(
        project_root: PathBuf,
        paths: ProjectPaths,
    ) -> CompileOptions {
        let roots = Roots::Split {
            project_root: project_root.clone(),
            paths: paths.clone(),
        };
        let sources = read_disk_sources(&roots);
        CompileOptions::split(project_root, paths).sources(sources)
    }

    // -- Finding #55/#65: memoized first-party parse must not leak gating ----

    /// The first-party parse cache (`firstparty_parsed`) is keyed per-source,
    /// not per-project — `phase_parse`'s `consumes`/`uses` gating still runs
    /// fresh for every project. Two in-memory projects that gate in
    /// *different* first-party units, compiled back-to-back in the same
    /// process (so both share the same cache), must each see exactly their
    /// own gated-in set: `bynk.map` (which itself `uses bynk.list`) for the
    /// first, `bynk.string` alone for the second — never the other's.
    #[test]
    fn firstparty_cache_does_not_leak_gating_across_projects() {
        let out = compile_in_memory(
            "commons app.only_map\n\nuses bynk.map\n\nfn f() -> Int { 1 }\n",
            BuildTarget::Bundle,
            Default::default(),
        )
        .unwrap_or_else(|_| panic!("`uses bynk.map` should compile"));
        let paths: Vec<String> = out
            .artefacts
            .docs
            .keys()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(paths.iter().any(|p| p == "bynk/map.ts"), "{paths:?}");
        assert!(
            paths.iter().any(|p| p == "bynk/list.ts"),
            "bynk.map itself uses bynk.list, so list must be injected too: {paths:?}"
        );
        assert!(
            !paths.iter().any(|p| p.starts_with("bynk/string")),
            "a project that never uses bynk.string must not gain it: {paths:?}"
        );

        let out2 = compile_in_memory(
            "commons app.only_string\n\nuses bynk.string\n\nfn f() -> String { \"x\" }\n",
            BuildTarget::Bundle,
            Default::default(),
        )
        .unwrap_or_else(|_| panic!("`uses bynk.string` should compile"));
        let paths2: Vec<String> = out2
            .artefacts
            .docs
            .keys()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(paths2.iter().any(|p| p == "bynk/string.ts"), "{paths2:?}");
        assert!(
            !paths2.iter().any(|p| p.starts_with("bynk/map")),
            "the shared first-party parse cache must not leak the first \
             project's gating into this one: {paths2:?}"
        );
    }

    // -- Finding #64: `check_project` must not bail past an earlier error --

    /// `compile_project`'s `Mode::Build` bails at the first structural error
    /// (here, `exports capability` naming an undeclared capability) and never
    /// reaches the per-unit checking pass — so a completely separate file's
    /// test-body type error is silently dropped. `check_project`'s
    /// `Mode::Analyse` must report both.
    #[test]
    fn check_project_reports_a_test_body_error_past_an_earlier_structural_error() {
        let root = scratch_project(
            "check_past_error",
            &[
                ("bynk.toml", "[project]\nname = \"c\"\n"),
                (
                    "src/greet.bynk",
                    "context greet {\n  exports capability { Bogus }\n}\n",
                ),
                (
                    "src/math.bynk",
                    "commons math {\n  fn double(n: Int) -> Int { n * 2 }\n}\n",
                ),
                (
                    "tests/math_test.bynk",
                    "suite math\n\ncase \"broken\" {\n  let x: Int = \"not an int\"\n  expect x == 1\n}\n",
                ),
            ],
        );
        let options = compile_options_split_with_sources(
            root.to_path_buf(),
            try_read_project_paths(&root).expect("well-formed fixture manifest"),
        );

        let check = check_project(&options);
        assert!(check.has_errors());
        let categories: Vec<&str> = check.errors.iter().map(|ae| ae.error.category).collect();
        assert!(
            categories.contains(&"bynk.exports.undeclared_capability"),
            "{categories:?}"
        );
        assert!(
            categories.contains(&"bynk.types.let_annotation_mismatch"),
            "check_project must still report the test body's own type error \
             past the earlier structural error: {categories:?}"
        );

        // The contrast: `compile_project`'s bail-fast `Mode::Build` is the
        // defect `check_project` exists to route `bynk check` around.
        let failure = match compile_project(&options) {
            Err(f) => f,
            Ok(_) => panic!("the structural error must still fail a real build"),
        };
        let failure_categories: Vec<&str> =
            failure.errors.iter().map(|ae| ae.error.category).collect();
        assert!(
            !failure_categories.contains(&"bynk.types.let_annotation_mismatch"),
            "compile_project must still bail before the test-body check runs \
             (documents why check_project is a separate entry point): {failure_categories:?}"
        );
    }

    // -- Slice 0: file identity is not the unit-validation path ---------------

    /// The defect, reproduced hermetically: two `include` roots each holding a
    /// file of the same name. Before slice 0 this yielded
    /// `["thing.bynk", "thing.bynk"]` — `parse_tree` stripped each tree's own
    /// root, so the two were indistinguishable and any consumer mapping by that
    /// key dropped one.
    ///
    /// This is the measurement from the track doc's §3.1, inverted into an
    /// assertion. It deliberately does **not** read `../examples/todo`:
    /// `bynk-emit` is published without an `exclude` list, so a test reaching
    /// outside the crate would fail a standalone `cargo test` on the released
    /// tarball. That `examples/todo` itself resolves is #647's regression
    /// fixture, where the LSP can actually observe it.
    #[test]
    fn split_roots_give_each_file_a_distinct_identity() {
        let root = scratch_project(
            "identity",
            &[
                ("bynk.toml", "[project]\nname = \"identity\"\n"),
                ("src/thing.bynk", "context thing\n"),
                ("tests/thing.bynk", "suite thing\n"),
            ],
        );
        let roots = Roots::Split {
            project_root: root.to_path_buf(),
            paths: try_read_project_paths(&root).expect("well-formed fixture manifest"),
        };
        let trees = roots.trees();
        assert_eq!(
            trees,
            vec![
                (root.join("src"), PathBuf::from("src")),
                (root.join("tests"), PathBuf::from("tests")),
            ],
            "the fixture must actually be two-rooted"
        );
        let sources = read_disk_sources(&roots);
        let run = run_checks(
            &trees,
            BuildTarget::Bundle,
            Platform::default(),
            ImportExt::Js,
            Mode::Analyse,
            &sources,
            &roots.excludes(),
            None,
            false,
            true,
            &SchemaLock::Off,
            None,
            roots.project_root(),
            None,
            None,
            None,
            false,
            &Arc::new(Types::new()),
        );
        let snapshots = match run {
            RunChecks::Bailed { snapshots, .. } => snapshots,
            RunChecks::Checked { snapshots, .. } => snapshots,
        };
        let mut keys: Vec<String> = snapshots
            .iter()
            .map(|(p, _)| p.to_string_lossy().replace('\\', "/"))
            .collect();
        keys.sort();
        assert_eq!(
            keys,
            vec!["src/thing.bynk", "tests/thing.bynk"],
            "a file's identity must be project-relative and unique across include roots"
        );
    }

    /// Regression (code review of #1114): an adapter declared outside the
    /// first `include` tree used to have its `binding` module resolved
    /// against `trees[0]` unconditionally (`phase_group`'s old `src_root:
    /// &Path` parameter) — a project with `include = ["src", "adapters",
    /// "tests"]` and an adapter under `adapters/` would look for its binding
    /// under `src/`, fail to find it, and report `bynk.adapter.no_binding`
    /// even though the binding file exists right beside the adapter.
    #[test]
    fn adapter_binding_resolves_against_its_own_include_tree() {
        let root = scratch_project(
            "adapter_binding_tree",
            &[
                (
                    "bynk.toml",
                    "[project]\nname = \"a\"\n\n[paths]\ninclude = [\"src\", \"adapters\", \"tests\"]\n",
                ),
                (
                    "src/math.bynk",
                    "commons math {\n  fn double(n: Int) -> Int { n * 2 }\n}\n",
                ),
                (
                    "adapters/payments.bynk",
                    "adapter payments {\n  binding \"./payments.binding.ts\"\n\n  exports capability { Pay }\n\n  capability Pay {\n    fn charge(amount: Int) -> Effect[String]\n  }\n\n  provides Pay = RealPay\n}\n",
                ),
                (
                    "adapters/payments.binding.ts",
                    "import type { Pay } from \"./payments.js\";\n\nexport class RealPay implements Pay {\n  async charge(amount: number): globalThis.Promise<string> {\n    return \"ok\";\n  }\n}\n",
                ),
            ],
        );
        let options = compile_options_split_with_sources(
            root.to_path_buf(),
            try_read_project_paths(&root).expect("well-formed fixture manifest"),
        );
        let out = compile_project(&options).unwrap_or_else(|f| {
            panic!(
                "adapter with a binding in a non-first include tree must compile: {}",
                render(&f.errors)
            )
        });
        let names: Vec<String> = out
            .artefacts
            .docs
            .keys()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(
            names.contains(&"payments.binding.ts".to_string()),
            "expected the binding to be copied into the output among {names:?}"
        );
    }

    /// Regression (code review of the #1114 fix itself): `tree_root_for`
    /// compared `pf.abs_path()` (always absolute, via `std::path::absolute`)
    /// against `trees`' roots as-is — for the ordinary CLI shape (a relative
    /// project root, e.g. `bynkc build .`), every tree root stays relative,
    /// so `starts_with` never matched and this silently fell back to
    /// `trees[0]` for every file, reproducing the exact bug the fix above
    /// exists to close. `scratch_project`-based tests never caught this
    /// because their project root is always an absolute temp path.
    #[test]
    fn tree_root_for_matches_against_a_relative_tree_root() {
        let trees = vec![
            (PathBuf::from("src"), PathBuf::from("src")),
            (PathBuf::from("adapters"), PathBuf::from("adapters")),
        ];
        let root = Path::new("adapters");
        // Relative, exactly as `discover_bynk_files`/`phase_parse` would pass
        // it when `Roots::Split.project_root` is itself relative.
        let rel_path = root.join("payments.bynk");
        let (parsed, _warnings) = parse_sources(
            root,
            Path::new("adapters"),
            &rel_path,
            "adapter payments {\n  binding \"./payments.binding.ts\"\n\n  exports capability { Pay }\n\n  capability Pay {\n    fn charge(amount: Int) -> Effect[String]\n  }\n\n  provides Pay = RealPay\n}\n".to_string(),
        )
        .expect("trivial adapter source must parse");
        assert_eq!(
            project_model::tree_root_for(&trees, &parsed[0]),
            Path::new("adapters"),
            "must resolve to the adapter's own (relative) tree root, not trees[0] (\"src\")"
        );
    }

    /// Regression (code review of #1114): the emitted test module's
    /// discovered-case location used to key off `trees.get(1)`'s prefix
    /// unconditionally (`tests_prefix` in `process_tests`/
    /// `emit_test_module`) — a project with `include = ["src", "examples",
    /// "tests"]` would prefix every discovered case's location with
    /// `examples/` (the second tree) even though the suite actually lives
    /// under `tests/` (the third).
    #[test]
    fn discovered_case_location_uses_the_suite_files_own_tree() {
        let root = scratch_project(
            "test_tree_prefix",
            &[
                (
                    "bynk.toml",
                    "[project]\nname = \"t\"\n\n[paths]\ninclude = [\"src\", \"examples\", \"tests\"]\n",
                ),
                (
                    "src/math.bynk",
                    "commons math {\n  fn double(n: Int) -> Int { n * 2 }\n}\n",
                ),
                (
                    "tests/math_test.bynk",
                    "suite math\n\ncase \"doubles\" {\n  expect double(2) == 4\n}\n",
                ),
            ],
        );
        let options = compile_options_split_with_sources(
            root.to_path_buf(),
            try_read_project_paths(&root).expect("well-formed fixture manifest"),
        );
        let out = compile_project(&options).unwrap_or_else(|f| {
            panic!(
                "a suite in the third include tree must compile: {}",
                render(&f.errors)
            )
        });
        let locations: Vec<String> = out
            .discovered
            .iter()
            .flat_map(|s| &s.cases)
            .filter_map(|c| c.location.as_ref())
            .map(|l| l.path.clone())
            .collect();
        assert!(
            locations
                .iter()
                .all(|p| p.starts_with("tests/") && !p.starts_with("examples/")),
            "case locations must key off the suite file's own tree (`tests/`), not the \
             second `include` tree (`examples/`): {locations:?}"
        );
    }

    /// #57 (testing track): a two-file, cross-referencing project compiled
    /// entirely through the public `compile_project` API with no on-disk
    /// tree at all — `CompileOptions::sources` replaces what
    /// `scratch_project` below has to fake with real temp-directory I/O.
    /// Before this seam, exercising `uses` across two units from inside
    /// `bynk-emit`'s own tests meant either a `scratch_project` (real files,
    /// cleaned up on drop) or `bynkc`'s on-disk fixtures one crate up.
    #[test]
    fn compile_project_with_in_memory_sources_resolves_a_cross_unit_uses() {
        let mut sources = HashMap::new();
        sources.insert(
            PathBuf::from("shapes.bynk"),
            "commons shapes\n\ntype Circle = { radius: Int }\n".to_string(),
        );
        sources.insert(
            PathBuf::from("app.bynk"),
            "commons app\n\nuses shapes\n\nfn area(c: Circle) -> Int {\n  c.radius * c.radius\n}\n"
                .to_string(),
        );
        let options = CompileOptions::single(".").sources(sources);
        let out = compile_project(&options).unwrap_or_else(|f| {
            panic!(
                "in-memory sources project should compile: {:?}",
                ProjectFailure::flatten(f)
            )
        });
        let names: Vec<String> = out
            .artefacts
            .docs
            .keys()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(
            names.contains(&"shapes.ts".to_string()),
            "expected shapes.ts among {names:?}"
        );
        assert!(
            names.contains(&"app.ts".to_string()),
            "expected app.ts among {names:?}"
        );
        let app_ts = out.artefacts.docs.get(Path::new("app.ts")).unwrap().text();
        assert!(
            app_ts.contains("radius"),
            "app.ts should reference the cross-unit Circle field:\n{app_ts}"
        );
    }

    /// A throwaway on-disk project, removed on drop — including when the test
    /// panics, which a trailing `remove_dir_all` would skip.
    struct Scratch(PathBuf);
    impl std::ops::Deref for Scratch {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Build a throwaway on-disk project. The e2e fixture suite cannot express
    /// these cases: `expected_error.txt` asserts *category strings only*, never
    /// a path, so no fixture there can pin attribution — which is precisely why
    /// the identity collision survived to slice 0.
    fn scratch_project(tag: &str, files: &[(&str, &str)]) -> Scratch {
        let dir = std::env::temp_dir().join(format!(
            "bynk_slice0_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = fs::remove_dir_all(&dir);
        for (rel, body) in files {
            let p = dir.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, body).unwrap();
        }
        Scratch(dir)
    }

    /// `AttributedError` is public API without a `Debug` impl; slice 0 is not
    /// the increment to add one, so tests render it themselves.
    fn render<'a>(errors: impl IntoIterator<Item = &'a AttributedError>) -> String {
        errors
            .into_iter()
            .map(|e| {
                format!(
                    "{} @ {}",
                    e.error.category,
                    e.source_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().replace('\\', "/"))
                        .unwrap_or_else(|| "<unattributed>".into())
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn analyse_split(root: &Path) -> Vec<AttributedError> {
        let roots = Roots::Split {
            project_root: root.to_path_buf(),
            paths: try_read_project_paths(root).expect("well-formed fixture manifest"),
        };
        let trees = roots.trees();
        let sources = read_disk_sources(&roots);
        let run = run_checks(
            &trees,
            BuildTarget::Bundle,
            Platform::default(),
            ImportExt::Js,
            Mode::Analyse,
            &sources,
            &roots.excludes(),
            None,
            false,
            true,
            &SchemaLock::Off,
            None,
            roots.project_root(),
            None,
            None,
            None,
            false,
            &Arc::new(Types::new()),
        );
        match run {
            RunChecks::Bailed { errors, .. } => errors.into_all(),
            RunChecks::Checked { errors, .. } => errors.into_all(),
        }
    }

    /// The defect's user-visible half: a diagnostic in a secondary-root file
    /// must be attributed to *that* file. Before slice 0 both roots' files were
    /// named `thing.bynk`, so a consumer keying by the attributed path (the LSP
    /// does) folded the two together and one file's diagnostics vanished.
    #[test]
    fn a_secondary_root_diagnostic_is_attributed_to_the_secondary_root_file() {
        let root = scratch_project(
            "attr",
            &[
                ("bynk.toml", "[project]\nname = \"attr\"\n"),
                ("src/thing.bynk", "context thing\n"),
                // Same basename as the src file, different root — the collision.
                //
                // A *parse* error, deliberately: `parse_tree` attributes it as
                // the file is read, which is the path slice 0 changed. (A
                // checker-level error would not do: test bodies are checked by
                // `process_tests` during emit, not in `Mode::Analyse` — which is
                // also why `bynkc check` is silent on a broken `case`.)
                ("tests/thing.bynk", "suite thing\n\ncase {{{ \n"),
            ],
        );
        let errors = analyse_split(&root);
        let paths: Vec<String> = errors
            .iter()
            .filter_map(|e| e.source_path.as_ref())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(
            !paths.is_empty(),
            "the fixture must produce at least one attributed diagnostic; got [{}]",
            render(&errors),
        );
        assert!(
            paths.iter().all(|p| p == "tests/thing.bynk"),
            "a tests-root diagnostic must be attributed to `tests/thing.bynk`, \
             never the bare `thing.bynk` it shares with `src/` — got {paths:?}",
        );
    }

    /// The layout that ruled out the cheaper repair. Prefixing only the
    /// secondary tree would have worked for a `tests/` tree of suites — but
    /// ADR 0147 made test-ness *structural*, so `include[1]` may hold an
    /// ordinary unit. `check_path_name_alignment` reads `source_path`
    /// (tree-relative), so that unit must still validate: `spec/other.bynk`
    /// declaring `context other` is aligned, and prefixing its
    /// unit-validation path would have broken it.
    #[test]
    fn a_non_test_unit_in_the_secondary_root_still_validates() {
        let root = scratch_project(
            "nontest",
            &[
                (
                    "bynk.toml",
                    "[project]\nname = \"nontest\"\n\n[paths]\ninclude = [\"src\", \"spec\"]\n",
                ),
                ("src/thing.bynk", "context thing\n"),
                ("spec/other.bynk", "context other\n"),
            ],
        );
        let errors = analyse_split(&root);
        let alignment: Vec<&AttributedError> = errors
            .iter()
            .filter(|e| e.error.category == "bynk.project.inconsistent_commons_name")
            .collect();
        assert!(
            alignment.is_empty(),
            "a non-test unit in include[1] must still pass path/name alignment — \
             its unit-validation path stays tree-relative; got [{}]",
            render(alignment.iter().copied()),
        );
    }

    /// End-to-end over the flat layout `conventional()` actually produces, so
    /// the normalisation is pinned where it is reachable and not only on the
    /// accessor. No e2e fixture has this layout.
    #[test]
    fn a_flat_project_with_a_manifest_reports_unprefixed_paths() {
        let root = scratch_project(
            "flat",
            &[
                ("bynk.toml", "[project]\nname = \"flat\"\n"),
                // Parse error: `parse_tree` attributes it as the file is read.
                ("thing.bynk", "context thing\n\nfn {{{ \n"),
            ],
        );
        let paths = try_read_project_paths(&root).expect("well-formed fixture manifest");
        assert_eq!(
            paths.include,
            vec![PathBuf::from(".")],
            "the fixture must actually exercise the flat layout"
        );
        let errors = analyse_split(&root);
        let attributed: Vec<String> = errors
            .iter()
            .filter_map(|e| e.source_path.as_ref())
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .collect();
        assert!(
            !attributed.is_empty(),
            "the fixture must produce an attributed diagnostic; got [{}]",
            render(&errors),
        );
        assert!(
            attributed.iter().all(|p| p == "thing.bynk"),
            "a flat project's diagnostics must report `thing.bynk`, never \
             `./thing.bynk` — got {attributed:?}",
        );
    }

    /// v0.29.4: assembly yields exactly one `UnitInfo` per group, every facet
    /// present, with `exports`/`aliases`/`flattened` defaulting to empty for a
    /// unit absent from those (genuinely optional) producer maps — reproducing
    /// the old `.unwrap_or(empty)` read semantics as a total field.
    #[test]
    fn assemble_unit_info_yields_one_record_per_group_with_all_facets() {
        let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        groups.insert("a.commons".to_string(), vec![0, 1]);
        groups.insert("a.context".to_string(), vec![2]);

        let mut kinds: BTreeMap<String, UnitKind> = BTreeMap::new();
        kinds.insert("a.commons".to_string(), UnitKind::Commons);
        kinds.insert("a.context".to_string(), UnitKind::Context);

        let mut unit_tables: HashMap<String, UnitTable> = HashMap::new();
        unit_tables.insert("a.commons".to_string(), UnitTable::default());
        unit_tables.insert("a.context".to_string(), UnitTable::default());

        let mut unit_uses: HashMap<String, Vec<String>> = HashMap::new();
        unit_uses.insert("a.context".to_string(), vec!["a.commons".to_string()]);

        let mut unit_consumes: HashMap<String, Vec<String>> = HashMap::new();
        unit_consumes.insert("a.context".to_string(), vec![]);

        // The genuinely-optional maps deliberately omit `a.commons` so the test
        // pins the empty-default behaviour.
        let mut unit_flattened: HashMap<String, HashMap<String, String>> = HashMap::new();
        unit_flattened.insert("a.context".to_string(), HashMap::new());
        let unit_consumes_aliases: HashMap<String, HashMap<String, String>> = HashMap::new();
        let mut exports_visibility: HashMap<String, HashMap<String, Visibility>> = HashMap::new();
        exports_visibility.insert("a.context".to_string(), HashMap::new());

        let mut unit_file_index: HashMap<String, FileDeclIndex> = HashMap::new();
        unit_file_index.insert(
            "a.commons".to_string(),
            FileDeclIndex {
                types: HashMap::new(),
                fns: HashMap::new(),
                methods: HashMap::new(),
                agents: HashMap::new(),
                capabilities: HashMap::new(),
            },
        );
        // `a.context` is absent from the file index → its `file_index` defaults.

        let info = project_model::assemble_unit_info(
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

        // One record per group, no more.
        assert_eq!(info.len(), 2);
        assert!(info.contains_key("a.commons"));
        assert!(info.contains_key("a.context"));

        // `files` mirrors the `groups` indices.
        assert_eq!(info["a.commons"].files, vec![0, 1]);
        assert_eq!(info["a.context"].files, vec![2]);

        // Non-optional facets are filled from their producer maps.
        assert_eq!(info["a.commons"].kind, UnitKind::Commons);
        assert_eq!(info["a.context"].kind, UnitKind::Context);
        assert_eq!(info["a.context"].uses, vec!["a.commons".to_string()]);

        // Optional facets default to empty for the unit with no entry.
        assert!(info["a.commons"].exports.is_empty());
        assert!(info["a.commons"].aliases.is_empty());
        assert!(info["a.commons"].flattened.is_empty());
        // And the absent `file_index` is an empty index, not a panic.
        assert!(info["a.context"].file_index.types.is_empty());
        assert!(info["a.context"].file_index.fns.is_empty());
        assert!(info["a.context"].file_index.methods.is_empty());
    }

    // -- #397: analyse_in_memory_with_types exposes expr_types (ADR 0094) -----

    #[test]
    fn analyse_in_memory_with_types_reports_expr_types_for_clean_source() {
        let src = "commons app.demo\n\nfn good() -> Int {\n  42\n}\n";
        let out = analyse_in_memory_with_types(src, BuildTarget::Bundle, Platform::default());
        assert!(
            out.errors.is_empty(),
            "clean source should have no errors: {:?}",
            out.errors
                .iter()
                .map(|e| &e.error.message)
                .collect::<Vec<_>>()
        );
        let offset = src.find("42").expect("source mentions 42");
        let ty = bynk_check::expr_types::type_at_offset(&out.expr_types, offset, &out.ty_intern);
        assert_eq!(
            ty.map(|t| t.display(&out.ty_intern)),
            Some("Int".to_string())
        );
    }

    #[test]
    fn analyse_in_memory_with_types_is_partial_under_a_sibling_error() {
        // ADR 0094: a function that types cleanly still contributes its
        // `expr_types` even though a *different* function in the same file
        // has an error — `check_record`'s pre-ADR-0094 all-or-nothing gate
        // applied per-file, not per-function, and this is the change that
        // relaxed it. Hover (#397) depends on this: it must not go blank
        // over a well-typed expression just because some other function in
        // the buffer is mid-edit and broken.
        let src = "commons app.demo\n\n\
            fn good() -> Int {\n  42\n}\n\n\
            fn bad() -> Int {\n  \"oops\"\n}\n";
        let out = analyse_in_memory_with_types(src, BuildTarget::Bundle, Platform::default());
        assert!(
            !out.errors.is_empty(),
            "the broken function must still be reported"
        );
        let offset = src.find("42").expect("source mentions 42");
        let ty = bynk_check::expr_types::type_at_offset(&out.expr_types, offset, &out.ty_intern);
        assert_eq!(
            ty.map(|t| t.display(&out.ty_intern)),
            Some("Int".to_string()),
            "the clean function's types must survive the sibling error"
        );
    }

    #[test]
    fn analyse_in_memory_still_returns_exactly_the_typed_variants_errors() {
        // Pins the refactor: `analyse_in_memory` must keep delegating to
        // `analyse_in_memory_with_types` rather than drift into a second,
        // independently-maintained `run_checks` call.
        let src = "commons app.demo\n\nfn bad() -> Int {\n  \"oops\"\n}\n";
        let errs = analyse_in_memory(src, BuildTarget::Bundle, Platform::default());
        let typed = analyse_in_memory_with_types(src, BuildTarget::Bundle, Platform::default());
        assert_eq!(errs.len(), typed.errors.len());
        assert!(!errs.is_empty());
    }

    // -- T3.3b: `expr_types` is total (R4.3, R2.5, R4.9) -----------------

    #[test]
    fn a_diagnosed_resolution_failure_records_ty_error_instead_of_nothing() {
        // An empty list literal with no expected element type to infer from
        // (`bynk.types.uninferable_element_type`, `checker.rs`'s `type_of`
        // `ExprKind::ListLit` arm) is a genuine, diagnosed `type_of` failure
        // reachable from a plain `fn` — no resolver/handler-body plumbing
        // needed to reproduce it.
        let src = "commons app.demo\n\nfn bad() -> Int {\n  []\n}\n";
        let out = analyse_in_memory_with_types(src, BuildTarget::Bundle, Platform::default());
        assert!(
            out.errors
                .iter()
                .any(|e| e.error.category == "bynk.types.uninferable_element_type"),
            "expected the uninferable-element-type diagnostic: {:?}",
            out.errors
                .iter()
                .map(|e| &e.error.message)
                .collect::<Vec<_>>()
        );
        let start = src.find("[]").expect("source mentions []");
        let recorded = out
            .expr_types
            .iter()
            .find(|(span, _)| span.start == start && span.end == start + 2)
            .map(|(_, t)| t.display(&out.ty_intern));
        assert_eq!(
            recorded.as_deref(),
            Some("<type error>"),
            "T3.3b: a diagnosed type_of failure must record Ty::Error, not leave the span \
             unrecorded"
        );
        // An editor query reads that recorded `Ty::Error` as "no type".
        assert_eq!(
            bynk_check::expr_types::type_at_offset(&out.expr_types, start, &out.ty_intern),
            None
        );
    }
}
