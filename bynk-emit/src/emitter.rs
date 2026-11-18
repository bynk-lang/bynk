//! TypeScript emission (spec §7, v0.1 §6, v0.2 §6).
//!
//! Walks the typed AST and writes a single TypeScript module.
//!
//! # Public API of the code generator
//!
//! Crate-hygiene D1 (#2982): the orchestration layer (`project`, which moves
//! to `bynk-driver`) drives this module, so the helpers it calls are a
//! deliberate public API rather than `pub(crate)` internals: the
//! `emit_*` entry points (`emit_project`, `emit_worker_entry`,
//! `emit_worker_compose`, `emit_wrangler_toml`, `emit_secrets_manifest`,
//! `emit_events_fanout_do`, `emit_tsconfig`), the specifier and name helpers
//! (`ts_specifier`, `ts_ident`, `ts_type_ref`, `cross_commons_import_specifier_for_path`),
//! the table walkers (`walk_unit_table_bodies`, `block_uses_emit`,
//! `any_service_binds_caller`, `table_has_plain_json`, …), the runtime-import
//! constants and `runtime_files` / `runtime_use`, and the
//! `wrangler`, `client`, `contracts`, `secrets`, `serialisation` and `emit`
//! submodules' items the orchestrator consumes. The shared output and context
//! model lives in [`crate::emit_model`]. Anything not reachable from a path
//! the orchestrator uses stays `pub(crate)`.
//!
//! v0.2 lowering rules:
//! - Refined-base types: branded type alias + constructor object with
//!   `of`/`unsafe` (+ any user-declared methods).
//! - Record types: TypeScript `interface` + namespace object with methods.
//! - Sum types: discriminated-union type alias + namespace object with
//!   variant constructors and methods.
//! - Field access lowers to property access.
//! - Method calls lower to `Type.method(receiver, args)` (UFCS).
//! - `match` lowers to a switch on `.tag`; in tail position it inlines,
//!   otherwise it becomes an IIFE.
//! - `is` lowers to a tag check; bindings become `const` declarations
//!   on the truthy side of `if`/`&&`.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// P7.5 (#1307): relocated from `emitter/source_map.rs` to `bynk-ts`,
// unchanged — `bynk-ts/src/source_map.rs`'s own module doc has the full
// grounding for why this API stays as-is while `bynk-ts`'s own printer
// gets a second, simpler way to use the same type.
use bynk_ts::{
    SourceMapBuilder, TsType, decimal_range_condition, decimal_signed_coef,
    pred_condition_and_message, pred_condition_and_message_for,
};

use crate::emit_model::{EmitProjectCtx, ImportExt};
use bynk_check::builtin_names::map_query;
use bynk_check::builtin_names::methods::RAW;
use bynk_check::builtin_names::types::*;
use bynk_check::checker::{CheckedProgram, ExprId, NamedKind, Ty, TyId, TypedCommons, Types};
use bynk_check::project_model::BuildTarget;
use bynk_check::symbols::UnitTable;
use bynk_model::TypeShape;
use bynk_model::{block_uses_emit, walk_block_exprs};
use bynk_model::{
    lower_capability_ops_ir, lower_protocol_ir, lower_service_handler_signature_ir,
    lower_store_field_shape_ir, lower_type_shape_ir,
};
use bynk_project::UnitKind;
use bynk_syntax::ast::{
    AgentDecl, BaseType, BinOp, Block, CommonsItem, Expr, ExprKind, FnDecl, FnName, Ident,
    InterpPart, MatchBody, MessagesDecl, ObservationMatcher, Param, Pattern, PredKind, ServiceDecl,
    ServiceProtocol, Statement, TypeBody, TypeDecl, TypeRef,
};
use ctx::UnitFacts;
use ts_types::{TyRender, asset_ts_type, ts_base};

pub mod client;
pub mod containers;
pub mod contracts;
pub(crate) mod events_fanout;
pub mod http_client;
pub mod secrets;
pub mod serialisation;
pub mod toml_doc;
pub(crate) mod workers;
pub(crate) mod workers_email;
pub(crate) mod workers_entry;
pub mod workers_group;
pub mod wrangler;

pub use containers::{container_class_names, emit_containers_ts};
pub use events_fanout::{Publisher, emit_events_fanout_do, emit_group_events_fanout_do};
pub use secrets::emit_secrets_manifest;
pub use toml_doc::print_toml_document;
pub use workers::emit_worker_compose;
pub use workers_entry::{
    emit_worker_entry, field_spec_source, form_body_specs, renamed_specs_source,
    table_has_plain_json,
};
pub use workers_group::{GroupEntry, GroupMember, emit_group_entry};
pub use wrangler::{ClassTombstones, emit_wrangler_toml};

mod cmp;
mod codec;
pub mod ctx;
mod lower;
#[cfg(feature = "text-exit-counters")]
pub use lower::text_counters;
mod lower_ctx;
mod lower_stmt;
pub(crate) mod runtime_use;
mod tco;
pub mod ts_types;
mod zero_value;
pub use lower::*;
pub use runtime_use::RuntimeUse;
pub mod emit;
pub(crate) use bynk_check::icu::{self, *};
pub(crate) use bynk_check::websocket;
pub use emit::*;

const INDENT_STEP: usize = 2;

pub mod runtime_files;
/// ADR 0615 (#2426): the runtime as one source per `@bynk/runtime` module. A
/// BUILD OUTPUT, not a hand-edited file: `bynk-emit/runtime/scripts/bundle.mjs`
/// writes `runtime/<module>.ts` and this table from the focused modules in
/// `bynk-emit/runtime/src` (CI's `runtime` job guards against drift). Keeping it
/// committed means `cargo build` stays Node-free and the emitter stays lockstep
/// with the runtime it embeds.
mod runtime_modules;
pub use runtime_files::{RUNTIME_DIR, emit_all_runtime_modules, rewrite_runtime_imports};

/// Events track, slice 2 (spine #936): the TS type of one buffered/fanned-out
/// event — a handler's `__events` local, the `__eventsDispatch` deps field
/// (service, agent, provider, and the Bundle/Workers dispatch closures that
/// implement it), and the fan-out DO's `FanoutEvent` (`events_fanout.rs`) all
/// declare this same shape independently, with no shared type today. Routing
/// every Rust-side site through one constant means the envelope field can
/// never drift by being added at 8 of 9 of them. The two TypeScript runtime
/// sources that also declare it (`runtime/src/agent.ts`'s
/// `dispatchToEventsFanout`, `runtime/src/boundary.ts`'s `deliverEvent`) are
/// hand-edited files this constant cannot reach — keep them textually
/// identical to this shape by hand; `cargo test -p bynk-tests --test it --
/// events_workers_wiring:: events_envelope_behaviour::` exercises every hop
/// and would fail on a real mismatch.
///
/// #973: `runtime/src/boundary.ts`'s `deserialiseEventEnvelope` is a related
/// but distinct hand-written piece — it validates the *inner* `envelope`
/// object's shape at the receiving `/_bynk/event/` route, not this outer
/// wire wrapper. Keep its field list in sync with the `envelope: { ... }`
/// portion of this shape by hand; nothing generates either from the other.
pub const EVENTS_WIRE_EVENT_TS_TYPE: &str = "{ type: string; payload: unknown; envelope: { eventId: string; publisherId: string; emittedAt: number; schemaVersion: number } }";

/// Emit the contents of `out/tsconfig.json`. The CLI uses `tsc -p` against
/// this when running `bynk test`; users can also drive `tsc` against it
/// directly to produce JS for deployment.
///
/// I4 (#1970): a context's generated TypeScript client (`bynk-client.ts`) is
/// excluded. It is for a foreign Worker's build, where `@bynk/wire-client`
/// resolves; the Bynk build neither imports it nor has that package.
/// Excluding a file drops the default `node_modules` exclusion, so it is
/// restated. A `[deploy.workers]` group's member writes it as
/// `<member>.bynk-client.ts` (#3562), so that name is excluded too.
///
/// #2148: `module: NodeNext` takes each file's format from the nearest
/// `package.json`, so the compiler also emits `out/package.json` declaring
/// `"type": "module"`. `tsc` then checks the output (hand-written adapter
/// bindings included) under the ES-module rules Node runs it with, and does
/// not down-level it to CommonJS, whose transform reserves the top-level names
/// `require` and (from TypeScript 7) `Object`.
pub fn emit_tsconfig() -> String {
    TSCONFIG_JSON.to_string()
}

/// The `bynk test --coverage` variant (#854): the same config with `sourceMap`
/// enabled, so `tsc` emits the `.js.map`s the coverage remap consumes (hop 1,
/// `.js` → emitted `.ts`). Kept coverage-only rather than folded into the
/// default so a normal `bynk test` / deployment `tsc` run ships no `.js.map`s.
/// The runner overwrites the default `out/tsconfig.json` with this before `tsc`.
pub fn emit_tsconfig_with_source_maps() -> String {
    TSCONFIG_JSON.replace(
        "\"outDir\": \"../out-js\",",
        "\"sourceMap\": true,\n    \"outDir\": \"../out-js\",",
    )
}

const TSCONFIG_JSON: &str = r#"{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "noImplicitAny": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "resolveJsonModule": true,
    "isolatedModules": true,
    "noEmit": false,
    "outDir": "../out-js",
    "rootDir": "."
  },
  "include": ["**/*.ts"],
  "exclude": ["node_modules", "**/bynk-client.ts", "**/*.bynk-client.ts"]
}
"#;

/// Compute the runtime import specifier for a module at `from_source`. For a
/// file at `commerce/payment.ts` the runtime sits two levels up, so this
/// returns `../runtime.js`; for a top-level file it returns `./runtime.js`.
pub fn runtime_import_for(from_source: &Path, ext: ImportExt) -> String {
    let depth = from_source
        .parent()
        .map(|p| {
            p.components()
                .filter(|c| matches!(c, std::path::Component::Normal(_)))
                .count()
        })
        .unwrap_or(0);
    let ext = ext.as_str();
    if depth == 0 {
        format!("./runtime.{ext}")
    } else {
        let prefix: String = "../".repeat(depth);
        format!("{prefix}runtime.{ext}")
    }
}

/// #1478: appends `stmts` to `out`, printing each at `depth` — the shared
/// "consume a real-node-returning callee's own output" step. #1479 adds the
/// `depth` parameter (was hardcoded to 0): `project/tests_emit.rs`'s own
/// scaffold-body callees (e.g. `emit_ns_destructure`) print nested inside a
/// generated function body, at that body's own depth — its own remaining
/// callers, `emit`/`emit_project` converted off this (#1486) to a real
/// `Vec<TsStmt>`/`TsProgram` collected and printed once via `bynk_ts::print`
/// instead.
pub fn extend_printed_at(out: &mut String, stmts: Vec<bynk_ts::TsStmt>, depth: usize) {
    for stmt in stmts {
        out.push_str(&bynk_ts::print_stmt(&stmt, depth));
    }
}

/// Emit TypeScript source for the typed commons (single-file mode).
///
/// Takes a [`CheckedProgram`] rather than a bare `TypedCommons` (T3.7, R3.10):
/// the only way to obtain one is [`bynk_check::checker::certify`], so this
/// function can no longer be called with an unchecked or partially-checked
/// program by construction.
pub(crate) fn emit(program: &CheckedProgram) -> String {
    let commons = program.program();
    // Emit the body first so the header can decide which runtime helpers to
    // import from what the body actually referenced (v0.110: the `__bynkBytes*`
    // helpers are imported only when a `Bytes` value is constructed/compared).
    // "What it referenced" comes from `dummy_ctx.runtime_use`, which the `Bytes`
    // lowerings write as they emit — not from scanning `body` for the helper's
    // name, which a user string literal or doc comment could also contain.
    // #1486: collects into a real `Vec<TsStmt>` and prints once via
    // `bynk_ts::print`, rather than `extend_printed`'s flat per-stmt loop —
    // `emit_type`/`emit_free_fn`/`emit_json_codec_helpers` (this function's
    // own callees, shared with `emit_project`) dropped their own explicit
    // inter-declaration blank `TsStmt`s as part of that conversion, relying
    // on the printer's automatic top-level spacing policy; this single-file
    // path needs the same policy to stay correctly spaced.
    let mut body_stmts: Vec<bynk_ts::TsStmt> = Vec::new();
    body_stmts.extend(write_commons_doc(commons));
    let dummy_ctx = single_file_ctx();
    // Types come first (they define interfaces and namespaces).
    for item in &commons.commons.items {
        if let CommonsItem::Type(t) = item {
            let shape = type_shape_for(t, program);
            body_stmts.extend(emit_type(t, &shape, commons, &dummy_ctx));
        }
    }
    // #2617: constants after the types, before the functions.
    body_stmts.extend(emit_consts(commons, &dummy_ctx));
    // Free functions afterward.
    for item in &commons.commons.items {
        if let CommonsItem::Fn(f) = item
            && let FnName::Free(_) = &f.name
        {
            body_stmts.extend(emit_free_fn(
                f,
                commons,
                false,
                &dummy_ctx.runtime_use,
                &dummy_ctx.cross_context,
                UnitFacts::of(&dummy_ctx),
            ));
        }
    }
    // v0.22b: module-local codec helpers for Json.encode/decode targets.
    body_stmts
        .extend(emit_json_codec_helpers(commons, &dummy_ctx, &HashSet::new(), &HashSet::new()).0);
    let body_program = bynk_ts::TsProgram { stmts: body_stmts };
    let body = bynk_ts::print(&body_program, "", "", "").text;
    let mut out = String::new();
    // v0.153 (ADR 0177): a commons that names `HttpResult` in any signature —
    // e.g. a free `fn -> HttpResult[T]` using the `?`-Option lift — imports it.
    // Structural (over the AST), not a body-string scan, so a comment or string
    // literal mentioning `HttpResult` never triggers a spurious import.
    let uses_http = file_mentions_http_result(commons);
    // #1319: same structural-scan pattern as `uses_http` — a commons naming
    // `QueueResult` in a field/type declaration imports it, independent of
    // any queue-consumer handler (which this single-file path has no
    // concept of anyway).
    let uses_queue = file_mentions_queue_result(commons);
    write_header_single(
        &mut out,
        commons,
        &dummy_ctx.runtime_use,
        uses_http,
        uses_queue,
    );
    out.push_str(&body);
    out
}

/// `t`'s already-lowered `TypeShape` (`bynk-emit::ir`, P6.6/#1188) — reuses the
/// canonical `Arc<TypeDecl>` `TypedCommons::types` already holds for `t` rather
/// than a fresh `Arc::new(t.clone())` per call (Decision B, #1188). `types`
/// holds an entry for every `CommonsItem::Type` *and* every `CommonsItem::Event`
/// (`resolver.rs`'s own resolve pass inserts both under the same table, the
/// event's own synthetic `TypeDecl` — `EventDecl::as_type_decl` — keyed
/// identically), so this one helper serves both emission loops below with no
/// special-casing for the event mirror.
///
/// Derives `commons` from `program` itself rather than taking it as a
/// separate parameter (review on #1190): the `TyId`s this returns are minted
/// from `program.program().ty_intern`, and every caller renders them straight
/// back through that same `commons.ty_intern` (`emit_record_type`/
/// `emit_sum_type`'s `ts_ty` calls) — a caller free to pass a `TypedCommons`
/// from a *different* check run would hit `Types::get`'s cross-table panic
/// instead of a diagnostic. One parameter makes that invariant
/// unrepresentable instead of merely true today.
fn type_shape_for(t: &TypeDecl, program: &CheckedProgram) -> TypeShape {
    let commons = program.program();
    let def = commons.types.get(&t.name.name).unwrap_or_else(|| {
        panic!(
            "bynk internal error (ADR 0334): type `{}` is not in TypedCommons::types, but the \
             checker already accepted this declaration",
            t.name.name
        )
    });
    lower_type_shape_ir(def, program)
}

/// A no-op project context for single-file emission. Single-file mode never
/// involves contexts or cross-unit imports, so most fields default to empty.
fn single_file_ctx() -> EmitProjectCtx {
    EmitProjectCtx {
        import_ext: crate::emit_model::ImportExt::Js,
        contracts: false,
        trace: None,
        inspect_agents: false,
        source_path: PathBuf::new(),
        identity_path: String::new(),
        commons_name: String::new(),
        file_decl_index: bynk_check::symbols::FileDeclIndex {
            types: HashMap::new(),
            fns: HashMap::new(),
            methods: HashMap::new(),
            agents: HashMap::new(),
            capabilities: HashMap::new(),
        },
        imported_from: HashMap::new(),
        imported_from_kind: HashMap::new(),
        imported_decl_paths: HashMap::new(),
        consumed_type_aliases: HashMap::new(),
        unit_kind: UnitKind::Commons,
        owning_context: None,
        exports_for_consumed: HashMap::new(),
        cross_context: bynk_check::resolver::CrossContextInfo::default(),
        target: BuildTarget::Bundle,
        local_agents: HashSet::new(),
        agent_given_deps: HashMap::new(),
        extra_import_lines: Vec::new(),
        agent_method_givens: HashMap::new(),
        agent_called_contexts: Vec::new(),
        actors: HashMap::new(),
        agent_schema_versions: HashMap::new(),
        event_schema_versions: HashMap::new(),
        consumed_adapters: HashSet::new(),
        consumed_types_with_methods: HashSet::new(),
        history_target_agents: HashSet::new(),
        asset_served: Default::default(),
        embed_text: Default::default(),
        imported_methods: HashMap::new(),
        runtime_use: Default::default(),
        worker_layout: Default::default(),
        unit_source: None,
        type_spellings: HashMap::new(),
    }
}

/// Emit TypeScript source for a single file inside a multi-file project,
/// including cross-file and cross-commons imports computed from
/// [`EmitProjectCtx`].
/// Emit one unit's TypeScript, plus its source map (slice 1, ADR 0103).
///
/// `source_text` is the originating `.bynk` file's text and `source_name` its
/// project-root-relative path; together they let the source-map builder resolve
/// each recorded span to a `(line, col)` and embed `sourcesContent`. Returns the
/// generated TS and the serialised source-map v3 JSON (`None` when nothing
/// mapped — e.g. a unit whose items all came from sibling files).
pub fn emit_project(
    program: &CheckedProgram,
    ctx: &EmitProjectCtx,
    source_text: &str,
    source_name: &str,
) -> (bynk_ts::TsProgram, Option<String>) {
    let commons = program.program();
    // #1486: no more `RefCell<SourceMapBuilder>` — every top-level item's
    // own checkpoint is `stmt.span` on the first statement its own emitter
    // returns, and `bynk_ts::print`'s own top-level loop (per #1477's own
    // extension) records it, and merges any `nested_map` (body-bearing
    // statements — service/agent handler bodies, `emit_free_fn`'s own body,
    // …) at its real print-time offset, automatically. This replaces the
    // old `smb.borrow_mut().record(out.len(), item.span)` + `print_stmt_
    // and_merge`/`extend_printed_and_merged` dance below with the printer
    // doing the same work at the one place (R7.4) it always should have.
    let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
    // Compute which names this file actually references that live elsewhere
    // (sibling file in the same commons/context, or a used commons / consumed
    // context).
    // #1778: `mut` — the implied names settle against the body built below.
    let mut references = collect_external_references(commons, ctx);
    // #1778: the body first, so the implied references can settle against
    // what it spells before the imports and rebrands are built from them.
    let mut body: Vec<bynk_ts::TsStmt> = Vec::new();
    body.extend(write_commons_doc(commons));
    for item in &commons.commons.items {
        if let CommonsItem::Type(t) = item {
            let shape = type_shape_for(t, program);
            let mut item_stmts = emit_type(t, &shape, commons, ctx);
            if let Some(first) = item_stmts.first_mut() {
                first.span = Some(t.span);
            }
            body.extend(item_stmts);
        }
    }
    // Events track, slice 0 (spine #936): an `event` is checker-visible as
    // a type (via `EventDecl::as_type_decl`, so exports/consumes/
    // construction all worked from day one), but nothing emitted its actual
    // TS declaration — this loop only ever matched `CommonsItem::Type`, so
    // a subscriber importing an event type across contexts (`from
    // Events(E)`, or `E` named in a cross-context signature) got a real
    // `tsc` "has no exported member" error. Reuses the identical synthetic
    // `TypeDecl` the checker already builds.
    for item in &commons.commons.items {
        if let CommonsItem::Event(e) = item {
            let t = e.as_type_decl();
            let shape = type_shape_for(&t, program);
            let mut item_stmts = emit_type(&t, &shape, commons, ctx);
            if let Some(first) = item_stmts.first_mut() {
                first.span = Some(t.span);
            }
            body.extend(item_stmts);
        }
    }
    // #1883, slice 1b: each asset is an exported constant holding its served
    // URL, emitted before the functions so no top-level initializer can reach
    // it before it is defined. It is typed with the structural `Asset` shape,
    // so it reads the same as any other `Asset` value. A unit that `uses` this one imports it like a
    // function (`record_name_ref`).
    for item in &commons.commons.items {
        if let CommonsItem::Asset(a) = item {
            let url = ctx
                .asset_served
                .get(&bynk_check::assets::asset_key(a))
                .cloned()
                .unwrap_or_else(|| {
                    panic!(
                        "bynk internal error: asset `{}` reached emission without a served path",
                        a.name.name
                    )
                });
            body.push(bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
                    name: ts_ident(&a.name.name),
                    ty: Some(asset_ts_type()),
                    init: bynk_ts::TsExpr::object(vec![(
                        "url".to_string(),
                        bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(url)),
                    )]),
                })),
                Some(a.span),
            ));
        }
    }
    // #2387, slice 2: each embed is an exported `string` constant holding its
    // file's text, emitted before the functions for the same reason as an
    // asset. A unit that `uses` this one imports it like a function
    // (`record_name_ref`); the bundler drops one nothing references.
    for item in &commons.commons.items {
        if let CommonsItem::Embed(e) = item {
            let key = bynk_project::assets::normalise_asset_path(&e.path);
            let text = ctx.embed_text.get(&key).cloned().unwrap_or_else(|| {
                panic!(
                    "bynk internal error: embed `{}` reached emission without its text",
                    e.name.name
                )
            });
            body.push(bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
                    name: ts_ident(&e.name.name),
                    ty: Some(TsType::named("string")),
                    init: bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(text)),
                })),
                Some(e.span),
            ));
        }
    }
    // #2617: each constant is an exported, hoisted function, after the types
    // and before the functions. A unit that `uses` this one, or a sibling
    // file, imports it like a function (`record_name_ref`).
    body.extend(emit_consts(commons, ctx));
    for item in &commons.commons.items {
        if let CommonsItem::Fn(f) = item
            && let FnName::Free(_) = &f.name
        {
            let mut item_stmts = emit_free_fn(
                f,
                commons,
                ctx.contracts,
                &ctx.runtime_use,
                &ctx.cross_context,
                UnitFacts::of(ctx),
            );
            if let Some(first) = item_stmts.first_mut() {
                first.span = Some(f.span);
            }
            body.extend(item_stmts);
        }
    }
    // message-bundles slice 2 (#874): every `messages` block in the commons
    // is emitted together, once, as a single multi-locale bundle — not
    // per-item like the other behavioural kinds below — so the generated
    // `render` can dispatch across every declared locale's own table rather
    // than reading only the `@reference` one (slice 1's scope). Recorded at
    // the `@reference` block's own span, matching how a single-item emission
    // records at that item's span elsewhere in this loop.
    let messages_blocks: Vec<&MessagesDecl> = commons
        .commons
        .items
        .iter()
        .filter_map(|item| match item {
            CommonsItem::Messages(m) => Some(m),
            _ => None,
        })
        .collect();
    if let Some(reference) = messages_blocks
        .iter()
        .find(|m| m.annotations.iter().any(|a| a.name.name == "reference"))
    {
        let mut item_stmts = emit_messages_bundle(&messages_blocks, reference, &ctx.runtime_use);
        if let Some(first) = item_stmts.first_mut() {
            first.span = Some(reference.span);
        }
        body.extend(item_stmts);
    }
    // v0.5: behavioural items follow the type/fn declarations.
    for item in &commons.commons.items {
        match item {
            CommonsItem::Capability(c) if c.derived.is_some() => {
                let mut item_stmts = emit_derived_capability(c, ctx);
                if let Some(first) = item_stmts.first_mut() {
                    first.span = Some(c.span);
                }
                body.extend(item_stmts);
            }
            CommonsItem::Capability(c) => {
                // P6.x (#1193, slice 3 of #1187): `emit_capability` reads
                // each op's resolved types off `ops`, not `c`'s own raw
                // `TypeRef`s (Decision B, #1193) — no separate helper, this
                // is `emit_capability`'s one and only call site.
                let ops = lower_capability_ops_ir(c, program);
                let mut item_stmts = emit_capability(c, &ops, commons, UnitFacts::of(ctx));
                if let Some(first) = item_stmts.first_mut() {
                    first.span = Some(c.span);
                }
                body.extend(item_stmts);
            }
            CommonsItem::Provider(p) => {
                if let Some(mut stmt) = emit_provider(p, commons, ctx) {
                    stmt.span = Some(p.span);
                    body.push(stmt);
                }
            }
            CommonsItem::Service(s) => {
                // #1187's slice 5: `emit_service` reads the protocol's own
                // resolved data (`ProtocolIr`) and each handler's resolved
                // signature (params/ret/effectful) instead of `s`'s own raw
                // `ServiceProtocol`/`TypeRef`s — shape readers only, never
                // a handler body (the full service/handler IR constructors
                // that lowered bodies were deleted by Slices D1/D2 of
                // #1542). No separate helper, this is `emit_service`'s one
                // and only call site.
                let protocol = lower_protocol_ir(&s.protocol, program);
                let signatures: Vec<_> = s
                    .handlers
                    .iter()
                    .map(|h| lower_service_handler_signature_ir(h, program))
                    .collect();
                let mut stmt = emit_service(s, &protocol, &signatures, commons, ctx);
                stmt.span = Some(s.span);
                body.push(stmt);
            }
            CommonsItem::Agent(a) => {
                let state: Vec<_> = a
                    .store_fields
                    .iter()
                    .map(|f| lower_store_field_shape_ir(f, program))
                    .collect();
                let mut item_stmts = emit_agent(a, &state, commons, ctx, source_text);
                if let Some(first) = item_stmts.first_mut() {
                    first.span = Some(a.span);
                }
                body.extend(item_stmts);
            }
            _ => {}
        }
    }
    // v0.9.2: per-test registry reset. The test runner calls this before each
    // test so a fresh test sees clean agent state (finding #10's "fresh per
    // test" half).
    let agent_names: Vec<&str> = commons
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Agent(a) => Some(a.name.name.as_str()),
            _ => None,
        })
        .collect();
    if !agent_names.is_empty() {
        // Arc C, slice 30 (#1392): a real `TsDecl::Function`.
        let reset_fn = bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::Function {
                name: "__resetAgents".to_string(),
                generics: Vec::new(),
                params: Vec::new(),
                return_type: Some(bynk_ts::TsType::named("void")),
                body: agent_names
                    .iter()
                    .map(|name| {
                        bynk_ts::TsStmt::expr_stmt(
                            bynk_ts::TsExpr::Call {
                                callee: Box::new(bynk_ts::TsExpr::Member {
                                    object: Box::new(bynk_ts::TsExpr::Ident(agent_registry_name(
                                        name,
                                    ))),
                                    property: "reset".to_string(),
                                }),
                                args: Vec::new(),
                            },
                            None,
                        )
                    })
                    .collect(),
                is_async: false,
                inline: false,
            })),
            None,
        );
        body.push(reset_fn);
    }
    // v0.6: cross-context surface assembly. Emit `__makeSurface` for any
    // context that declares services — the composition root references it
    // for every such context, not just those consumed by others. Skipped
    // in workers mode where each Worker has its own `compose(env)` root.
    if ctx.unit_kind == UnitKind::Context && matches!(ctx.target, BuildTarget::Bundle) {
        let has_services = commons
            .commons
            .items
            .iter()
            .any(|i| matches!(i, CommonsItem::Service(_)));
        if has_services {
            body.extend(emit_make_surface(commons, ctx));
        }
    }
    // v0.8: in workers mode, the context module also exports per-type
    // serialise/deserialise helpers for every type that crosses a
    // boundary. The commons modules likewise carry helpers for their
    // own commons-declared boundary types.
    // v0.96 (ADR 0124): runs on both targets — workers emits service-call +
    // agent-rehydration boundary helpers; bundle emits only the agent-rehydration
    // ones (the gate's deserialisers), since in-process calls need no wire codec.
    let (boundary_stmts, boundary_names, boundary_insts) = emit_boundary_helpers(program, ctx);
    // #1817: a boundary helper's `import` goes in the module's import block,
    // not where the helpers are. A top-level use above the helpers (an agent's
    // wire table names its codecs) otherwise ran before the import bound them
    // once compiled to CommonJS: `ReferenceError: Cannot access … before
    // initialization`.
    let (mut boundary_imports, boundary_stmts): (Vec<_>, Vec<_>) = boundary_stmts
        .into_iter()
        .partition(|s| s.is_named_import());
    body.extend(boundary_stmts);
    // v0.22b: module-local codec helpers for this file's Json.encode/decode
    // targets, deduped against the workers boundary helpers above.
    let (json_stmts, json_names) =
        emit_json_codec_helpers(commons, ctx, &boundary_names, &boundary_insts);
    body.extend(json_stmts);
    // #1829: a helper this module emits for a commons type names the types
    // that type reaches, and the source may never mention one (an agent
    // stores a used commons' `Item`, or `Json.encode`s one, and nothing reads
    // its `note`). Each is implied, so `settle_implied` imports it when a
    // helper spells it.
    references.imply_names(boundary_names.iter().chain(&json_names), commons, ctx);
    // #2027: an instantiation codec spells a commons type by its unbranded
    // alias (`__CommonsPage`), and a generic one has no codec of its own to
    // import, so nothing above implies it. Each alias the body spells
    // implies its type, which `settle_implied` then keeps.
    let unbranded = unbranded_commons_qual(commons, ctx);
    if !unbranded.is_empty() {
        let spelled = body_words(&body);
        let reached: Vec<String> = unbranded
            .into_iter()
            .filter(|(_, alias)| spelled.contains(alias))
            .map(|(name, _)| name)
            .collect();
        references.imply_names(reached.iter(), commons, ctx);
    }
    references.settle_implied(&body);
    let mut project_imports = emit_project_imports(commons, ctx, &references);
    let mut cross_context_imports = emit_cross_context_namespace_imports(commons, ctx, &body);
    // #1486: the boundary between `emit_project_imports`'s own last
    // statement and `emit_cross_context_namespace_imports`'s own first
    // (when non-empty) must reproduce the pre-#1486 code's own unconditional
    // rule exactly — no blank, *except* when `references` is non-empty,
    // which wants exactly one separating "this file's own sibling imports"
    // from "this file's cross-context namespace imports" — regardless of
    // either side's real `TsStmtKind`, which the printer's automatic
    // "no blank between adjacent imports" exemption cannot see through:
    // `emit_project_imports`'s own last statement is not always a real
    // import decl (its own `extra_import_lines` tail is pre-formatted `Raw`
    // text that *reads* as an import but isn't classified as one —
    // `328_agent_given_workers`'s own real fixture output: a `Raw`
    // adapter-binding import directly followed by a real `ImportNamespace`,
    // no blank between them despite neither side of the exemption matching).
    if !cross_context_imports.is_empty() {
        if references.is_empty() {
            // Force the "no blank" default explicitly — the automatic
            // exemption only reliably suppresses it when *both* sides are a
            // real import decl, which is not guaranteed here.
            cross_context_imports[0].no_blank_before = true;
        } else {
            // Force exactly one blank regardless of whether the automatic
            // exemption would otherwise suppress it (both sides real import
            // decls) or already supply it for free (either side `Raw`) —
            // `no_blank_before` on both the spacer and the statement right
            // after it suppresses the automatic policy on both sides,
            // leaving only the spacer's own single rendered blank line.
            let mut spacer = bynk_ts::TsStmt::blank(None);
            spacer.no_blank_before = true;
            project_imports.push(spacer);
            cross_context_imports[0].no_blank_before = true;
        }
    }
    stmts.extend(project_imports);
    stmts.extend(cross_context_imports);
    if let Some(first) = boundary_imports.first_mut() {
        first.no_blank_before = true;
    }
    stmts.extend(boundary_imports);
    // For contexts: emit per-context nominal rebrand aliases for each type
    // imported via `uses` that this file references. The structural shape is
    // inherited from the original commons type; the brand makes the
    // rebranded type nominally distinct (v0.4 §6.2).
    if ctx.unit_kind == UnitKind::Context {
        stmts.extend(emit_context_rebrands(&references, commons, ctx));
        stmts.extend(emit_consumed_type_reexports(&references, commons, ctx));
    }
    stmts.append(&mut body);
    // #1476: `ctx.runtime_use` is fully populated now — every producer above has
    // had its chance to note `bytes()`/`icu()` (`emitter::runtime_use`'s own doc:
    // this used to key on `out.contains("<helper name>")`, wrong in both
    // directions — a user string literal/doc comment false-positive, or an
    // unrelated formatting change silently dropping a *required* import). Build
    // the header — including its own runtime-import line, `bytes()`/`icu()`
    // folded in directly rather than spliced in afterward — then prepend it.
    // #1486: no more `shift_checkpoints` — every item's own checkpoint lives
    // on `stmt.span`/`stmt.nested_map`, which travel with the statement
    // itself; prepending the header is now ordinary `Vec` splicing, nothing
    // to rebase.
    let mut program_stmts = write_header(commons, ctx);
    // #1486: `write_header`'s own last statement (the runtime import, when
    // the file has any commons items) sits directly adjacent to whatever
    // `stmts` starts with — genuinely import-kind itself whenever
    // `emit_project_imports` contributed any sibling/cross-unit imports,
    // which exempts this boundary from the printer's automatic "no blank
    // between adjacent imports" policy. This boundary always wants exactly
    // one blank regardless (the pre-#1486 code's own unconditional-when-
    // items-non-empty blank, replicated exactly), so it's forced the same
    // way the project/cross-context import boundary above is, rather than
    // relying on the automatic policy.
    if !commons.commons.items.is_empty() {
        let mut spacer = bynk_ts::TsStmt::blank(None);
        spacer.no_blank_before = true;
        program_stmts.push(spacer);
        if let Some(first) = stmts.first_mut() {
            first.no_blank_before = true;
        }
    } else if stmts.is_empty() {
        // Review of #1486, finding 2: pre-conversion, `write_header`
        // unconditionally seeded a trailing blank line after the banner
        // regardless of whether the file had any commons items — the
        // automatic policy alone reproduces that gap everywhere else (it
        // fires between the banner and whatever `stmts` starts with), but
        // not in this fully-degenerate case, where there is no later
        // statement left for it to fire before. Restores the old trailing
        // blank line this module's own output would otherwise lose.
        program_stmts.push(bynk_ts::TsStmt::blank(None));
    }
    program_stmts.append(&mut stmts);
    let program = bynk_ts::TsProgram {
        stmts: program_stmts,
    };
    // The generated `file` name: the source basename with `.bynk` → `.ts`.
    let generated_file = Path::new(source_name)
        .file_stem()
        .map(|s| format!("{}.ts", s.to_string_lossy()))
        .unwrap_or_else(|| "module.ts".to_string());
    let printed = bynk_ts::print(&program, source_name, source_text, &generated_file);
    (program, printed.source_map)
}

/// Whether a service's `on call` handler hands work to the execution context
/// (`detach`, `within` or `race`, directly or through the functions it calls or
/// names, #2719), so the Bundle target's `__makeSurface` deps must carry the
/// execution context (`deps.__exec`) it forwards to the handler. The set is the
/// checker's ([`bynk_check::exec_plan::ExecPlan`]), computed from what it
/// resolved rather than by walking the syntax for method names.
pub fn call_handler_needs_exec(
    plan: &bynk_check::exec_plan::ExecPlan,
    h: &bynk_syntax::ast::Handler,
) -> bool {
    matches!(h.kind, bynk_syntax::ast::HandlerKind::Call) && plan.handler_needs(h)
}

/// P6.48 (design/tracks/the-ir.md §6b): every handler/op body in `table` —
/// every service handler, every agent handler, every provider op — visited
/// via [`walk_block_exprs`]. The shared walk `project::unit_table_uses_emit`'s
/// own `body_uses_emit` and `project::called_cross_context_services` each
/// hand-rolled a separate copy of, over exactly the same three `UnitTable`
/// collections in the same order. Lives here (already counted regardless by
/// [`ast_importers`]) so those two `project.rs` callers need only a `&Expr`
/// closure parameter, never a `&Block` or `&bynk_syntax::ast::Expr` of their
/// own.
pub(crate) fn walk_unit_table_bodies(table: &UnitTable, f: &mut impl FnMut(&Expr)) {
    for service in table.services.values() {
        for h in &service.handlers {
            walk_block_exprs(&h.body, f);
        }
    }
    for agent in table.agents.values() {
        for h in &agent.handlers {
            walk_block_exprs(&h.body, f);
        }
    }
    for provider in table.providers.values() {
        for op in &provider.ops {
            walk_block_exprs(&op.body, f);
        }
    }
}

/// #1961: the consumed contexts an agent handler body calls a service of,
/// read from the receivers' syntax (`B.svc(…)` / `Alias.svc(…)`) through
/// `resolve` — `CrossContextInfo::resolve_prefix`, or its equivalent over the
/// unit's `consumes`. A handler that constructs an agent hands its own `deps`
/// to the agent's methods (#1960), so its `deps.env` type must name these, and
/// the Worker must bind them. Syntactic because a file's `handlers.ts` is
/// emitted before the unit's other files are checked; it over-counts only a
/// local that shadows a consumed alias, which costs an unused binding.
pub fn agent_cross_context_targets(
    table: &UnitTable,
    resolve: impl Fn(&str) -> Option<String>,
) -> std::collections::BTreeSet<String> {
    let mut out = std::collections::BTreeSet::new();
    for agent in table.agents.values() {
        for h in &agent.handlers {
            walk_block_exprs(&h.body, &mut |e| {
                if let ExprKind::MethodCall { receiver, .. } = &e.kind
                    && let Some(chain) = emit::flatten_emit_ident_chain(receiver)
                    && let Some(consumed) = resolve(&chain)
                {
                    out.insert(consumed);
                }
            });
        }
    }
    out
}

/// P6.32 (design/tracks/the-ir.md §6a): the one built-in wrapper type
/// [`type_ref_mentions`] is looking for — `file_mentions_json_error`/
/// `_http_result`/`_connection` used to carry three separately hand-written
/// copies of the identical recursive walk below, differing in exactly one
/// line each (which wrapper variant stops the recursion and reports `true`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum TypeRefMarker {
    DecodeError,
    HttpResult,
    Connection,
    /// Closes the `ts_any` residual (#1319): `has_queue` (`write_header`'s
    /// own `QueueResult` import gate) only detected a real `from queue on
    /// message` consumer handler, never a bare field/type-declaration
    /// mention — the one gap among the four runtime-owned error types that
    /// `DecodeError`/`HttpResult` already closed via this same marker
    /// mechanism (`ValidationError` needs no marker at all; it's imported
    /// unconditionally).
    QueueResult,
}

/// The shared recursive walk `file_mentions_json_error`/`_http_result`/
/// `_connection` each used to hand-roll their own copy of. `marker`'s own
/// wrapper variant reports `true` immediately without recursing into its
/// payload (matching each original function's own `=> true` arm exactly —
/// `matches!(marker, ..) || type_ref_mentions(a, marker)` short-circuits on
/// the left, so the right side never evaluates when `t` itself is a match);
/// every other wrapper variant recurses into its inner type(s) exactly as
/// each original's own "recurse" bucket did.
fn type_ref_mentions(t: &TypeRef, marker: TypeRefMarker) -> bool {
    match t {
        TypeRef::DecodeError(_) => marker == TypeRefMarker::DecodeError,
        TypeRef::HttpResult(a, _) => {
            marker == TypeRefMarker::HttpResult || type_ref_mentions(a, marker)
        }
        TypeRef::Connection(a, _) => {
            marker == TypeRefMarker::Connection || type_ref_mentions(a, marker)
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            type_ref_mentions(a, marker) || type_ref_mentions(b, marker)
        }
        TypeRef::Option(a, _)
        | TypeRef::Effect(a, _)
        | TypeRef::Query(a, _)
        | TypeRef::Stream(a, _)
        | TypeRef::History(a, _)
        | TypeRef::List(a, _)
        | TypeRef::Set(a, _) => type_ref_mentions(a, marker),
        TypeRef::Fn(params, ret, _) => {
            params.iter().any(|p| type_ref_mentions(p, marker)) || type_ref_mentions(ret, marker)
        }
        // v0.157 (ADR 0183): recurse into a generic application's arguments.
        TypeRef::App { args, .. } => args.iter().any(|a| type_ref_mentions(a, marker)),
        TypeRef::QueueResult(_) => marker == TypeRefMarker::QueueResult,
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::Unit(_) => false,
    }
}

/// The shared outer walk `file_mentions_json_error`/`_http_result` used to
/// hand-roll two byte-identical copies of (P6.32) — every signature or type
/// declaration in the file, checked via [`type_ref_mentions`].
/// `file_mentions_connection` keeps its own distinct outer walk (a
/// `Connection` can additionally live in a `store` field, which neither
/// marker below can) rather than being folded in here.
fn commons_mentions_type(commons: &TypedCommons, marker: TypeRefMarker) -> bool {
    let in_type_ref = |t: &TypeRef| type_ref_mentions(t, marker);
    let sig = |params: &[Param], ret: &TypeRef| {
        params.iter().any(|p| in_type_ref(&p.type_ref)) || in_type_ref(ret)
    };
    commons.commons.items.iter().any(|item| match item {
        CommonsItem::Fn(f) => sig(&f.params, &f.return_type),
        CommonsItem::Service(s) => s.handlers.iter().any(|h| sig(&h.params, &h.return_type)),
        CommonsItem::Agent(a) => a.handlers.iter().any(|h| sig(&h.params, &h.return_type)),
        CommonsItem::Capability(c) => c.ops.iter().any(|op| sig(&op.params, &op.return_type)),
        CommonsItem::Provider(p) => p.ops.iter().any(|op| sig(&op.params, &op.return_type)),
        CommonsItem::Type(t) => match &t.body {
            TypeBody::Record(r) => r.fields.iter().any(|f| in_type_ref(&f.type_ref)),
            TypeBody::Sum(s) => s
                .variants
                .iter()
                .any(|v| v.payload.iter().any(|p| in_type_ref(&p.type_ref))),
            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => false,
        },
        // An `event` registers into the `types` table and is checked over
        // the same record-field path as `CommonsItem::Type`'s `Record` arm.
        CommonsItem::Event(e) => e.body.fields.iter().any(|f| in_type_ref(&f.type_ref)),
        // #2617: a constant's annotation is part of the module's surface.
        CommonsItem::Const(c) => in_type_ref(&c.ty),
        CommonsItem::Actor(_)
        | CommonsItem::Messages(_)
        | CommonsItem::Asset(_)
        | CommonsItem::Embed(_) => false,
    })
}

/// v0.22b: whether any signature or type declaration in this file names
/// `DecodeError` — drives the conditional `type DecodeError` runtime import.
fn file_mentions_json_error(commons: &TypedCommons) -> bool {
    commons_mentions_type(commons, TypeRefMarker::DecodeError)
}

/// Closes the `ts_any` residual (#1319): true if any signature or type
/// declaration in this file names `QueueResult` — a field/sum-payload
/// mention, not just a `from queue on message` consumer handler (which
/// `has_queue`, `write_header`'s own other `QueueResult` gate, already
/// covers). Drives the conditional `QueueResult` runtime import the same
/// way `file_mentions_json_error`/`file_mentions_http_result` already do
/// for their own types.
fn file_mentions_queue_result(commons: &TypedCommons) -> bool {
    commons_mentions_type(commons, TypeRefMarker::QueueResult)
}

/// v0.153 (ADR 0177): true if any signature or type declaration in this file
/// names `HttpResult` — a service HTTP handler, or a free `fn` / provider /
/// capability whose parameter or return type mentions it (the `?`-Option lift
/// makes a bare `fn -> HttpResult[T]` emit `HttpResult.NotFound`). Drives the
/// conditional `HttpResult` runtime import in both single-file and project
/// headers, so the import can never be missing nor spuriously added.
fn file_mentions_http_result(commons: &TypedCommons) -> bool {
    commons_mentions_type(commons, TypeRefMarker::HttpResult)
}

/// v0.102: true if a file's signatures or store fields mention `Connection[F]`,
/// so the header imports the runtime `Connection` interface. Covers the held
/// sites: capability-operation returns, service/agent handler parameters, and
/// `store` field value types (`Map[K, Connection]` / `Cell[Option[Connection]]`).
fn file_mentions_connection(commons: &TypedCommons) -> bool {
    let in_type_ref = |t: &TypeRef| type_ref_mentions(t, TypeRefMarker::Connection);
    let sig = |params: &[Param], ret: &TypeRef| {
        params.iter().any(|p| in_type_ref(&p.type_ref)) || in_type_ref(ret)
    };
    commons.commons.items.iter().any(|item| match item {
        CommonsItem::Fn(f) => sig(&f.params, &f.return_type),
        CommonsItem::Service(s) => s.handlers.iter().any(|h| sig(&h.params, &h.return_type)),
        CommonsItem::Agent(a) => {
            a.handlers.iter().any(|h| sig(&h.params, &h.return_type))
                || a.store_fields
                    .iter()
                    .any(|f| f.kind.args.iter().any(in_type_ref))
        }
        CommonsItem::Capability(c) => c.ops.iter().any(|op| sig(&op.params, &op.return_type)),
        CommonsItem::Provider(p) => p.ops.iter().any(|op| sig(&op.params, &op.return_type)),
        // A `Connection` is a held resource storable only in a `store` field
        // (handled above) — never in a plain record field, so `Type`/`Event`
        // need no case here; `Actor`/`Messages` carry no `TypeRef` at all.
        CommonsItem::Type(_)
        | CommonsItem::Actor(_)
        | CommonsItem::Messages(_)
        | CommonsItem::Event(_)
        | CommonsItem::Asset(_)
        | CommonsItem::Embed(_)
        // #2617: a constant's static initialiser can never be a held value.
        | CommonsItem::Const(_) => false,
    })
}

/// v0.22b: a checker `Ty` rendered back to a `TypeRef` for the codec
/// machinery (which is `TypeRef`-driven). `None` for types the codec
/// rejects anyway (functions, effects, type variables). `pub(crate)` since
/// P6.28 (design/tracks/the-ir.md §6a): `project/tests_emit.rs`'s own drain
/// of `RuntimeUse::json_codec_roots` (a sibling module tree, not a
/// descendant of `emitter`) needs this same conversion, once, right before
/// `collect_codec_closure` — the one remaining consumer still genuinely
/// `TypeRef`-driven.
pub fn ty_to_type_ref(t: TyId, tys: &Arc<Types>) -> Option<TypeRef> {
    let sp = bynk_syntax::span::Span::new(0, 0);
    Some(match &*tys.get(t) {
        Ty::Base(b) => TypeRef::Base(*b, sp),
        // v0.174 (#592): a generic-record instantiation (`Paginated[User]`,
        // `args` non-empty) round-trips as a `TypeRef::App` so the codec closure
        // reaches its monomorphised helper; a non-generic named type stays a
        // bare `Named`.
        Ty::Named { name, args, .. } if !args.is_empty() => TypeRef::App {
            name: Ident {
                name: name.clone(),
                span: sp,
            },
            args: args
                .iter()
                .map(|a| ty_to_type_ref(*a, tys))
                .collect::<Option<Vec<_>>>()?,
            span: sp,
        },
        Ty::Named { name, .. } => TypeRef::Named(Ident {
            name: name.clone(),
            span: sp,
        }),
        Ty::Result(a, b) => TypeRef::Result(
            Box::new(ty_to_type_ref(*a, tys)?),
            Box::new(ty_to_type_ref(*b, tys)?),
            sp,
        ),
        Ty::Option(a) => TypeRef::Option(Box::new(ty_to_type_ref(*a, tys)?), sp),
        Ty::List(a) => TypeRef::List(Box::new(ty_to_type_ref(*a, tys)?), sp),
        Ty::Set(a) => TypeRef::Set(Box::new(ty_to_type_ref(*a, tys)?), sp),
        Ty::Map(k, v) => TypeRef::Map(
            Box::new(ty_to_type_ref(*k, tys)?),
            Box::new(ty_to_type_ref(*v, tys)?),
            sp,
        ),
        Ty::Unit => TypeRef::Unit(sp),
        Ty::ValidationError => TypeRef::ValidationError(sp),
        Ty::DecodeError => TypeRef::DecodeError(sp),
        // R4.3: `Ty::Error` has no codec — same as the other non-boundary
        // types below, but for the additional reason that a checked program
        // should never contain one at a codec-generation site.
        Ty::Error
        | Ty::Never
        | Ty::Effect(_)
        | Ty::Query(_)
        | Ty::Stream(_)
        | Ty::TestConnection(_)
        | Ty::Connection(_)
        | Ty::HttpResult(_)
        | Ty::HttpOutcome(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::RequestQuery
        | Ty::RequestCookies
        | Ty::TokenClaims
        | Ty::Fn { .. }
        | Ty::Var(_)
        | Ty::Actor(_)
        | Ty::ActorSum(_) => {
            return None;
        }
    })
}

/// v0.22b: collect the `Json.encode`/`Json.decode[T]` target type-refs in
/// this file's bodies — the roots of the module-local codec-helper closure.
///
/// R8.14 (Arc D, P7.d3), revisiting P6.56's own declined attempt: P6.56 found
/// no `Callee`-classification for `Json.encode`/`decode` to read at the time
/// ("`commons.callees` carries no entry for this call site at all"). That is
/// no longer true — `checker::calls`'s own JSON-static dispatch now inserts
/// `Callee::Intrinsic { ns: JSON, op }` for exactly this call shape (the same
/// `ctx.lookup(JSON).is_none() && !ctx.input.types.contains_key(JSON)` guard
/// against a local shadow that this function's own bare `id.name == JSON`
/// match had no way to apply). Reading it here closes two things at once:
/// R8.14's own "AST-shaped, not checker-resolved" framing, and a real
/// (if narrow) correctness gap the old syntactic match carried — a local
/// variable or type named `Json` shadowing the builtin would have been
/// misread as a real `Json.encode`/`decode` call; `Callee::Intrinsic`'s own
/// presence is exactly the checker's already-verified "no, this really is
/// the builtin" answer.
/// Whether the module needs the JSON-codec imports: it has a codec root, or it
/// declares a `Codable` witness, whose parameter type names `__JsonValue` and
/// `__BoundaryError` whether or not the body encodes anything.
fn uses_json_codec(commons: &TypedCommons) -> bool {
    !collect_json_codec_roots(commons).is_empty()
        || commons.commons.items.iter().any(|item| match item {
            CommonsItem::Fn(f) => bynk_check::checker::codable::witness_list(f, &commons.types)
                .iter()
                .any(|w| w.kind == bynk_check::checker::codable::WitnessKind::Codec),
            _ => false,
        })
}

fn collect_json_codec_roots(commons: &TypedCommons) -> Vec<TypeRef> {
    let tys = commons.tys();
    let mut roots: Vec<TypeRef> = Vec::new();
    {
        let push_ground = |t: TyId, roots: &mut Vec<TypeRef>| {
            let mut parts = Vec::new();
            codec::ground_parts(t, commons, &mut parts);
            roots.extend(parts.into_iter().filter_map(|p| ty_to_type_ref(p, tys)));
            // #2792: a user generic type over a type variable needs its factory.
            let mut factories = Vec::new();
            codec::generic_factories(t, commons, &mut factories);
            for f in factories {
                let root = serialisation::codec_factory_root(&f, &commons.types);
                // `CodecGen::for_ty_node` names `__codecOf_<f>` for exactly these, so a
                // declaration that is not a generic record or sum here would leave
                // a reference to a function nothing emits.
                debug_assert!(
                    root.is_some(),
                    "bynk internal error: no codec factory for generic type `{f}`"
                );
                roots.extend(root);
            }
        };
        let mut visit = |e: &Expr| {
            // Constrained generics slice 3: every concrete type that fills a
            // `Codable` witness or a `Codable` capability argument is a root,
            // as are the concrete parts of one that mentions a type parameter.
            match commons.callees.get(&e.id) {
                Some(
                    bynk_check::checker::Callee::Fn(decl, ws)
                    | bynk_check::checker::Callee::FnValue(decl, ws)
                    | bynk_check::checker::Callee::Method(decl, ws)
                    | bynk_check::checker::Callee::Static(decl, ws),
                ) => {
                    for (w, ty) in bynk_check::checker::codable::witness_list(decl, &commons.types)
                        .iter()
                        .zip(ws)
                    {
                        if w.kind == bynk_check::checker::codable::WitnessKind::Codec {
                            push_ground(*ty, &mut roots);
                        }
                    }
                }
                Some(
                    bynk_check::checker::Callee::Capability { wire: Some(w), .. }
                    | bynk_check::checker::Callee::CrossCap { wire: Some(w), .. },
                ) => {
                    for ty in w.encode.iter().flatten() {
                        push_ground(*ty, &mut roots);
                    }
                    // #3458: a typed read decodes its reply as the payload.
                    if let Some(reply) = &w.reply {
                        push_ground(reply.payload, &mut roots);
                    }
                }
                // #2865: a typed `D1` read decodes its row (or `List` of rows).
                Some(bynk_check::checker::Callee::D1Select { row, first, .. }) => {
                    push_ground(
                        if *first {
                            *row
                        } else {
                            commons.tys().intern(Ty::List(*row))
                        },
                        &mut roots,
                    );
                }
                _ => {}
            }
            let ExprKind::MethodCall { args, .. } = &e.kind else {
                return;
            };
            let Some(bynk_check::checker::Callee::Intrinsic { ns, op }) =
                commons.callees.get(&e.id)
            else {
                return;
            };
            // #3214: `Yaml.decode[T]` decodes through the same codec helpers.
            if *ns != JSON && *ns != YAML && *ns != FORM {
                return;
            }
            match op.as_str() {
                "decode" | "decodeSql" => {
                    if let Some(Ty::Result(t, _)) = commons.expr_ty(e.id).as_deref() {
                        push_ground(*t, &mut roots);
                    }
                }
                "encode" | "encodePlain" => {
                    if let Some(a) = args.first()
                        && let Some(t) = commons.expr_types.get(&a.id).map(|te| te.ty)
                    {
                        push_ground(t, &mut roots);
                    }
                }
                _ => {}
            }
        };
        for item in &commons.commons.items {
            match item {
                CommonsItem::Fn(f) => walk_block_exprs(&f.body, &mut visit),
                CommonsItem::Service(s) => {
                    for h in &s.handlers {
                        walk_block_exprs(&h.body, &mut visit);
                    }
                }
                CommonsItem::Agent(a) => {
                    for h in &a.handlers {
                        walk_block_exprs(&h.body, &mut visit);
                    }
                }
                CommonsItem::Provider(p) => {
                    for op in &p.ops {
                        walk_block_exprs(&op.body, &mut visit);
                    }
                }
                _ => {}
            }
        }
    }
    roots
}

/// v0.22b: module-local serialise/deserialise helpers for the types this
/// file's `Json.encode`/`Json.decode[T]` calls reference (ADR 0045). The
/// closure machinery is shared with the workers boundary path; `skip_names`
/// / `skip_insts` dedupe against helpers that path already emitted into
/// this module.
/// #1478: returns real [`bynk_ts::TsStmt`]s (was `out: &mut String`) — the
/// same `decls_as_stmts[_block]` conversion `emit_boundary_helpers`/
/// `emit_consumed_context_helpers` just used. #1829: also returns the type
/// names it emitted helpers for, which the module must bring into scope.
fn emit_json_codec_helpers(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    skip_names: &HashSet<String>,
    skip_insts: &HashSet<String>,
) -> (Vec<bynk_ts::TsStmt>, Vec<String>) {
    use serialisation::{collect_codec_closure, emit_generic_helpers, emit_helpers_for_owner};
    let mut roots = collect_json_codec_roots(commons);
    if roots.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // #2792: a codec factory root is not a codec target: emit its factory, and
    // root the module's ordinary closure at the parts of its declaration that
    // name no type parameter.
    let mut factories: Vec<TypeRef> = Vec::new();
    let mut factory_seen: HashSet<String> = HashSet::new();
    let mut plain: Vec<TypeRef> = Vec::new();
    for r in roots.drain(..) {
        match serialisation::codec_factory_name(&r) {
            Some(n) => {
                if factory_seen.insert(n.to_string()) {
                    plain.extend(serialisation::codec_factory_ground_roots(
                        &r,
                        &commons.types,
                    ));
                    factories.push(r);
                }
            }
            None => plain.push(r),
        }
    }
    let roots = plain;
    let (names, insts) = collect_codec_closure(&roots, &commons.types);
    let names: Vec<String> = names
        .into_iter()
        .filter(|n| !skip_names.contains(n))
        .collect();
    let mut stmts = serialisation::decls_as_stmts_block(emit_helpers_for_owner(
        &names,
        &commons.types,
        &ctx.commons_name,
        &ctx.runtime_use,
    ));
    let insts: Vec<serialisation::GenericInst> = insts
        .into_iter()
        .filter(|i| !skip_insts.contains(&i.ts_name()))
        .collect();
    if !insts.is_empty() {
        stmts.extend(serialisation::decls_as_stmts(emit_generic_helpers(
            &insts,
            &commons.types,
            &ctx.runtime_use,
        )));
    }
    if !factories.is_empty() {
        stmts.extend(serialisation::decls_as_stmts(
            factories
                .iter()
                .map(|r| serialisation::emit_codec_factory(r, &commons.types, &ctx.runtime_use))
                .collect(),
        ));
    }
    (stmts, names)
}

/// Emit boundary serialise/deserialise helpers (v0.8 §3.4 / §5.2) for
/// every named type declared in this file that flows through a
/// cross-context call, plus the specialised generic helpers for any
/// Result/Option instantiation used at the boundary. Returns the emitted
/// (or locally-bound) helper type names and generic-instantiation names so
/// the v0.22b codec emission can dedupe against them.
/// #1478: returns real [`bynk_ts::TsStmt`]s (was `out: &mut String`) as a
/// new first element of the tuple — every `out`-write here is already
/// exclusively through `serialisation::decls_as_stmts[_block]` or a real
/// `bynk_ts::print_stmt` call, so each becomes a `stmts.extend`/`stmts.push`
/// into the same local `stmts`, threaded through `emit_consumed_context_
/// helpers`'s own identical conversion.
/// #1678: the type positions an agent call puts on the wire on the `workers`
/// target — every handler's parameters and its return type — in agent then
/// handler order. Positions with no codec (`agent_wire_passes_through`) carry
/// no named types to collect, so they are left out.
pub(crate) fn agent_call_boundary_roots(
    agents: &HashMap<String, AgentDecl>,
    // #3581: a `bynk dev` build records each instance's key, so every agent's key
    // type is a boundary type, not only one an agent reads.
    inspect: bool,
) -> Vec<TypeRef> {
    let mut names: Vec<&String> = agents.keys().collect();
    names.sort();
    let mut roots = Vec::new();
    for name in names {
        for h in &agents[name].handlers {
            for p in &h.params {
                roots.push(p.type_ref.clone());
            }
            roots.push(h.return_type.clone());
        }
        // #1818: an agent that reads its own key is sent it with each call,
        // through the key type's codec.
        if inspect || crate::emitter::emit::agent_reads_key(&agents[name]) {
            roots.push(agents[name].key_type.clone());
        }
    }
    roots
        .into_iter()
        .filter(|t| !serialisation::agent_wire_passes_through(t))
        .collect()
}

fn emit_boundary_helpers(
    program: &CheckedProgram,
    ctx: &EmitProjectCtx,
) -> (Vec<bynk_ts::TsStmt>, HashSet<String>, HashSet<String>) {
    use serialisation::{
        collect_boundary_types, collect_generic_instantiations, emit_generic_helpers,
        emit_helpers_for_owner,
    };
    // Review of #1211: `commons` is always `program.program()` — derived
    // rather than taken as a second parameter, so the two can never alias
    // to different tables. `lower_protocol_ir`/`ty_to_type_ref` below mint
    // and resolve `TyId`s against the same `program`, and `Types::get`'s
    // own doc comment (`bynk-check/src/checker.rs`) names exactly what a
    // mismatched pair would silently do in a release build: resolve an
    // in-range foreign id to an unrelated `Ty`, not panic.
    let commons = program.program();

    // For contexts: walk the local services to discover boundary types.
    // For commons: walk every consumer's services that reference us
    // (approximated as: emit for every type declared in this file).
    //
    // Service handler types cross the *cross-Worker call* boundary, which only
    // exists on the `workers` target; on `bundle` calls are in-process, so their
    // serialise/deserialise helpers are not emitted. The agent **rehydration**
    // boundary (ADR 0124), in contrast, exists on both targets, so agent
    // store-field types are always collected (below).
    let workers = matches!(ctx.target, BuildTarget::Workers);
    let services: HashMap<String, ServiceDecl> = if workers {
        commons
            .commons
            .items
            .iter()
            .filter_map(|i| match i {
                CommonsItem::Service(s) => Some((s.name.name.clone(), s.clone())),
                _ => None,
            })
            .collect()
    } else {
        HashMap::new()
    };

    // v0.96 (ADR 0124): an agent's `store`-field types are rehydration-boundary
    // types — their deserialisers drive the load-time validation gate.
    let agents: HashMap<String, AgentDecl> = commons
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Agent(a) => Some((a.name.name.clone(), a.clone())),
            _ => None,
        })
        .collect();

    let locally_declared: HashSet<String> = ctx.file_decl_index.types.keys().cloned().collect();
    if ctx.unit_kind == UnitKind::Context {
        let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
        // #1678: on `workers` an agent call crosses a Durable Object `fetch`, so
        // every agent handler's parameter and return types are boundary types
        // too. On `bundle` the call is in-process and needs no codec.
        let agent_call_roots: Vec<TypeRef> = if workers {
            agent_call_boundary_roots(&agents, ctx.inspect_agents)
        } else {
            Vec::new()
        };
        let boundary_types_all =
            collect_boundary_types(&commons.types, &services, &agents, &agent_call_roots);
        // Locally-declared boundary types get full helpers in this module. On
        // `bundle` (v0.96, ADR 0124) the commons modules emit no boundary helpers,
        // so a cross-commons *agent-state* type's deserialiser — needed by the
        // rehydration gate — is emitted here in the context instead of re-exported.
        let local_boundary: Vec<String> = boundary_types_all
            .iter()
            .filter(|n| !workers || locally_declared.contains(*n))
            .cloned()
            .collect();
        stmts.extend(serialisation::decls_as_stmts_block(emit_helpers_for_owner(
            &local_boundary,
            &commons.types,
            ctx.commons_name.as_str(),
            &ctx.runtime_use,
        )));

        // Re-export helpers for commons-owned boundary types so consumers
        // can address them through this context's handlers.ts namespace
        // (matching the namespace import they already use for cross-
        // context types). Grouped by source commons. Workers only — on `bundle`
        // the commons emit no helpers, so cross-commons types are emitted
        // locally above (v0.96) rather than imported.
        let mut by_commons: HashMap<String, Vec<String>> = HashMap::new();
        for n in &boundary_types_all {
            if !workers || locally_declared.contains(n) {
                continue;
            }
            if let Some(commons_name) = codec_owner(ctx, n) {
                by_commons
                    .entry(commons_name.clone())
                    .or_default()
                    .push(n.clone());
            }
        }
        // #1836 review: grouped by the *declaring file*, not the commons. A
        // multi-file commons emits each type's codecs in the file declaring it
        // (#479), so one import per commons named members a sibling file
        // exports (TS2305).
        let mut by_path: std::collections::BTreeMap<PathBuf, std::collections::BTreeSet<String>> =
            std::collections::BTreeMap::new();
        for (commons_name, names) in &by_commons {
            for n in names {
                by_path
                    .entry(commons_decl_path(ctx, commons_name, n))
                    .or_default()
                    .insert(n.clone());
            }
        }
        for (group_index, (target_path, names)) in by_path.into_iter().enumerate() {
            let import_spec = cross_commons_import_specifier_for_path(
                &ctx.source_path,
                &target_path,
                ctx.import_ext,
            );
            let parts: Vec<String> = names
                .iter()
                .flat_map(|n| [format!("__serialise_{n}"), format!("__deserialise_{n}")])
                .collect();
            let import_parts: Vec<String> = names
                .iter()
                .flat_map(|n| {
                    ["__serialise_", "__deserialise_"]
                        .map(|prefix| codec_import_spec(ctx, prefix, n))
                })
                .collect();
            // v0.9.1: emit both a regular import (so the names are bound
            // locally for use inside this file's serialisation helpers) and a
            // re-export (so downstream consumers can still reach them
            // through this module). A bare `export { ... } from "..."`
            // re-export does not create a local binding, which `tsc --strict`
            // catches when the body calls one of the helpers directly.
            //
            // Arc C, slice 30 (#1392): the import is a real `TsDecl::Import`.
            // The re-export is a BARE `export { ... };` — already-bound
            // local names, no `from` clause — which `TsDecl::ReExport`
            // cannot represent (it always carries one). #1820: it was opaque
            // `TsStmt::raw`, but a split context's `handlers.ts` must see
            // what each file exports (two files can both re-export a
            // commons type's codecs), so it is `TsDecl::ExportNames`.
            //
            // #1817: the module assembler hoists the import into the import
            // block; the re-export stays here with the helpers. Every import
            // after the first is glued to the one before it there.
            let mut import_stmt = bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::Import {
                    type_only: false,
                    names: import_parts,
                    from: import_spec,
                },
                None,
            );
            import_stmt.no_blank_before = group_index > 0;
            stmts.push(import_stmt);
            stmts.push(bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::ExportNames {
                    names: parts.clone(),
                },
                None,
            ));
        }
        // #1486: no explicit blank stmt here — the printer's automatic
        // top-level spacing policy supplies the single blank before the
        // Result_/Option_ helpers below (neither import- nor comment-kind,
        // so never exempted from it).

        // Specialised Result_/Option_ helpers for the instantiations used —
        // in handler signatures or in boundary-type fields (v0.18).
        //
        // #977: the field walk follows `local_boundary`, not `boundary_types_all`
        // — the same narrowing `emit_helpers_for_owner` applies just above, and
        // for the same reason. A boundary type this context does not *declare* is
        // either commons-owned (its codec, and its own instantiations, come from
        // the commons module) or consumed (its codec is regenerated below by
        // `emit_consumed_context_helpers`, whose `Qual` map reaches the owner's
        // `import type * as <ns>` alias). Walking a foreign type's fields here
        // emitted its instantiations *unqualified* — `Option<Region>` for a
        // consumed `Region` — and then seeded `emitted_insts` so the qualified
        // pass below skipped it, leaving `tsc --strict` with `TS2304: Cannot find
        // name 'Region'`. Handler signatures still walk in full: a *local*
        // handler naming `Option[ConsumedRegion]` directly is this module's own
        // boundary either way.
        let insts = collect_generic_instantiations(
            &services,
            &agents,
            &local_boundary,
            &commons.types,
            &agent_call_roots,
        );
        stmts.extend(serialisation::decls_as_stmts(
            serialisation::emit_generic_helpers_qualified(
                &insts,
                &commons.types,
                &unbranded_commons_qual(commons, ctx),
                &ctx.runtime_use,
            ),
        ));

        // #661 (ADR 0199 Decision G discharged): the caller's own view of each
        // consumed context's boundary codecs, so a cross-context call reaches
        // `deserialise_Result_AuthId_PaymentError` **locally** instead of through
        // the callee's module. Workers only — on `bundle` the call is in-process
        // and needs no wire codec. Everything the caller already emits (its own
        // boundary types, the commons re-exports above, its own generic
        // instantiations) is skipped, so only the callee-*owned* types the caller
        // lacks a local view of are generated.
        let (consumed_names, consumed_insts) = if workers {
            let mut emitted_names: HashSet<String> = local_boundary.iter().cloned().collect();
            for names in by_commons.values() {
                emitted_names.extend(names.iter().cloned());
            }
            let mut emitted_insts: HashSet<String> = insts.iter().map(|i| i.ts_name()).collect();
            // #2036: a consumed context's type on this context's *own* boundary
            // (`on call(b: Batch)`) needs its codecs here even when no call to
            // that context reaches it: the entry decodes the argument through
            // `handlers.__deserialise_Batch`. Neither path above emits it (it is
            // not declared here, and no commons owns it), so it roots the
            // consumed context's codec closure alongside the called services.
            let mut own_roots: HashMap<String, Vec<TypeRef>> = HashMap::new();
            for n in &boundary_types_all {
                if locally_declared.contains(n)
                    || ctx.imported_from_kind.get(n) != Some(&UnitKind::Context)
                {
                    continue;
                }
                let Some(owner) = ctx.imported_from.get(n) else {
                    continue;
                };
                if ctx.consumed_adapters.contains(owner) {
                    continue;
                }
                own_roots
                    .entry(owner.clone())
                    .or_default()
                    .push(TypeRef::Named(Ident {
                        name: n.clone(),
                        span: Default::default(),
                    }));
            }
            let (consumed_stmts, consumed_names, consumed_insts, foreign_names) =
                emit_consumed_context_helpers(
                    program,
                    ctx,
                    &own_roots,
                    &mut emitted_names,
                    &mut emitted_insts,
                );
            stmts.extend(consumed_stmts);
            // #1823: codecs of commons types a call to another context needs,
            // beyond those this context's own boundary already imports.
            let (imports, imported) =
                commons_codec_imports(foreign_names, &emitted_names, false, ctx);
            stmts.extend(imports);
            // #1836 review: registered, so the Json-codec pass does not emit a
            // local copy beside the import (TS2440).
            let mut consumed_names = consumed_names;
            consumed_names.extend(imported);
            (consumed_names, consumed_insts)
        } else {
            (Vec::new(), Vec::new())
        };

        let mut ret_names: HashSet<String> = boundary_types_all.into_iter().collect();
        ret_names.extend(consumed_names);
        let mut ret_insts: HashSet<String> = insts.iter().map(|i| i.ts_name()).collect();
        ret_insts.extend(consumed_insts);
        (stmts, ret_names, ret_insts)
    } else if !workers {
        // Commons/adapters have no agents (no rehydration boundary), and on
        // `bundle` there is no cross-Worker call boundary either — so emit no
        // boundary helpers, matching pre-v0.96 bundle output (the rehydration
        // pass that now always runs is for context-declared agents only).
        (Vec::new(), HashSet::new(), HashSet::new())
    } else {
        // Commons/adapters (workers): emit helpers for every type declared in
        // this file, plus (v0.18) the generic instantiations their fields use —
        // a record like the bynk surface's `Request` carries
        // `Option[String]` fields whose serialisers delegate to the
        // specialised helpers.
        //
        // v0.132 (#479): scope to types declared in *this* file, not the whole
        // unit. `file_decl_index` is unit-wide (name -> declaring-file path), so a
        // multi-file commons must filter by the current file — otherwise a
        // non-declaring sibling (e.g. the file holding `fn T.make`, not `type T`)
        // emits an orphan `serialise_T`/`deserialise_T` with `T` out of scope, and
        // the codec is duplicated across files. Unlike a workers *context* (which
        // collapses to one `handlers.ts` under a synthetic source_path, so its
        // unit-wide `locally_declared` above is correct), a commons emits per file.
        let mut locally: Vec<String> = ctx
            .file_decl_index
            .types
            .iter()
            .filter(|(_, path)| path.as_path() == ctx.source_path.as_path())
            .map(|(name, _)| name.clone())
            .collect();
        locally.sort();
        let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
        // #1815: a local type's codec calls the codec of each type its fields
        // reach. One owned by another commons (`Repo` in `t.core`, reached from
        // `t.model`'s `Run`) is imported from that commons, which exports it.
        let roots: Vec<TypeRef> = locally
            .iter()
            .map(|n| {
                TypeRef::Named(Ident {
                    name: n.clone(),
                    span: Default::default(),
                })
            })
            .collect();
        let reached =
            collect_boundary_types(&commons.types, &HashMap::new(), &HashMap::new(), &roots);
        let declared: HashSet<String> = locally.iter().cloned().collect();
        let (imports, imported) = commons_codec_imports(reached, &declared, true, ctx);
        stmts.extend(imports);
        stmts.extend(serialisation::decls_as_stmts_block(emit_helpers_for_owner(
            &locally,
            &commons.types,
            ctx.commons_name.as_str(),
            &ctx.runtime_use,
        )));
        let insts = collect_generic_instantiations(
            &HashMap::new(),
            &HashMap::new(),
            &locally,
            &commons.types,
            &[],
        );
        stmts.extend(serialisation::decls_as_stmts(emit_generic_helpers(
            &insts,
            &commons.types,
            &ctx.runtime_use,
        )));
        // #1836 review: the imported names count as emitted, so the Json-codec
        // pass does not emit a local copy beside the import (TS2440).
        let mut names: HashSet<String> = locally.into_iter().collect();
        names.extend(imported);
        (stmts, names, insts.iter().map(|i| i.ts_name()).collect())
    }
}

/// Whether `name` is a type a consumed *context* declares. An adapter's types
/// are recorded as a consumed context's too, but an adapter is linked into each
/// consuming Worker, so its module is in reach (P5, #2323: an `R2Object` the
/// source never spells is imported like any other implied name).
fn is_consumed_context_type(ctx: &EmitProjectCtx, name: &str) -> bool {
    ctx.imported_from_kind.get(name) == Some(&UnitKind::Context)
        && !ctx
            .imported_from
            .get(name)
            .is_some_and(|owner| ctx.consumed_adapters.contains(owner))
}

/// The unit whose module exports `name`'s codecs for this one to import: a
/// commons it `uses`, or (#1845) an adapter it consumes. An adapter is
/// recorded in `imported_from_kind` as a consumed *context*, but unlike one it
/// is linked into each consuming Worker, so its module and codecs are in
/// reach. A consumed context's types get their codecs from
/// `emit_consumed_context_helpers` instead.
fn codec_owner<'a>(ctx: &'a EmitProjectCtx, name: &str) -> Option<&'a String> {
    let owner = ctx.imported_from.get(name)?;
    match ctx.imported_from_kind.get(name) {
        Some(UnitKind::Commons) => Some(owner),
        // `Adapter` too, should the checker ever record an adapter's types as
        // its own kind.
        Some(UnitKind::Context | UnitKind::Adapter) if ctx.consumed_adapters.contains(owner) => {
            Some(owner)
        }
        _ => None,
    }
}

/// The file declaring `name` in the commons `commons_name`, as a module
/// imports it: the declaring file of a multi-file commons, or the commons'
/// single-file path.
fn commons_decl_path(ctx: &EmitProjectCtx, commons_name: &str, name: &str) -> PathBuf {
    ctx.imported_decl_paths
        .get(commons_name)
        .and_then(|m| m.get(name).cloned())
        .unwrap_or_else(|| EmitProjectCtx::commons_path(commons_name))
}

/// #1815/#1823: `import { __serialise_X, __deserialise_X } from "<file>"` for
/// each type in `names` whose codecs this module calls but neither declares nor
/// already imports (`already`): one owned by a commons this module uses, and,
/// when `siblings` (a file of a multi-file commons), one declared in another
/// file of the same commons. Grouped by declaring file (#1836 review), in
/// name order. Returns the imports and the names they bring in. The module
/// assembler hoists the imports into the import block (#1817).
fn commons_codec_imports(
    names: impl IntoIterator<Item = String>,
    already: &HashSet<String>,
    siblings: bool,
    ctx: &EmitProjectCtx,
) -> (Vec<bynk_ts::TsStmt>, HashSet<String>) {
    let mut by_path: std::collections::BTreeMap<PathBuf, std::collections::BTreeSet<String>> =
        std::collections::BTreeMap::new();
    for n in names {
        if already.contains(&n) {
            continue;
        }
        let path = if let Some(commons_name) = codec_owner(ctx, &n) {
            commons_decl_path(ctx, commons_name, &n)
        } else if siblings
            && let Some(path) = ctx.file_decl_index.types.get(&n)
            && path.as_path() != ctx.source_path.as_path()
        {
            path.clone()
        } else {
            continue;
        };
        by_path.entry(path).or_default().insert(n);
    }
    let mut imported = HashSet::new();
    let stmts = by_path
        .into_iter()
        .map(|(target_path, names)| {
            let from = cross_commons_import_specifier_for_path(
                &ctx.source_path,
                &target_path,
                ctx.import_ext,
            );
            let parts = names
                .iter()
                .flat_map(|n| {
                    ["__serialise_", "__deserialise_"]
                        .map(|prefix| codec_import_spec(ctx, prefix, n))
                })
                .collect();
            imported.extend(names);
            bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::Import {
                    type_only: false,
                    names: parts,
                    from,
                },
                None,
            )
        })
        .collect();
    (stmts, imported)
}

/// #2027: a whole-name [`serialisation`] `Qual` naming each `uses`-imported
/// commons type by its unbranded import, `Line` as `__CommonsLine`, for a
/// context's generic-instantiation codecs (`__serialise_List_Line`).
///
/// The context's own `Line` is rebranded (`emit_context_rebrands`), and so is
/// every other context's. One instantiation codec serves both this context's
/// values and, through a consumed context's codec (`value: ev_store.Batch`),
/// the callee's, whose `Line` carries the callee's brand: named by the local
/// rebrand, the codec rejected the callee's value (`TS2345`). The unbranded
/// type accepts either, as the commons' own `__serialise_Line` does, and a
/// decoded unbranded value is assignable to any context's optionally-branded
/// type. Empty outside a context, which has no rebrands. `settle_implied`
/// reads a spelled `__CommonsLine` as a use of `Line`, so its import and
/// rebrand are emitted.
fn unbranded_commons_qual(commons: &TypedCommons, ctx: &EmitProjectCtx) -> HashMap<String, String> {
    if ctx.unit_kind != UnitKind::Context || ctx.owning_context.is_none() {
        return HashMap::new();
    }
    commons
        .types
        .keys()
        .filter(|n| {
            bynk_check::resolver::compute_is_uses_commons_type(
                &ctx.imported_from_kind,
                &commons.types,
                n,
            )
        })
        .map(|n| (n.clone(), format!("__Commons{n}")))
        .collect()
}

/// #661: emit the caller's own `serialise_*`/`deserialise_*` for every
/// callee-owned boundary type reachable from the services this context
/// **calls**, so a `workers` cross-context call resolves its codecs locally
/// rather than importing the callee's module as a value.
///
/// The codec function names stay bare and local; only the TS *type* positions
/// reach through the callee's `import type * as <ns>` alias (via the `Qual`
/// map built here). Refinement validation follows the export visibility: an
/// opaque type casts structurally (Decision C), a transparent refined type
/// inlines its predicates (Decision D) — both decided inside the codec emitter
/// from the type's own body.
///
/// Only the callee-*owned* types (its `exports`) are generated. Commons types
/// reachable through the boundary (`Money`) are already emitted or re-exported
/// by the caller's own path, so they are left out here and deduped against
/// `emitted_names` / `emitted_insts`, which the caller seeds with everything it
/// has already emitted. Returns the names and generic-instantiation names newly
/// emitted, so the Json-codec pass dedupes against them too.
/// #1478: returns real [`bynk_ts::TsStmt`]s (was `out: &mut String`) as a
/// new first element of the tuple — every `out`-write here is already
/// exclusively through `serialisation::decls_as_stmts[_block]`, called
/// directly since that slice, appended into `stmts`.
fn emit_consumed_context_helpers(
    program: &CheckedProgram,
    ctx: &EmitProjectCtx,
    own_roots: &HashMap<String, Vec<bynk_syntax::ast::TypeRef>>,
    emitted_names: &mut HashSet<String>,
    emitted_insts: &mut HashSet<String>,
) -> (Vec<bynk_ts::TsStmt>, Vec<String>, Vec<String>, Vec<String>) {
    use serialisation::{
        collect_codec_closure, emit_generic_helpers_qualified, emit_helpers_for_owner_qualified,
    };
    // Review of #1211: derived, not taken as a second parameter — see
    // `emit_boundary_helpers`'s own identical note.
    let commons = program.program();
    let info = &ctx.cross_context;
    let mut stmts: Vec<bynk_ts::TsStmt> = Vec::new();
    let mut consumed_names_out: Vec<String> = Vec::new();
    let mut consumed_insts_out: Vec<String> = Vec::new();
    let mut foreign_names_out: Vec<String> = Vec::new();

    // Only the services this context actually **calls** — not the callee's whole
    // provided surface. `consumed_services` carries every service the dependency
    // provides; generating a codec for one this context never reaches would bloat
    // the bundle with a contract it does not participate in (and pull in the
    // uncalled service's own boundary types). Mirrors the `called` narrowing the
    // contract manifest applies to `expects` (ADR 0200 Decision E, one layer up).
    let called = called_consumed_services(commons, info);

    // #973: this context's own `from Events(E)` subscriptions, keyed by the
    // consumed context that declares each `E` — a subscriber calls no method
    // on the publisher, so `called_consumed_services` alone would never see
    // it, and the `continue` below on an empty `called_here` would skip the
    // event's payload type entirely (the root cause of #973: a subscriber's
    // generated module had no `deserialise_<Payload>` at all).
    // #1187's own closing scoping pass: the event's own identity now reads
    // `ProtocolIr::Events`'s already-resolved `event: TyId` (via
    // `lower_protocol_ir`) instead of a raw match on `svc.protocol`/
    // `event_type`. `collect_codec_closure` below is still `TypeRef`-driven
    // (confirmed: no resolved-field-type table exists anywhere in
    // `bynk-check` to substitute), so `ty_to_type_ref` converts back at the
    // end — the same TyId-in, TypeRef-out shape `lower_json_codec_call`
    // already uses for the identical reason. `event`'s own type is always
    // non-generic in practice: `context_checks.rs`'s own event/handler
    // param-type-agreement check (`type_ref_named`) only ever admits a bare
    // `Named` event type, so `ty_to_type_ref`'s generic-instantiation arm
    // never actually fires here — not relied on silently, just not the
    // shape a certified program can produce.
    let mut consumed_event_roots: HashMap<String, Vec<bynk_syntax::ast::TypeRef>> = HashMap::new();
    for item in &commons.commons.items {
        let CommonsItem::Service(svc) = item else {
            continue;
        };
        let bynk_model::ProtocolIr::Events { event, .. } =
            bynk_model::lower_protocol_ir(&svc.protocol, program)
        else {
            continue;
        };
        // Review of #1211: unlike the raw-AST match this replaced, a
        // resolve miss here (`lower_protocol_ir` degrades to `Ty::Unit`
        // rather than propagating a failure, `ir/lower.rs`'s own
        // `resolve_type_ref`/`unit_ty()` fallback) is indistinguishable
        // from a legitimately non-`Named` header — a silent `continue`
        // would reproduce #973's own regression shape (a subscriber module
        // missing its `deserialise_<Payload>`) with no diagnostic. The
        // event/handler param-type-agreement check (`context_checks.rs`'s
        // `bynk.event.handler_param_type_mismatch`) makes this
        // unreachable on a certified program, but that argument is longer
        // than the old syntactic match needed — enforced here, not just
        // narrated, so a day this stops holding fails loud in the bless
        // suite (which runs debug) instead of quietly shipping a
        // subscriber with no deserialiser.
        let resolved = commons.tys().get(event);
        debug_assert!(
            matches!(&*resolved, Ty::Named { .. }),
            "bynk internal error: a `from Events(...)` header resolved to a non-named \
             type — a certified program cannot produce this (see #973)"
        );
        let Ty::Named { name, .. } = &*resolved else {
            continue;
        };
        let Some(event_type) = ty_to_type_ref(event, commons.tys()) else {
            continue;
        };
        for (c, names) in &info.consumed_event_names {
            if names.contains(name) {
                consumed_event_roots
                    .entry(c.clone())
                    .or_default()
                    .push(event_type.clone());
            }
        }
    }

    let empty_svcs: HashMap<String, bynk_check::resolver::CrossContextService> = HashMap::new();
    let empty_called: HashSet<String> = HashSet::new();
    let empty_event_roots: Vec<bynk_syntax::ast::TypeRef> = Vec::new();

    let mut consumed_keys: HashSet<&String> = info.consumed_services.keys().collect();
    consumed_keys.extend(consumed_event_roots.keys());
    consumed_keys.extend(own_roots.keys());
    let mut consumed_keys: Vec<&String> = consumed_keys.into_iter().collect();
    consumed_keys.sort();
    for c in consumed_keys {
        let svcs = info.consumed_services.get(c).unwrap_or(&empty_svcs);
        let event_roots = consumed_event_roots.get(c).unwrap_or(&empty_event_roots);
        // #2036: `c`'s types on this context's own boundary.
        let own = own_roots.get(c).unwrap_or(&empty_event_roots);
        let called_here = called.get(c).unwrap_or(&empty_called);
        if (svcs.is_empty() || called_here.is_empty()) && event_roots.is_empty() && own.is_empty() {
            continue;
        }
        let Some(types_table) = info.consumed_types.get(c) else {
            continue;
        };
        // The callee's exports — the set of types it *owns* and a consumer may
        // name. A closure type outside this set (a commons type the callee only
        // `uses`, e.g. `Money`) is the caller's own already, not the callee's to
        // hand out, so the caller never regenerates it under the callee's ns.
        let exports = ctx.exports_for_consumed.get(c);
        let owned = |n: &str| exports.is_some_and(|e| e.contains_key(n));
        // #1846: a type the callee declares but does not export (`Code`, a
        // field of its exported `Ticket`) is the callee's too, though the
        // caller can't name it and may declare its own `Code`. It is renamed
        // in this copy of the callee's table (`t_vault__Code`), so its codecs
        // and instantiations can't collide with the caller's, and generated
        // here like an owned type, its TS type still the callee's own.
        let (renamed_table, hidden) = rename_unexported_types(
            c,
            types_table,
            &owned,
            &commons.types,
            &ctx.imported_from_kind,
        );
        let types_table = &renamed_table;
        let hidden_subst = hidden_subst(&hidden);

        // Roots: every called service's parameter and return types, plus (#973)
        // any event type this context subscribes to from `c` — a subscriber
        // participates in the event's contract as its receiving half, so its
        // payload is not an uncalled surface the way an unreached method is
        // (the narrowing this loop otherwise applies, mirroring ADR 0200
        // Decision E one layer up, at `called_consumed_services` above).
        let mut svc_names: Vec<&String> =
            svcs.keys().filter(|s| called_here.contains(*s)).collect();
        svc_names.sort();
        let mut roots: Vec<bynk_syntax::ast::TypeRef> = event_roots.clone();
        roots.extend(own.iter().cloned());
        for sn in svc_names {
            let svc = &svcs[sn];
            for (_, t) in &svc.params {
                roots.push(t.clone());
            }
            roots.push(svc.return_type.clone());
        }
        let roots: Vec<bynk_syntax::ast::TypeRef> = roots
            .iter()
            .map(|r| serialisation::rename_type_ref(r, &hidden_subst))
            .collect();
        let (names, cinsts) = collect_codec_closure(&roots, types_table);

        let ns = format!("{}.", qualified_to_ns(c));
        let mut qual: HashMap<String, String> = HashMap::new();
        for n in &names {
            if owned(n) {
                qual.insert(n.clone(), ns.clone());
            }
        }
        // #1846: the whole TS name, `t_vault.Code` (see `serialisation::qual_type`).
        // #2073: a type `c` consumes from another context is that context's,
        // so its TS name goes through the declaring context (`shop_order.Order`),
        // which the caller consumes too (the checker refuses the call
        // otherwise). Its codecs keep `c`'s renamed names, which the call sites
        // share.
        // A key `contract_types_for` qualified by its declaring context
        // (`shop_order__Region`, the declaration's name too, #2087) is named in
        // TS by the name that context gives it (`shop_order.Region`). The
        // declaring context's namespace is reached when this module consumes
        // it (imported by `emit_cross_context_namespace_imports`); otherwise
        // through `c`'s, whose `handlers.ts` re-exports each consumed type it
        // imports (#2044).
        let owners = info.consumed_type_owners.get(c);
        for (renamed, original) in &hidden {
            let owner = owners.and_then(|o| o.get(original));
            let owner_ns = owner
                .filter(|owner| info.consumed_contexts.contains(*owner))
                .map(|owner| format!("{}.", qualified_to_ns(owner)))
                .unwrap_or_else(|| ns.clone());
            let ts_name = owner
                .and_then(|owner| original.strip_prefix(&format!("{}__", qualified_to_ns(owner))))
                .unwrap_or(original.as_str());
            // #2890: a type `c` renamed itself is named by the name it declares.
            let ts_name = info
                .callee_type_aliases
                .get(c)
                .and_then(|a| a.get(ts_name))
                .map_or(ts_name, String::as_str);
            qual.insert(renamed.clone(), format!("{owner_ns}{ts_name}"));
        }
        // #2887: a type this context renames from `c` is named in TS by the
        // name `c` declares, whole.
        for (alias, original) in info.consumed_type_aliases.get(c).into_iter().flatten() {
            if names.contains(alias) {
                qual.insert(alias.clone(), format!("{ns}{original}"));
            }
        }
        // #1736: a generic type the callee owns reaches the closure only as the
        // base of an instantiation (`Envelope[Int]`), never as a plain name, so
        // the loop above can't see it. Without this the instantiation's codec
        // named it bare (`Envelope<number>`), which the caller doesn't declare:
        // `TS2304`, failing every consumer of a service that returns one.
        for i in &cinsts {
            if let serialisation::GenericInst::RecordInst { name, .. }
            | serialisation::GenericInst::SumInst { name, .. } = i
                && owned(name)
            {
                qual.insert(name.clone(), ns.clone());
            }
        }

        // #1823: a closure type the callee does not own is a commons type
        // (`Cents`); the caller imports its codec from the commons. Before,
        // that was assumed done by the caller's own boundary path, which only
        // covers types in the caller's own signatures.
        let generated = |n: &str| owned(n) || hidden.contains_key(n);
        foreign_names_out.extend(names.iter().filter(|n| !generated(n)).cloned());
        let mut to_emit: Vec<String> = names
            .iter()
            .filter(|n| generated(n) && emitted_names.insert((*n).clone()))
            .cloned()
            .collect();
        to_emit.sort();
        stmts.extend(serialisation::decls_as_stmts_block(
            emit_helpers_for_owner_qualified(
                &to_emit,
                types_table,
                ctx.commons_name.as_str(),
                &qual,
                &ctx.runtime_use,
            ),
        ));
        consumed_names_out.extend(to_emit);

        let to_emit_insts: Vec<serialisation::GenericInst> = cinsts
            .into_iter()
            .filter(|i| emitted_insts.insert(i.ts_name()))
            .collect();
        for i in &to_emit_insts {
            consumed_insts_out.push(i.ts_name());
        }
        // #2027: a commons type argument is named unbranded, as the caller's
        // own instantiations name it (`unbranded_commons_qual`); the callee's
        // own types keep their namespace.
        let mut inst_qual = unbranded_commons_qual(commons, ctx);
        inst_qual.extend(qual);
        stmts.extend(serialisation::decls_as_stmts(
            emit_generic_helpers_qualified(
                &to_emit_insts,
                types_table,
                &inst_qual,
                &ctx.runtime_use,
            ),
        ));
    }

    (
        stmts,
        consumed_names_out,
        consumed_insts_out,
        foreign_names_out,
    )
}

/// #1846: `types_table`, a consumed context `c`'s type table, with each type
/// `c` declares but does not export renamed `<ns>__<Name>` (`t_vault__Code`),
/// wherever it is declared or named; and those types, renamed to original.
/// A type that isn't `c`'s own (a commons type it `uses`, whose codecs the
/// caller imports, [`commons_codec_imports`]) keeps its name: it is the
/// caller's same declaration, under a codec owner.
fn rename_unexported_types(
    c: &str,
    types_table: &HashMap<String, Arc<TypeDecl>>,
    owned: &dyn Fn(&str) -> bool,
    caller_types: &HashMap<String, Arc<TypeDecl>>,
    imported_from_kind: &HashMap<String, UnitKind>,
) -> (
    HashMap<String, Arc<TypeDecl>>,
    std::collections::BTreeMap<String, String>,
) {
    let hidden = unexported_type_names(c, types_table, owned, caller_types, imported_from_kind);
    if hidden.is_empty() {
        return (types_table.clone(), hidden);
    }
    let subst = hidden_subst(&hidden);
    let renamed = types_table
        .iter()
        .map(|(n, d)| {
            let name = subst.get(n).cloned().unwrap_or_else(|| n.clone());
            (name, Arc::new(rename_in_decl(d, &subst)))
        })
        .collect();
    (renamed, hidden)
}

/// #1846, #1932: which of consumed context `c`'s types the consumer renames,
/// as `renamed → original`: those it neither owns through `c`'s exports nor
/// shares through a commons. The one decision both [`rename_unexported_types`]
/// (the codec generator's table) and [`unexported_type_renames`] (the call
/// sites' map) take, so the two always agree.
fn unexported_type_names(
    c: &str,
    types_table: &HashMap<String, Arc<TypeDecl>>,
    owned: &dyn Fn(&str) -> bool,
    caller_types: &HashMap<String, Arc<TypeDecl>>,
    imported_from_kind: &HashMap<String, UnitKind>,
) -> std::collections::BTreeMap<String, String> {
    let shared = |n: &str, d: &Arc<TypeDecl>| {
        matches!(imported_from_kind.get(n), Some(UnitKind::Commons))
            && caller_types
                .get(n)
                .is_some_and(|mine| Arc::ptr_eq(mine, d) || mine.span == d.span)
    };
    let prefix = qualified_to_ns(c);
    types_table
        .iter()
        .filter(|(n, d)| !owned(n) && !shared(n, d))
        .map(|(n, _)| (format!("{prefix}__{n}"), n.clone()))
        .collect()
}

/// #1932: the renames `rename_unexported_types` gives consumed context `c`'s
/// types, as `original → renamed`, for the call site: a cross-context call
/// names the codecs `emit_consumed_context_helpers` generates under the
/// same names. Computed once per consumed context, where the emitter's
/// `CrossContextInfo` is assembled, from the inputs the generator uses.
pub fn unexported_type_renames(
    c: &str,
    types_table: &HashMap<String, Arc<TypeDecl>>,
    exports: Option<&HashMap<String, bynk_syntax::ast::Visibility>>,
    caller_types: &HashMap<String, Arc<TypeDecl>>,
    imported_from_kind: &HashMap<String, UnitKind>,
) -> HashMap<String, String> {
    let owned = |n: &str| exports.is_some_and(|e| e.contains_key(n));
    hidden_subst(&unexported_type_names(
        c,
        types_table,
        &owned,
        caller_types,
        imported_from_kind,
    ))
}

/// #1846: `original → renamed`, as [`serialisation::rename_type_ref`] takes it.
fn hidden_subst(hidden: &std::collections::BTreeMap<String, String>) -> HashMap<String, String> {
    hidden
        .iter()
        .map(|(renamed, original)| (original.clone(), renamed.clone()))
        .collect()
}

/// #1846: `decl` with every type it names, and its own name, renamed by `subst`.
pub(crate) fn rename_in_decl(decl: &TypeDecl, subst: &HashMap<String, String>) -> TypeDecl {
    let mut out = decl.clone();
    if let Some(to) = subst.get(&decl.name.name) {
        out.name.name = to.clone();
    }
    match &mut out.body {
        TypeBody::Record(r) => {
            for f in &mut r.fields {
                f.type_ref = serialisation::rename_type_ref(&f.type_ref, subst);
            }
        }
        TypeBody::Sum(sum) => {
            for v in &mut sum.variants {
                for f in &mut v.payload {
                    f.type_ref = serialisation::rename_type_ref(&f.type_ref, subst);
                }
            }
            for e in &mut sum.embeds {
                e.source_type = serialisation::rename_type_ref(&e.source_type, subst);
            }
        }
        TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
    }
    out
}

/// #661: the cross-context services this unit actually **calls**, as `consumed
/// context → service names`. A copy of `project::called_cross_context_services`
/// over the emitter's AST view (`commons`) — the caller-side codec set follows
/// the *called* subset, not the callee's full provided surface, so it stays in
/// step with what the contract manifest records under `expects`.
///
/// P6.56 (design/tracks/the-ir.md §6b): reads the checker's own
/// already-resolved `Callee::Cross { unit, service }` for each visited call
/// site instead of reconstructing cross-context-ness syntactically
/// (`flatten_emit_ident_chain` on a `MethodCall` receiver, then
/// `info.resolve_prefix`) — the identical resolution `CrossContextInfo::
/// resolve_prefix` already performed once, at check time, per call site
/// (`project::called_cross_context_services`'s own #1187 conversion, the
/// direct precedent this mirrors). Same shadowing-hazard class `block_uses_emit`
/// closed for `Events.emit`.
fn called_consumed_services(
    commons: &TypedCommons,
    info: &bynk_check::resolver::CrossContextInfo,
) -> HashMap<String, HashSet<String>> {
    let mut out: HashMap<String, HashSet<String>> = HashMap::new();
    if info.consumed_contexts.is_empty() && info.aliases.is_empty() {
        return out;
    }
    let mut visit = |e: &Expr| {
        if let Some(bynk_check::checker::Callee::Cross { unit, service }) =
            commons.callees.get(&e.id)
        {
            out.entry(unit.clone()).or_default().insert(service.clone());
        }
    };
    for item in &commons.commons.items {
        match item {
            CommonsItem::Service(s) => {
                for h in &s.handlers {
                    walk_block_exprs(&h.body, &mut visit);
                }
            }
            CommonsItem::Agent(a) => {
                for h in &a.handlers {
                    walk_block_exprs(&h.body, &mut visit);
                }
            }
            CommonsItem::Provider(p) => {
                for op in &p.ops {
                    walk_block_exprs(&op.body, &mut visit);
                }
            }
            _ => {}
        }
    }
    out
}

/// For each type imported via `uses` that's referenced in this file, emit:
/// 1. (Done in imports) an aliased import: `import { Money as __CommonsMoney } from ...`
/// 2. A rebranded type alias: `export type Money = __CommonsMoney & { readonly __ctxBrand?: "..." }`
///
/// The brand makes two contexts that both `uses` the same commons see distinct
/// nominal `Money` types in their TypeScript output (v0.4 §3.4 / §6.2). It is
/// optional (#1704), so an unbranded commons value (a record literal, a nested
/// field of another rebranded record, a commons fn's result) is accepted where
/// the context's type is expected, while a value carrying another context's
/// brand is still rejected.
/// #1478: real-node-internally already — returns its real declarations
/// (a rebrand type alias, and — for a refined/opaque base — a forwarding
/// const, per name) instead of writing into `out: &mut String`.
/// #2036: on `workers`, re-export as types the names this `handlers.ts`
/// imports from a consumed context. `compose.ts` names every type in a
/// wrapper's signature as `handlers.<Name>` (it imports no type names of its
/// own), so a consumed `Batch` in a handler's parameters must be a member of
/// `handlers` too (a consumed *adapter's* type — `R2Upload` — likewise, #2647:
/// it is imported from the adapter's module by the same statement), the way a
/// `uses`d commons type is through its rebrand alias. Type-only
/// (`export { type Batch }`): `compose.ts` uses the name in type position
/// only, a consumed refined alias has no value to re-export (TS1205 under
/// `isolatedModules`), and a value re-export would deepen the caller's
/// dependency on the callee's module. Only names the module imports
/// are re-exported, so each is bound here.
fn emit_consumed_type_reexports(
    refs: &ExternalReferences,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    if ctx.target != BuildTarget::Workers {
        return Vec::new();
    }
    // #2647: a consumed *adapter's* types are re-exported only when some
    // service handler's parameter names them, in the handler kinds below. A consumed
    // context's are re-exported whenever imported, as before.
    let handler_param_types: HashSet<String> = commons
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Service(s) => Some(s),
            _ => None,
        })
        .flat_map(|s| s.handlers.iter())
        // The kinds whose `compose.ts` wrapper types its parameters with
        // `qualified_ts_type_ref` (call, cron, queue `message`, http); an
        // `event` handler's other parameters never reach `compose.ts`.
        .filter(|h| {
            matches!(
                h.kind,
                bynk_syntax::ast::HandlerKind::Call
                    | bynk_syntax::ast::HandlerKind::Http { .. }
                    | bynk_syntax::ast::HandlerKind::Cron { .. }
                    | bynk_syntax::ast::HandlerKind::Message
            )
        })
        .flat_map(|h| h.params.iter())
        .flat_map(|p| workers::named_types_in(&p.type_ref))
        .collect();
    let mut names: Vec<String> = refs
        .by_commons
        .iter()
        .flat_map(|(unit, set)| {
            let adapter = ctx.consumed_adapters.contains(unit.as_str());
            let handler_param_types = &handler_param_types;
            set.iter()
                .filter(move |n| !adapter || handler_param_types.contains(n.as_str()))
        })
        .filter(|n| ctx.imported_from_kind.get(n.as_str()) == Some(&UnitKind::Context))
        .map(|n| format!("type {}", ts_ident(n)))
        .collect();
    if names.is_empty() {
        return Vec::new();
    }
    names.sort();
    names.dedup();
    vec![bynk_ts::TsStmt::decl(
        bynk_ts::TsDecl::ExportNames { names },
        None,
    )]
}

fn emit_context_rebrands(
    refs: &ExternalReferences,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    let Some(owning) = &ctx.owning_context else {
        return Vec::new();
    };
    // Collect names imported via `uses` (kind == Commons in imported_from_kind).
    // R4.10/R8.2: reads `bynk_check::resolver::compute_is_uses_commons_type`, the same
    // predicate `prepare_unit_check_ctx` (`check_pipeline.rs`) computes its own
    // `uses_commons_type_names` from — one definition, not two independently
    // maintained copies linked only by a doc-comment promise (see that
    // function's own doc comment for the real defect this closes, ADR 0226).
    let mut names: Vec<String> = Vec::new();
    for set in refs.by_commons.values() {
        for n in set {
            // v0.20b: only *types* get the context rebrand — a
            // `uses`-imported function is a value and imports plainly.
            if bynk_check::resolver::compute_is_uses_commons_type(
                &ctx.imported_from_kind,
                &commons.types,
                n,
            ) {
                names.push(n.clone());
            }
        }
    }
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Vec::new();
    }
    let mut stmts = Vec::new();
    for name in &names {
        // v0.174 (#592): a generic commons type keeps its parameters across the
        // rebrand — `Paginated[T]` aliases as `Paginated<T> =
        // __CommonsPaginated<T> & { … }`, not a bare `Paginated`, which would
        // both drop the parameter and make every `Paginated<User>` reference in
        // the context a "type is not generic" error.
        let params: Vec<&str> = commons
            .types
            .get(name)
            .map(|d| d.type_params.iter().map(|p| p.name.name.as_str()).collect())
            .unwrap_or_default();
        let generic_args: Vec<bynk_ts::TsType> =
            params.iter().map(|p| bynk_ts::TsType::named(*p)).collect();
        // Arc C, slice 30 (#1392): the SAME `TsDecl::TypeAlias` over
        // `TsType::Intersection` shape #1339's own `emit_refined_type`
        // already established for its sibling `__brand` alias (a real,
        // proven precedent, not this slice's own new gap).
        let type_alias = bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::TypeAlias {
                name: name.clone(),
                type_params: params.iter().map(|p| p.to_string()).collect(),
                ty: bynk_ts::TsType::intersection(vec![
                    bynk_ts::TsType::named_with_args(format!("__Commons{name}"), generic_args),
                    // #1704: the brand is *optional*. A plain commons value (a
                    // record literal, a nested field, a commons fn's result)
                    // has no brand, and it is assignable here; another
                    // context's brand (`"a"` against `"b"`) still is not, so
                    // the two contexts' types stay nominally distinct.
                    bynk_ts::TsType::Object(vec![bynk_ts::TsTypeMember::Prop {
                        name: "__ctxBrand".to_string(),
                        ty: bynk_ts::TsType::named(format!("\"{owning}\"")),
                        optional: true,
                        readonly: true,
                    }]),
                ]),
            })),
            None,
        );
        stmts.push(type_alias);
        // v0.9.2: a commons refined/opaque type carries a value-side
        // constructor (`.of`, and `.unsafe` for opaque). Re-export it under the
        // rebranded name so a context calling `ShortCode.of(...)` resolves to a
        // value — delegating to the imported commons constructor but reporting
        // the context-branded type. (Without this, `ShortCode` is type-only in
        // the context and `.of` fails to resolve.)
        if let Some(base) = commons
            .types
            .get(name)
            .and_then(|d| refined_or_opaque_base(d))
        {
            let ts_base_name = ts_base(base);
            let is_opaque = matches!(
                commons.types.get(name).map(|d| &d.body),
                Some(TypeBody::Opaque { .. })
            );
            // #1704: `return __CommonsX.of(value);` with no cast — the
            // commons result is unbranded, which fits the context's
            // optionally-branded `Result<X, ValidationError>` directly.
            let of_entry = bynk_ts::TsObjectEntry::Method {
                name: "of".to_string(),
                is_async: false,
                generics: Vec::new(),
                params: vec![bynk_ts::TsParam {
                    name: "value".to_string(),
                    ty: Some(bynk_ts::TsType::named(ts_base_name)),
                    optional: false,
                }],
                return_type: Some(bynk_ts::TsType::named_with_args(
                    "Result",
                    vec![
                        bynk_ts::TsType::named(name.clone()),
                        bynk_ts::TsType::named("ValidationError"),
                    ],
                )),
                doc: None,
                inline: true,
                body: vec![bynk_ts::TsStmt::return_stmt(
                    Some(bynk_ts::TsExpr::Call {
                        callee: Box::new(bynk_ts::TsExpr::Member {
                            object: Box::new(bynk_ts::TsExpr::Ident(format!("__Commons{name}"))),
                            property: "of".to_string(),
                        }),
                        args: vec![bynk_ts::TsExpr::Ident("value".to_string())],
                    }),
                    None,
                )],
            };
            let mut entries = vec![of_entry];
            // ADR 0182: only opaque types have a public `.unsafe` to forward.
            // A refined/alias type has none — a consuming context brands an
            // admitted literal with an inline `as` cast, not a forwarder call.
            if is_opaque {
                entries.push(bynk_ts::TsObjectEntry::Method {
                    name: "unsafe".to_string(),
                    is_async: false,
                    generics: Vec::new(),
                    params: vec![bynk_ts::TsParam {
                        name: "value".to_string(),
                        ty: Some(bynk_ts::TsType::named(ts_base_name)),
                        optional: false,
                    }],
                    return_type: Some(bynk_ts::TsType::named(name.clone())),
                    doc: None,
                    inline: true,
                    body: vec![bynk_ts::TsStmt::return_stmt(
                        Some(bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Member {
                                object: Box::new(bynk_ts::TsExpr::Ident(format!(
                                    "__Commons{name}"
                                ))),
                                property: "unsafe".to_string(),
                            }),
                            args: vec![bynk_ts::TsExpr::Ident("value".to_string())],
                        }),
                        None,
                    )],
                });
            }
            // v0.132.1 (#481): forward the commons' user-defined attached methods
            // (`Cents.fromInt`, …) so the rebranded const carries more than the
            // built-in `of`/`unsafe`. Without this a consumer's `Cents.fromInt(n)`
            // — which `bynk check` accepts — fails `tsc`. The methods aren't in
            // this context's own `commons` (only imported *types* are merged);
            // they arrive via `ctx.imported_methods`, keyed by type name.
            if let Some(methods) = ctx.imported_methods.get(name) {
                entries.extend(emit_forwarded_methods(
                    name,
                    methods,
                    UnitFacts::of(ctx).render(commons),
                ));
            }
            let const_decl = bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
                    name: name.clone(),
                    ty: None,
                    init: bynk_ts::TsExpr::multiline_object_entries(entries),
                })),
                None,
            );
            stmts.push(const_decl);
        } else if ctx
            .imported_methods
            .get(name)
            .is_some_and(|m| !m.is_empty())
        {
            // #2751: a record/sum commons type has no `.of`, but its
            // attached methods are still called as `Type.method(recv)`, which
            // needs a value `Type` in the context (the rebrand above is only
            // a type). The value is the commons' own, re-exported unbranded:
            // its methods already speak types the context's optionally-branded
            // ones fit, and — unlike a re-declared forwarder — it carries a
            // generic type's parameters, a bounded method's hidden comparator
            // and codec witnesses, and a sum's variants unchanged.
            stmts.push(bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::Export(Box::new(bynk_ts::TsDecl::ConstDecl {
                    name: name.clone(),
                    ty: None,
                    init: bynk_ts::TsExpr::Ident(format!("__Commons{name}")),
                })),
                None,
            ));
        }
    }
    // #1486: no explicit trailing blank stmt here — the printer's automatic
    // top-level spacing policy supplies the single blank before whatever
    // follows.
    //
    // Every rebrand entry (a lone `type_alias`, or a `type_alias`+`const_decl`
    // pair) sits directly under its predecessor with no blank between them —
    // confirmed against `129_full_orders_http_api`'s own real fixture output,
    // several rebrands stacked with zero blank lines anywhere among them.
    // Only the very first statement in this whole function's own returned
    // `Vec` keeps the printer's ordinary automatic blank relative to
    // whatever precedes `emit_context_rebrands`'s own call site.
    for stmt in stmts.iter_mut().skip(1) {
        stmt.no_blank_before = true;
    }
    stmts
}

/// If a type declaration is a refined or opaque base type, return its base
/// (both lower to a branded base with a `.of` / `.unsafe` constructor object).
///
/// P6.56 (design/tracks/the-ir.md §6b): investigated routing this through
/// `TypeShape::Refined` and declined — that variant's own `base` field is
/// `bynk_syntax::ast::BaseType` (P6.41 ruled it stays, phase 7 — the bounds
/// keep source lexemes for byte-stable emission), so this function's own
/// `Option<BaseType>` return type is identical either way. Converting would
/// add a `CheckedProgram`/`TypedCommons` dependency this function doesn't
/// have today for zero reduction in AST-type surface.
fn refined_or_opaque_base(decl: &TypeDecl) -> Option<BaseType> {
    match &decl.body {
        TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } => Some(*base),
        _ => None,
    }
}

/// Names that this file needs to import from elsewhere (sibling files of
/// the same commons, or other commons via `uses`).
#[derive(Default)]
struct ExternalReferences {
    /// `commons name` → set of names to import.
    by_commons: HashMap<String, HashSet<String>>,
    /// `sibling source path` → set of names to import (same-commons).
    by_sibling: HashMap<PathBuf, HashSet<String>>,
    /// #1778: names found only in an expression's checked type, keyed like
    /// `by_commons`/`by_sibling`. The lowering spells only some of them, so
    /// each is held here until [`ExternalReferences::settle_implied`] sees the
    /// emitted body. Importing the rest would add an unused import, and in a
    /// context a rebrand the module exports for nothing.
    implied_by_commons: HashMap<String, HashSet<String>>,
    implied_by_sibling: HashMap<PathBuf, HashSet<String>>,
    /// #1820: names imported from a sibling file as types only: a capability
    /// is an `interface`, which has no value to import once the module is
    /// stripped to JavaScript.
    types_by_sibling: HashMap<PathBuf, HashSet<String>>,
    /// #1820: every capability a sibling file declares, settled like
    /// `implied_by_sibling` into `types_by_sibling`. A handler's `deps` type
    /// names the capabilities it needs, and those include what the agents
    /// and services it calls need, so which a file spells is known only
    /// once its body is emitted.
    implied_types_by_sibling: HashMap<PathBuf, HashSet<String>>,
}

impl ExternalReferences {
    fn is_empty(&self) -> bool {
        self.by_commons.is_empty() && self.by_sibling.is_empty() && self.types_by_sibling.is_empty()
    }

    /// #1829: record each of `names`, the types a module's emitted helpers
    /// name, as an *implied* import, resolved the way a source reference is.
    /// A name this file declares, or a consumed context declares, is skipped,
    /// as [`record_ty_refs`] skips it.
    fn imply_names<'a>(
        &mut self,
        names: impl Iterator<Item = &'a String>,
        commons: &TypedCommons,
        ctx: &EmitProjectCtx,
    ) {
        let local_to_file = local_names(commons);
        let mut found = ExternalReferences::default();
        for name in names {
            if !is_consumed_context_type(ctx, name) {
                record_name_ref(name, &local_to_file, ctx, &mut found);
            }
        }
        for (unit, names) in found.by_commons {
            self.implied_by_commons
                .entry(unit)
                .or_default()
                .extend(names);
        }
        for (path, names) in found.by_sibling {
            self.implied_by_sibling
                .entry(path)
                .or_default()
                .extend(names);
        }
    }

    /// #1778: promote each implied name the emitted body spells, as a whole
    /// TypeScript identifier, to a real import, and drop the rest. Called
    /// once the body is built and before the imports and rebrands are, which
    /// read only `by_commons`/`by_sibling`.
    fn settle_implied(&mut self, body: &[bynk_ts::TsStmt]) {
        let mut spelled = body_words(body);
        // #2027: `__CommonsLine` (`unbranded_commons_qual`) is `Line`'s own
        // import alias, so it spells `Line`.
        let aliased: Vec<String> = spelled
            .iter()
            .filter_map(|w| w.strip_prefix("__Commons").map(str::to_string))
            .collect();
        spelled.extend(aliased);
        for (unit, names) in std::mem::take(&mut self.implied_by_commons) {
            let kept: HashSet<String> = names.intersection(&spelled).cloned().collect();
            if !kept.is_empty() {
                self.by_commons.entry(unit).or_default().extend(kept);
            }
        }
        for (path, names) in std::mem::take(&mut self.implied_by_sibling) {
            let kept: HashSet<String> = names.intersection(&spelled).cloned().collect();
            if !kept.is_empty() {
                self.by_sibling.entry(path).or_default().extend(kept);
            }
        }
        for (path, names) in std::mem::take(&mut self.implied_types_by_sibling) {
            let kept: HashSet<String> = names.intersection(&spelled).cloned().collect();
            if !kept.is_empty() {
                self.types_by_sibling.entry(path).or_default().extend(kept);
            }
        }
    }
}

/// Every whole TypeScript identifier `body` spells, as printed.
fn body_words(body: &[bynk_ts::TsStmt]) -> HashSet<String> {
    let mut words = HashSet::new();
    for stmt in body {
        let text = bynk_ts::print_stmt(stmt, 0);
        words.extend(
            text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '$'))
                .filter(|w| !w.is_empty())
                .map(str::to_string),
        );
    }
    words
}

/// The names this file declares, so a reference to one needs no import. A
/// `messages` block declares no importable identifier of its own (its
/// `render` is synthesised separately), so `name()` is `None` there and it
/// contributes nothing.
fn local_names(commons: &TypedCommons) -> HashSet<String> {
    commons
        .commons
        .items
        .iter()
        .filter_map(|i| i.name().map(|n| n.name.clone()))
        .collect()
}

fn collect_external_references(commons: &TypedCommons, ctx: &EmitProjectCtx) -> ExternalReferences {
    // Names declared in this file (so we know what's local-to-file).
    let local_to_file = local_names(commons);

    let mut refs = ExternalReferences::default();

    // Walk every expression and TypeRef in this file's items, recording
    // any reference that resolves to a name declared in a sibling file or
    // an imported commons.
    for item in &commons.commons.items {
        match item {
            CommonsItem::Type(t) => {
                collect_refs_in_type_decl(t, &local_to_file, ctx, &mut refs);
            }
            // Events track, slice 0 (spine #936): an `event`'s field types
            // are collected exactly like a `type`'s, via the same synthetic
            // `TypeDecl` `EventDecl::as_type_decl` builds.
            CommonsItem::Event(e) => {
                collect_refs_in_type_decl(&e.as_type_decl(), &local_to_file, ctx, &mut refs);
            }
            CommonsItem::Fn(f) => {
                collect_refs_in_fn(f, &local_to_file, commons, ctx, &mut refs);
            }
            CommonsItem::Capability(c) => {
                for op in &c.ops {
                    for p in &op.params {
                        collect_refs_in_typeref(&p.type_ref, &local_to_file, ctx, &mut refs);
                    }
                    collect_refs_in_typeref(&op.return_type, &local_to_file, ctx, &mut refs);
                }
            }
            CommonsItem::Provider(p) => {
                // Reference to the capability so we can import it: a sibling
                // file's is implied, below (#1820).
                let _ = &p.capability;
                for op in &p.ops {
                    for param in &op.params {
                        collect_refs_in_typeref(&param.type_ref, &local_to_file, ctx, &mut refs);
                    }
                    collect_refs_in_typeref(&op.return_type, &local_to_file, ctx, &mut refs);
                    collect_refs_in_block(&op.body, &local_to_file, commons, ctx, &mut refs);
                }
            }
            CommonsItem::Service(s) => {
                // #2720: a `via schema(...)` subscriber whose handler declares
                // no `env` gets a synthetic `EventEnvelope` parameter at emit
                // time (`emit_service`); that parameter's type must be imported
                // exactly as a declared `env: EventEnvelope` would be.
                let synthesises_envelope = matches!(
                    &s.protocol,
                    ServiceProtocol::Events {
                        schema_dispatch: Some(_),
                        ..
                    }
                );
                for h in &s.handlers {
                    for p in &h.params {
                        collect_refs_in_typeref(&p.type_ref, &local_to_file, ctx, &mut refs);
                    }
                    // Kept in step with `emit_service`'s `schema_dispatch_env_binder`
                    // on purpose: event handler, `schema_dispatch`, no `env` param.
                    if synthesises_envelope
                        && matches!(h.kind, bynk_syntax::ast::HandlerKind::Event)
                        && h.params.len() < 2
                    {
                        record_name_ref("EventEnvelope", &local_to_file, ctx, &mut refs);
                    }
                    collect_refs_in_typeref(&h.return_type, &local_to_file, ctx, &mut refs);
                    collect_refs_in_block(&h.body, &local_to_file, commons, ctx, &mut refs);
                }
            }
            CommonsItem::Agent(a) => {
                collect_refs_in_typeref(&a.key_type, &local_to_file, ctx, &mut refs);
                for f in &a.store_fields {
                    for arg in &f.kind.args {
                        collect_refs_in_typeref(arg, &local_to_file, ctx, &mut refs);
                    }
                }
                for h in &a.handlers {
                    for p in &h.params {
                        collect_refs_in_typeref(&p.type_ref, &local_to_file, ctx, &mut refs);
                    }
                    collect_refs_in_typeref(&h.return_type, &local_to_file, ctx, &mut refs);
                    collect_refs_in_block(&h.body, &local_to_file, commons, ctx, &mut refs);
                }
            }
            CommonsItem::Actor(a) => {
                if let Some(id) = &a.identity {
                    collect_refs_in_typeref(id, &local_to_file, ctx, &mut refs);
                }
            }
            // A `messages` block's generated table and `render` name
            // `LocaleTag`/`Message`/`MessageArg` and `render`/`renderArg`
            // only under private aliases (#1697), imported by `emit_unit`'s
            // (project.rs) hand-written lines, so a user declaration of the
            // same name in this unit can't capture them. Nothing is
            // registered here; user code that names `LocaleTag` itself
            // imports it the ordinary way.
            // #1883, slice 1a: an asset references no other declaration.
            CommonsItem::Messages(_) | CommonsItem::Asset(_) => {}
            // #2387, slice 1: an embed references no other declaration.
            CommonsItem::Embed(_) => {}
            // #2617: a constant's annotation and initialiser may name types,
            // variants and other constants of a sibling file or used commons.
            CommonsItem::Const(c) => {
                collect_refs_in_typeref(&c.ty, &local_to_file, ctx, &mut refs);
                collect_refs_in_expr(&c.init, &local_to_file, commons, ctx, &mut refs);
            }
        }
    }
    for (cap, path) in &ctx.file_decl_index.capabilities {
        if !local_to_file.contains(cap) && path != &ctx.source_path {
            refs.implied_types_by_sibling
                .entry(path.clone())
                .or_default()
                .insert(cap.clone());
        }
    }
    refs
}

fn collect_refs_in_type_decl(
    t: &TypeDecl,
    local_to_file: &HashSet<String>,
    ctx: &EmitProjectCtx,
    out: &mut ExternalReferences,
) {
    match &t.body {
        TypeBody::Record(r) => {
            for f in &r.fields {
                collect_refs_in_typeref(&f.type_ref, local_to_file, ctx, out);
            }
        }
        TypeBody::Sum(s) => {
            for v in &s.variants {
                for p in &v.payload {
                    collect_refs_in_typeref(&p.type_ref, local_to_file, ctx, out);
                }
            }
        }
        _ => {}
    }
}

fn collect_refs_in_fn(
    f: &FnDecl,
    local_to_file: &HashSet<String>,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    out: &mut ExternalReferences,
) {
    for p in &f.params {
        collect_refs_in_typeref(&p.type_ref, local_to_file, ctx, out);
    }
    collect_refs_in_typeref(&f.return_type, local_to_file, ctx, out);
    // For methods: the attached type may also be elsewhere.
    if let FnName::Method { type_name, .. } = &f.name {
        record_name_ref(&type_name.name, local_to_file, ctx, out);
    }
    collect_refs_in_block(&f.body, local_to_file, commons, ctx, out);
}

fn collect_refs_in_typeref(
    r: &TypeRef,
    local_to_file: &HashSet<String>,
    ctx: &EmitProjectCtx,
    out: &mut ExternalReferences,
) {
    match r {
        TypeRef::Named(id) => record_name_ref(&id.name, local_to_file, ctx, out),
        TypeRef::Result(t, e, _) => {
            collect_refs_in_typeref(t, local_to_file, ctx, out);
            collect_refs_in_typeref(e, local_to_file, ctx, out);
        }
        // Exhaustive over the compound constructors (#527, the #507 disease):
        // the old `_ => {}` catch-all dropped `List[KindCount]` and friends,
        // so a name referenced only inside such a position was never
        // imported and the emitted module failed `tsc`.
        TypeRef::Option(t, _)
        | TypeRef::Effect(t, _)
        | TypeRef::HttpResult(t, _)
        | TypeRef::List(t, _)
        | TypeRef::Set(t, _)
        | TypeRef::Query(t, _)
        | TypeRef::Stream(t, _)
        | TypeRef::Connection(t, _)
        | TypeRef::History(t, _) => collect_refs_in_typeref(t, local_to_file, ctx, out),
        TypeRef::Map(k, v, _) => {
            collect_refs_in_typeref(k, local_to_file, ctx, out);
            collect_refs_in_typeref(v, local_to_file, ctx, out);
        }
        TypeRef::Fn(params, ret, _) => {
            for t in params {
                collect_refs_in_typeref(t, local_to_file, ctx, out);
            }
            collect_refs_in_typeref(ret, local_to_file, ctx, out);
        }
        // v0.157 (ADR 0183): a `Name[Arg, …]` application references the
        // generic type plus every argument — all must be imported.
        TypeRef::App { name, args, .. } => {
            record_name_ref(&name.name, local_to_file, ctx, out);
            for t in args {
                collect_refs_in_typeref(t, local_to_file, ctx, out);
            }
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

fn collect_refs_in_block(
    b: &Block,
    local_to_file: &HashSet<String>,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    out: &mut ExternalReferences,
) {
    for stmt in &b.statements {
        collect_refs_in_stmt(stmt, local_to_file, commons, ctx, out);
    }
    collect_refs_in_expr(&b.tail, local_to_file, commons, ctx, out);
}

fn collect_refs_in_stmt(
    stmt: &Statement,
    local_to_file: &HashSet<String>,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    out: &mut ExternalReferences,
) {
    match stmt {
        Statement::Let(l) | Statement::EffectLet(l) => {
            if let Some(t) = &l.type_annot {
                collect_refs_in_typeref(t, local_to_file, ctx, out);
            }
            collect_refs_in_expr(&l.value, local_to_file, commons, ctx, out);
        }
        Statement::Expect(a) => {
            collect_refs_in_expr(&a.value, local_to_file, commons, ctx, out);
        }
        Statement::Do(d) => {
            collect_refs_in_expr(&d.value, local_to_file, commons, ctx, out);
        }
        Statement::Assign(a) => {
            collect_refs_in_expr(&a.value, local_to_file, commons, ctx, out);
        }
        Statement::Advance(a) => {
            collect_refs_in_expr(&a.amount, local_to_file, commons, ctx, out);
        }
        Statement::Par(p) => {
            for inner in &p.statements {
                collect_refs_in_stmt(inner, local_to_file, commons, ctx, out);
            }
        }
    }
}

fn collect_refs_in_expr(
    e: &Expr,
    local_to_file: &HashSet<String>,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    out: &mut ExternalReferences,
) {
    // #1778: the lowering also names types the source never spells. A
    // literal admitted as a refined type is cast to it (`("a" as Name)`),
    // and the collection kernels, `if` slots and `List.empty` annotate with
    // the `ts_ty` of checked types (`(__x: Name) => …`). So every name in
    // this expression's checked type is a candidate import, kept if the
    // emitted body spells it (`ExternalReferences::settle_implied`).
    if let Some(te) = commons.expr_types.get(&e.id) {
        record_ty_refs(te.ty, local_to_file, commons, ctx, out);
    }
    match &e.kind {
        // A bare ident the checker typed as a sum is a nullary variant
        // constructor — the lowering qualifies it to `Type.Variant`, so the
        // owning type must be imported (v0.18: first hit by `Get` from the
        // consumed bynk surface's `Method`).
        ExprKind::Ident(id) => {
            if let Some(type_name) = sum_owner_of_variant(&id.name, e.id, commons) {
                record_name_ref(&type_name, local_to_file, ctx, out);
            } else if let Some(bynk_check::checker::Callee::FnValue(decl, _)) = commons.callee(e.id)
            {
                // #2868: a function named as a value, from a sibling file or
                // an imported unit, is imported where it is named.
                record_name_ref(&decl.name.display(), local_to_file, ctx, out);
            } else if commons.assets.contains_key(&id.name)
                || commons.embeds.contains_key(&id.name)
                || commons.consts.contains_key(&id.name)
            {
                // #1883, slice 1b: a bare asset name (an `Asset` argument,
                // not just `style.url`, which the field-access arm already
                // records) imports the declaring module's constant.
                record_name_ref(&id.name, local_to_file, ctx, out);
            }
        }
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::DecimalLit { .. }
        | ExprKind::DurationLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::None
        | ExprKind::UnitLit => {}
        // v0.43: a hole's expression may reference imported names.
        ExprKind::Wire(inner) => collect_refs_in_expr(inner, local_to_file, commons, ctx, out),
        ExprKind::InterpStr(parts) => {
            for part in parts {
                if let InterpPart::Hole(hole) = part {
                    collect_refs_in_expr(hole, local_to_file, commons, ctx, out);
                }
            }
        }
        // v0.20a: a lambda — its annotated param types may reference
        // imported types; the body walks like any expression.
        ExprKind::Lambda(lambda) => {
            for p in &lambda.params {
                if let Some(tr) = &p.type_ref {
                    collect_refs_in_typeref(tr, local_to_file, ctx, out);
                }
            }
            collect_refs_in_expr(&lambda.body, local_to_file, commons, ctx, out);
        }
        ExprKind::EffectPure(inner) => {
            collect_refs_in_expr(inner, local_to_file, commons, ctx, out);
        }
        ExprKind::Expect(inner) => {
            collect_refs_in_expr(inner, local_to_file, commons, ctx, out);
        }
        // #1812: a fault claim's call-site identity is an expression too.
        ExprKind::Faults(f) => {
            if let Some(identity) = f.principal.as_ref().and_then(|p| p.identity.as_deref()) {
                collect_refs_in_expr(identity, local_to_file, commons, ctx, out);
            }
            collect_refs_in_expr(&f.call, local_to_file, commons, ctx, out);
        }
        ExprKind::Val { args, .. } => {
            for a in args {
                collect_refs_in_expr(a, local_to_file, commons, ctx, out);
            }
        }
        ExprKind::ListLit(elems, ..) => {
            for el in elems {
                collect_refs_in_expr(el, local_to_file, commons, ctx, out);
            }
        }
        // v0.117: observation predicates may reference types/fns; `trace` does not.
        ExprKind::Observation(o) => {
            if let ObservationMatcher::Called { count, with_pred } = &o.matcher {
                if let Some(c) = count {
                    collect_refs_in_expr(c, local_to_file, commons, ctx, out);
                }
                if let Some(p) = with_pred {
                    collect_refs_in_expr(p, local_to_file, commons, ctx, out);
                }
            }
        }
        ExprKind::Trace { .. } => {}
        ExprKind::RecordSpread {
            type_name,
            base,
            overrides,
            ..
        } => {
            if let Some(tn) = type_name {
                record_name_ref(&tn.name, local_to_file, ctx, out);
            }
            collect_refs_in_expr(base, local_to_file, commons, ctx, out);
            for f in overrides {
                if let Some(v) = &f.value {
                    collect_refs_in_expr(v, local_to_file, commons, ctx, out);
                }
            }
        }
        ExprKind::Call { name, args, .. } => {
            record_name_ref(&name.name, local_to_file, ctx, out);
            // #1820: constructing an agent declared in a sibling file of a
            // multi-file unit lowers to its factory, `__make<Agent>`, which
            // that file exports.
            // The checker's classification decides, as the lowering does
            // (`Callee::AgentInit`): a sum variant spelled like an agent is
            // not one. The index only supplies the declaring file.
            if !local_to_file.contains(&name.name)
                && matches!(
                    commons.callee(e.id),
                    Some(bynk_check::checker::Callee::AgentInit(_))
                )
                && let Some(path) = ctx.file_decl_index.agents.get(&name.name)
                && path != &ctx.source_path
            {
                out.by_sibling
                    .entry(path.clone())
                    .or_default()
                    .insert(agent_factory_name(&name.name));
            }
            // A payload-carrying bare variant call (`Won(prize)`) lowers to
            // `Type.Variant(…)` — import the owning sum type too.
            if let Some(type_name) = sum_owner_of_variant(&name.name, e.id, commons) {
                record_name_ref(&type_name, local_to_file, ctx, out);
            }
            // #527: a call to a commons-imported fn may lower with a rebrand
            // assertion naming its return type (`(decide(…) as Decision)`),
            // so the return type's names must be imported (and rebranded)
            // in step with the cast.
            if ctx.unit_kind == UnitKind::Context
                && ctx.imported_from_kind.get(&name.name) == Some(&UnitKind::Commons)
                && let Some(f) = commons.fns.get(&name.name)
            {
                collect_refs_in_typeref(&f.return_type, local_to_file, ctx, out);
            }
            for a in args {
                collect_refs_in_expr(a, local_to_file, commons, ctx, out);
            }
        }
        ExprKind::BinOp(_, l, r) => {
            collect_refs_in_expr(l, local_to_file, commons, ctx, out);
            collect_refs_in_expr(r, local_to_file, commons, ctx, out);
        }
        ExprKind::UnaryOp(_, i)
        | ExprKind::Paren(i)
        | ExprKind::Ok(i)
        | ExprKind::Err(i)
        | ExprKind::Some(i)
        | ExprKind::Question(i) => collect_refs_in_expr(i, local_to_file, commons, ctx, out),
        ExprKind::Block(b) => collect_refs_in_block(b, local_to_file, commons, ctx, out),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            collect_refs_in_expr(cond, local_to_file, commons, ctx, out);
            collect_refs_in_block(then_block, local_to_file, commons, ctx, out);
            collect_refs_in_block(else_block, local_to_file, commons, ctx, out);
        }
        ExprKind::RecordConstruction {
            type_name, fields, ..
        } => {
            record_name_ref(&type_name.name, local_to_file, ctx, out);
            for f in fields {
                if let Some(v) = &f.value {
                    collect_refs_in_expr(v, local_to_file, commons, ctx, out);
                }
            }
        }
        ExprKind::FieldAccess { receiver, field: _ } => {
            // The bare-ident-as-type case (`TypeName.Variant`) — record the
            // name so we import the type.
            if let ExprKind::Ident(id) = &receiver.kind {
                record_name_ref(&id.name, local_to_file, ctx, out);
            } else {
                collect_refs_in_expr(receiver, local_to_file, commons, ctx, out);
            }
        }
        ExprKind::MethodCall {
            receiver,
            method: _,
            type_args,
            args,
            ..
        } => {
            // #2795: a type named only in an explicit list (`b.probe[Item]()`,
            // `Box.empty[Item]()`) is kept in the emitted call, so it is imported.
            for t in type_args {
                collect_refs_in_typeref(t, local_to_file, ctx, out);
            }
            if let ExprKind::Ident(id) = &receiver.kind {
                record_name_ref(&id.name, local_to_file, ctx, out);
            } else {
                collect_refs_in_expr(receiver, local_to_file, commons, ctx, out);
            }
            for a in args {
                collect_refs_in_expr(a, local_to_file, commons, ctx, out);
            }
        }
        ExprKind::Match { discriminant, arms } => {
            collect_refs_in_expr(discriminant, local_to_file, commons, ctx, out);
            for arm in arms {
                if let Pattern::Variant {
                    type_name: Some(tn),
                    ..
                } = &arm.pattern
                {
                    record_name_ref(&tn.name, local_to_file, ctx, out);
                }
                // #1800: a name used only in a guard needs its import too;
                // skipping the guard emitted a reference `tsc` cannot find.
                if let Some(guard) = &arm.guard {
                    collect_refs_in_expr(guard, local_to_file, commons, ctx, out);
                }
                match &arm.body {
                    MatchBody::Expr(e) => collect_refs_in_expr(e, local_to_file, commons, ctx, out),
                    MatchBody::Block(b) => {
                        collect_refs_in_block(b, local_to_file, commons, ctx, out)
                    }
                }
            }
        }
        ExprKind::Is { value, pattern } => {
            collect_refs_in_expr(value, local_to_file, commons, ctx, out);
            if let Pattern::Variant {
                type_name: Some(tn),
                ..
            } = pattern.as_ref()
            {
                record_name_ref(&tn.name, local_to_file, ctx, out);
            }
        }
    }
}

/// If `name` at `span` is a bare reference to a variant of a sum type (per
/// the checker's expression type), return the owning sum's name — the same
/// test the lowering uses to qualify it as `Type.Variant` (see the
/// `ExprKind::Ident` arm of `lower_expr_into`).
///
/// P6.56 (design/tracks/the-ir.md §6b): investigated routing variant
/// membership through a `TypedCommons`-only `TypeShape::Sum` lowering and
/// declined, not built — `lower_type_item_ir`'s own `TypeBody::Sum` arm
/// resolves every payload field's own `TyId` for every variant (an
/// `.unwrap_or_else(|| panic!(..))` on any resolution miss), just to answer
/// a name-membership question this call site can settle with a zero-cost,
/// infallible string comparison today. `positional_field_name` below has
/// the identical shape and the identical verdict, for the identical
/// reason.
fn sum_owner_of_variant(name: &str, id: ExprId, commons: &TypedCommons) -> Option<String> {
    if let Some(Ty::Named {
        kind: NamedKind::Sum,
        name: type_name,
        ..
    }) = commons.expr_ty(id).as_deref()
        && let Some(decl) = commons.types.get(type_name)
        && let TypeBody::Sum(s) = &decl.body
        && s.variants.iter().any(|v| v.name.name == name)
    {
        return Some(type_name.clone());
    }
    None
}

/// #1778: record every named type reachable in `ty`, the checked type of an
/// expression, as an *implied* import, which `settle_implied` keeps only if
/// the emitted body spells it. A name a consumed *context* declares is skipped:
/// the lowering never names one through a checked type unqualified, and under
/// Workers a context's value cannot be imported across its Worker boundary.
fn record_ty_refs(
    ty: TyId,
    local_to_file: &HashSet<String>,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    out: &mut ExternalReferences,
) {
    match &*commons.tys().get(ty) {
        Ty::Named { name, args, .. } => {
            if !is_consumed_context_type(ctx, name) {
                let mut found = ExternalReferences::default();
                record_name_ref(name, local_to_file, ctx, &mut found);
                for (unit, names) in found.by_commons {
                    out.implied_by_commons
                        .entry(unit)
                        .or_default()
                        .extend(names);
                }
                for (path, names) in found.by_sibling {
                    out.implied_by_sibling
                        .entry(path)
                        .or_default()
                        .extend(names);
                }
            }
            for a in args {
                record_ty_refs(*a, local_to_file, commons, ctx, out);
            }
        }
        Ty::Option(t)
        | Ty::Effect(t)
        | Ty::HttpResult(t)
        | Ty::HttpOutcome(t)
        | Ty::List(t)
        | Ty::Set(t)
        | Ty::Query(t)
        | Ty::Stream(t)
        | Ty::TestConnection(t)
        | Ty::Connection(t)
        | Ty::Actor(t) => record_ty_refs(*t, local_to_file, commons, ctx, out),
        Ty::Result(a, b) | Ty::Map(a, b) => {
            record_ty_refs(*a, local_to_file, commons, ctx, out);
            record_ty_refs(*b, local_to_file, commons, ctx, out);
        }
        Ty::Fn { params, ret } => {
            for p in params {
                record_ty_refs(*p, local_to_file, commons, ctx, out);
            }
            record_ty_refs(*ret, local_to_file, commons, ctx, out);
        }
        Ty::ActorSum(members) => {
            for (_, t) in members {
                record_ty_refs(*t, local_to_file, commons, ctx, out);
            }
        }
        Ty::Error
        | Ty::Never
        | Ty::Base(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::RequestQuery
        | Ty::RequestCookies
        | Ty::TokenClaims
        | Ty::ValidationError
        | Ty::DecodeError
        | Ty::Unit
        | Ty::Var(_) => {}
    }
}

fn record_name_ref(
    name: &str,
    local_to_file: &HashSet<String>,
    ctx: &EmitProjectCtx,
    out: &mut ExternalReferences,
) {
    if local_to_file.contains(name) {
        return;
    }
    // Imported from another commons?
    if let Some(commons_name) = ctx.imported_from.get(name) {
        out.by_commons
            .entry(commons_name.clone())
            .or_default()
            .insert(name.to_string());
        return;
    }
    // Sibling file in the same commons?
    if let Some(path) = ctx.file_decl_index.types.get(name)
        && path != &ctx.source_path
    {
        out.by_sibling
            .entry(path.clone())
            .or_default()
            .insert(name.to_string());
        return;
    }
    if let Some(path) = ctx.file_decl_index.fns.get(name)
        && path != &ctx.source_path
    {
        out.by_sibling
            .entry(path.clone())
            .or_default()
            .insert(name.to_string());
    }
}

/// Emit `import * as <ns> from "..."` for each consumed context that
/// exposes services (so the consuming file can reference its `__makeSurface`
/// return type and brand the cross-context call arguments).
/// #1478: real-node-internally already — returns one real `TsDecl::
/// ImportNamespace` per consumed context instead of writing into
/// `out: &mut String`.
fn emit_cross_context_namespace_imports(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    body: &[bynk_ts::TsStmt],
) -> Vec<bynk_ts::TsStmt> {
    let info = &ctx.cross_context;
    // Consumed contexts that expose services (v0.6) plus, v0.15, those whose
    // capabilities this context references via `given B.Cap`.
    let mut needed: std::collections::BTreeSet<String> = info
        .consumed_services
        .iter()
        .filter(|(_, svcs)| !svcs.is_empty())
        .map(|(q, _)| q.clone())
        .collect();
    needed.extend(cross_context_cap_namespaces(commons, ctx));
    // #1961: on workers, the owner of each event this context subscribes to.
    // Its payload codec (`emit_consumed_context_helpers`) names the event
    // through this alias, and a publisher with no callable service (only
    // `http` ones) was otherwise never imported: `TS2304` on the subscriber.
    if matches!(ctx.target, BuildTarget::Workers) {
        let subscribed: HashSet<String> = commons
            .commons
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Service(s) => {
                    let bynk_model::ProtocolIr::Events { event, .. } =
                        bynk_model::lower_protocol_ir_from_commons(&s.protocol, commons)
                    else {
                        return None;
                    };
                    match &*commons.tys().get(event) {
                        Ty::Named { name, .. } => Some(name.clone()),
                        _ => None,
                    }
                }
                _ => None,
            })
            .collect();
        needed.extend(
            info.consumed_event_names
                .iter()
                .filter(|(_, names)| names.iter().any(|n| subscribed.contains(n)))
                .map(|(c, _)| c.clone()),
        );
    }
    // #2058: a consumed context the body names a type of through its alias
    // (`(1 as ev_store.Code)`, a literal admitted to its refined type), even
    // one with no service this context calls.
    // Only a refined type an expression here is checked as can be cast to,
    // and only one the body then spells through the alias is imported.
    let type_ns = crate::emitter::emit::consumed_type_ns(ctx);
    let named: HashSet<String> = commons
        .expr_types
        .values()
        .filter_map(|te| match &*commons.tys().get(te.ty) {
            Ty::Named {
                name,
                kind: NamedKind::Refined(_),
                ..
            } => type_ns.get(name).cloned(),
            _ => None,
        })
        .collect();
    if !named.is_empty() {
        let spelled = body_words(body);
        needed.extend(
            info.consumed_contexts
                .iter()
                .filter(|c| {
                    let ns = qualified_to_ns(c);
                    named.contains(&ns) && spelled.contains(&ns)
                })
                .cloned(),
        );
    }
    // #2073: on workers, any consumed context whose alias this module's codecs
    // spell, even one with no service this context calls: the declaring
    // context of a type a borrowed signature type reaches (`shop_base.Base` in
    // `shop.order`'s `Order`), or of a field of a type on this context's own
    // boundary (#2044's codecs, `shop_base.Base` for `Order.b`).
    if matches!(ctx.target, BuildTarget::Workers) {
        let spelled = body_words(body);
        needed.extend(
            info.consumed_contexts
                .iter()
                .filter(|c| {
                    !ctx.consumed_adapters.contains(c.as_str())
                        && spelled.contains(&qualified_to_ns(c))
                })
                .cloned(),
        );
    }
    if needed.is_empty() {
        return Vec::new();
    }
    let mut stmts = Vec::new();
    let consumed_with_services: Vec<&String> = needed.iter().collect();
    for q in &consumed_with_services {
        // Pick the first known file path for the consumed context as the
        // import target. (The composition root lives in the consumed
        // context's directory; any of its files would work as an import
        // target since they're all in the same module namespace, but we
        // currently emit one file per .bynk source so a single import per
        // consumed name suffices for the surface contract.)
        let target_paths = ctx.imported_decl_paths.get(q.as_str());
        let target = target_paths
            .and_then(|m| m.values().next().cloned())
            .unwrap_or_else(|| {
                // No imported declaration pins the path (e.g. a capability-only
                // consumed context, v0.15). Fall back to the unit's own module:
                // its per-Worker handlers in workers mode, or its <segment>.bynk
                // source in bundle mode. v0.17: a consumed *adapter* is not a
                // Worker — its capability types live in its root module
                // (`<adapter>.ts`) in both targets.
                if ctx.consumed_adapters.contains(q.as_str()) {
                    let mut p = EmitProjectCtx::commons_path(q);
                    p.set_extension("bynk");
                    p
                } else {
                    match ctx.target {
                        BuildTarget::Workers => ctx.worker_layout.handlers_source_path(q),
                        BuildTarget::Bundle => {
                            let mut p = EmitProjectCtx::commons_path(q);
                            p.set_extension("bynk");
                            p
                        }
                    }
                }
            });
        let import =
            cross_commons_import_specifier_for_path(&ctx.source_path, &target, ctx.import_ext);
        let ns = qualified_to_ns(q);
        // #661: under `workers`, a consumed *context*'s module is imported for
        // its **types only** — the caller now generates its own codecs
        // (`emit_boundary_helpers`) and reaches the callee's types through this
        // alias in type position (`deps: { Clock: platform_time.Clock }`,
        // `Result<commerce_payment.AuthId, …>`). An `import type` is erased
        // outright, so the callee's *module* — and its provider implementation —
        // never enters the caller's Worker bundle. This does **not** apply to a
        // consumed *adapter* (its binding namespace, e.g. `tokens`, is a real
        // value import used by `compose.ts`) nor on `bundle` (contexts compile
        // together, and the value uses in `compose.ts` are legitimate).
        let type_only = matches!(ctx.target, BuildTarget::Workers)
            && !ctx.consumed_adapters.contains(q.as_str());
        // Arc C, slice 30 (#1392): a real `TsDecl::ImportNamespace` —
        // `type_only` (#1392's own new field) covers the `import type * as`
        // form.
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::ImportNamespace {
                type_only,
                alias: ns,
                from: import,
            },
            None,
        ));
    }
    // #1486: no explicit trailing blank stmt here — the printer's automatic
    // top-level spacing policy supplies the single blank before whatever
    // follows (never import-kind at this function's own call sites, so
    // never exempted from it). The boundary this function's own *first*
    // statement forms with `emit_project_imports`'s own last statement
    // (both import-kind, genuinely wanting a blank despite the "no blank
    // between adjacent imports" exemption) is handled at `emit_project`'s
    // own call site instead — the only place that knows both neighbours.
    stmts
}

/// #1478: real-node-internally already — returns one real `TsDecl::Import`
/// per sibling/cross-unit import group instead of writing into
/// `out: &mut String`.
fn emit_project_imports(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    refs: &ExternalReferences,
) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    // Events track, slice 0 (spine #936): the bare event-type names this
    // context's own `from Events(E)` service headers name — see the
    // Workers type-only-import narrowing below.
    // P6.24a/P6.19: reads the protocol's own resolved `ProtocolIr::Events`
    // instead of matching `ServiceProtocol::Events { event_type:
    // TypeRef::Named(id), .. }` directly — a resolve miss (`lower_protocol_
    // ir_from_commons` degrades to `Ty::Unit` on one, never panics) is
    // indistinguishable from a legitimately non-`Named` header, so falls
    // through to `None` exactly like the raw match's own `_ => None` arm
    // did, not a new failure mode.
    let subscribed_event_type_names: HashSet<String> = commons
        .commons
        .items
        .iter()
        .filter_map(|item| match item {
            CommonsItem::Service(s) => {
                let bynk_model::ProtocolIr::Events { event, .. } =
                    bynk_model::lower_protocol_ir_from_commons(&s.protocol, commons)
                else {
                    return None;
                };
                match &*commons.tys().get(event) {
                    Ty::Named { name, .. } => Some(name.clone()),
                    _ => None,
                }
            }
            _ => None,
        })
        .collect();
    // Sibling imports: relative path within the same commons/context directory.
    let mut sibling_paths: Vec<(&PathBuf, &HashSet<String>)> = refs.by_sibling.iter().collect();
    sibling_paths.sort_by(|a, b| a.0.cmp(b.0));
    for (path, names) in sibling_paths {
        let import = sibling_import_specifier(&ctx.source_path, path, ctx.import_ext);
        let mut sorted: Vec<&String> = names.iter().collect();
        sorted.sort();
        // Arc C, slice 30 (#1392): a real `TsDecl::Import`.
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Import {
                type_only: false,
                names: sorted.iter().map(|s| ts_ident(s)).collect(),
                from: import,
            },
            None,
        ));
    }
    let mut type_paths: Vec<(&PathBuf, &HashSet<String>)> = refs.types_by_sibling.iter().collect();
    type_paths.sort_by(|a, b| a.0.cmp(b.0));
    for (path, names) in type_paths {
        let mut sorted: Vec<&String> = names.iter().collect();
        sorted.sort();
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Import {
                type_only: true,
                names: sorted.iter().map(|s| ts_ident(s)).collect(),
                from: sibling_import_specifier(&ctx.source_path, path, ctx.import_ext),
            },
            None,
        ));
    }
    // Cross-unit imports: group by *target file path*.
    let mut unit_names: Vec<(&String, &HashSet<String>)> = refs.by_commons.iter().collect();
    unit_names.sort_by(|a, b| a.0.cmp(b.0));
    for (unit_name, names) in unit_names {
        let target_paths = ctx.imported_decl_paths.get(unit_name.as_str());
        let mut by_target: std::collections::BTreeMap<PathBuf, Vec<&String>> =
            std::collections::BTreeMap::new();
        for n in names {
            let path = target_paths
                .and_then(|p| p.get(n))
                .cloned()
                .unwrap_or_else(|| EmitProjectCtx::commons_path(unit_name));
            by_target.entry(path).or_default().push(n);
        }
        for (target, mut name_list) in by_target {
            name_list.sort();
            let import =
                cross_commons_import_specifier_for_path(&ctx.source_path, &target, ctx.import_ext);
            // For context units, aliase commons-source imports so we can emit
            // rebrand aliases of the same short name. Imports from consumed
            // contexts keep their original name. v0.20b: the rebrand applies
            // to *types* only — a `uses`-imported function (bynk.list's
            // `traverse`) is a value, imports plainly, and is never branded.
            let mut parts: Vec<String> = Vec::new();
            for n in &name_list {
                let is_subscribed_event_type = ctx.target == BuildTarget::Workers
                    && subscribed_event_type_names.contains(n.as_str());
                // #2045: under Workers, a consumed *context*'s type is named in
                // type position only — its codecs are the caller's own
                // (`emit_consumed_context_helpers`) — so it imports type-only and
                // the callee's module never enters the caller's Worker bundle
                // (#661). A consumed *adapter* is linked into each Worker, and
                // its values (`Method.Get`) are real imports.
                let is_consumed_context_type = ctx.target == BuildTarget::Workers
                    && ctx.unit_kind == UnitKind::Context
                    && !ctx.consumed_adapters.contains(unit_name.as_str())
                    && ctx.imported_from_kind.get(n.as_str()) == Some(&UnitKind::Context)
                    && !ctx.consumed_types_with_methods.contains(n.as_str());
                // R4.10/R8.2: the same shared `is_uses_commons_type` predicate
                // `emit_context_rebrands` below reads — this import-aliasing
                // site is that function's own step 1 ("Done in imports", its
                // own doc comment), and the two must agree exactly: an alias
                // narrower than the rebrand leaves an undefined name in the
                // generated import; a rebrand narrower than the alias leaves
                // an alias imported and never used. `ctx.unit_kind ==
                // UnitKind::Context` is the same guard as
                // `emit_context_rebrands`'s own `ctx.owning_context.is_some()`
                // early return (`owning_context` is `Some` exactly when
                // `unit_kind == Context`, `project.rs`'s own construction) —
                // kept explicit here since this loop runs for every unit
                // kind, not just contexts.
                if ctx.unit_kind == UnitKind::Context
                    && bynk_check::resolver::compute_is_uses_commons_type(
                        &ctx.imported_from_kind,
                        &commons.types,
                        n,
                    )
                {
                    parts.push(format!("{n} as __Commons{n}"));
                } else if is_subscribed_event_type || is_consumed_context_type {
                    // Events track, slice 0 (spine #936): under Workers, a
                    // context deploys as its own separate Worker script —
                    // there is no shared module graph to import a peer
                    // context's *value* across (the #661 hazard this
                    // mirrors: a caller generates its own codec rather than
                    // importing the callee's runtime code). `from
                    // Events(E)`'s `E` is the one plain named type crossing
                    // a context boundary directly by name (every other
                    // cross-context reference goes through a generated
                    // Service-Binding codec instead) — used only in type
                    // position (`e: E`), so this specific name is type-only.
                    // Narrowly scoped to event types specifically, not every
                    // cross-context import: a `type`/`enum` crossing via
                    // `uses`/`consumes` (e.g. `bynk`'s `Method`) is often
                    // used as a *value* too (`Method.Get`), which a blanket
                    // `import type` would wrongly break.
                    parts.push(format!("type {}", import_spec_of(ctx, n)));
                } else {
                    parts.push(import_spec_of(ctx, n));
                }
            }
            // Arc C, slice 30 (#1392): a real `TsDecl::Import` — `parts`'
            // own per-name `as __CommonsX`/`type X` prefixes already match
            // `names`'s own documented "raw text slot" convention, no new
            // gap.
            stmts.push(bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::Import {
                    type_only: false,
                    names: parts,
                    from: import,
                },
                None,
            ));
        }
    }
    // #527: imports the DO-side agent-deps expressions need (binding modules,
    // other Workers' handlers). Precomputed by the project driver — already
    // formed statement text, stays opaque `TsStmt::raw`, the established
    // "pre-formatted pass-through" pattern.
    for line in &ctx.extra_import_lines {
        stmts.push(bynk_ts::TsStmt::raw(format!("{line}\n"), None));
    }
    // #1486: every statement in this function's own return, whatever
    // section it came from (sibling imports, cross-unit imports, the
    // rebrand-aliasing pair, `extra_import_lines`' own pre-formatted `Raw`
    // text), sits directly under its predecessor with no blank between
    // them — this function never inserted one under the pre-#1486
    // flat-print regime (confirmed: no explicit blank `TsStmt` anywhere in
    // its own body), so every internal adjacency needs forcing regardless
    // of `TsStmtKind` (a `Raw` import line next to a real `ImportNamespace`
    // is exactly the case the printer's own kind-based "both imports"
    // exemption can't see). The very first statement is left alone — its
    // own blank-or-not relative to whatever precedes this function's call
    // site is `emit_project`'s own call-site decision (the header/`emit_
    // project_imports` boundary, and the project/cross-context import
    // boundary, both forced explicitly there instead).
    for stmt in stmts.iter_mut().skip(1) {
        stmt.no_blank_before = true;
    }
    stmts
}

/// #2887: `n` as an import specifier. A type this unit renames with `consumes
/// U { T as Alias }` is imported under the name `U` declares and bound to the
/// alias (`Digest as CryptoDigest`), so every use in the module reaches the
/// module's real type, not a new one.
fn import_spec_of(ctx: &EmitProjectCtx, n: &str) -> String {
    match ctx.consumed_type_aliases.get(n) {
        Some(original) => format!("{} as {}", ts_ident(original), ts_ident(n)),
        None => ts_ident(n),
    }
}

/// #2887: a codec helper of type `n` (`prefix` is `__serialise_` or
/// `__deserialise_`) as an import specifier: for a type this unit renames, the
/// helper the module exports for the real type, bound under the alias's name.
fn codec_import_spec(ctx: &EmitProjectCtx, prefix: &str, n: &str) -> String {
    match ctx.consumed_type_aliases.get(n) {
        Some(original) => format!("{prefix}{original} as {prefix}{n}"),
        None => format!("{prefix}{n}"),
    }
}

/// Compute a relative import specifier from `from_source` (a `.bynk` path)
/// to `to_source` (another `.bynk` path), with `.bynk` rewritten to `.js`
/// for compatibility with NodeNext/strict TS resolution.
fn sibling_import_specifier(from_source: &Path, to_source: &Path, ext: ImportExt) -> String {
    let from_dir = from_source.parent().unwrap_or(Path::new(""));
    let target = to_source.with_extension(ext.as_str());
    let rel = relative_to(from_dir, &target);
    format!("./{}", ts_specifier(&rel))
}

/// Render a path as a TypeScript module specifier: **always forward
/// slashes**. `Path::display()` uses the platform separator, and on Windows
/// that emitted `import ... from "./commerce\orders.js"` — broken ESM
/// output, caught by the first CI matrix run on windows-latest.
pub fn ts_specifier(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Compute a relative import specifier from this file's location to a
/// specific source file in another commons. `target_source` is the project-
/// relative path of the target `.bynk` file. The result is suitable for
/// `import { ... } from "..."` in NodeNext/strict TypeScript.
pub fn cross_commons_import_specifier_for_path(
    from_source: &Path,
    target_source: &Path,
    ext: ImportExt,
) -> String {
    let from_dir = from_source.parent().unwrap_or(Path::new(""));
    let target = target_source.with_extension(ext.as_str());
    let rel = relative_to(from_dir, &target);
    let display = ts_specifier(&rel);
    if display.starts_with("../") || display.starts_with("./") {
        display
    } else {
        format!("./{display}")
    }
}

/// Compute `target` as a path relative to `from`. Handles parent traversal
/// (`..`) for cases where `target` lives in a sibling directory.
fn relative_to(from: &Path, target: &Path) -> PathBuf {
    use std::path::Component as C;
    let f_comps: Vec<C> = from.components().collect();
    let t_comps: Vec<C> = target.components().collect();
    let mut shared = 0;
    while shared < f_comps.len() && shared < t_comps.len() && f_comps[shared] == t_comps[shared] {
        shared += 1;
    }
    let mut out = PathBuf::new();
    for _ in shared..f_comps.len() {
        out.push("..");
    }
    for c in &t_comps[shared..] {
        out.push(c.as_os_str());
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

/// #1478: real-node-internally already — returns its own real statements
/// (the two-comment banner, plus a conditional runtime-import declaration)
/// instead of writing into `out: &mut String`.
/// The runtime names a module's cookie helpers import: the `RequestCookies` view (#3277)
/// and the `SignedCookie` session-cookie issuer and deleter (#3386).
fn cookie_runtime_imports(ru: &RuntimeUse, parts: &mut Vec<&str>) {
    for (used, imports) in [
        (ru.request_cookies(), REQUEST_COOKIES_RUNTIME_IMPORTS),
        (ru.session_cookie(), SESSION_COOKIE_RUNTIME_IMPORTS),
        (ru.session_signing(), SESSION_SIGNING_RUNTIME_IMPORTS),
    ] {
        if used {
            parts.extend(imports.trim_start_matches(", ").split(", "));
        }
    }
}

fn write_header(commons: &TypedCommons, ctx: &EmitProjectCtx) -> Vec<bynk_ts::TsStmt> {
    // Arc C, slice 30 (#1392): the 2-line banner converts to two real
    // `TsStmt::Comment` statements, the same `events_fanout.rs`-established
    // precedent (#1317) every other Arc C header already uses.
    let kind = match ctx.unit_kind {
        UnitKind::Commons => "commons",
        UnitKind::Context => "context",
        UnitKind::Test => "test",
        UnitKind::Integration => "integration test",
        UnitKind::Adapter => "adapter",
    };
    // #1486: no explicit blank stmt after the banner — the printer's
    // automatic top-level spacing policy already supplies the single blank
    // after it (`Comment`→`Comment` is exempted, matching the banner's own
    // two adjacent lines; the automatic policy is not exempted for whatever
    // follows the banner, so the wanted blank appears there for free).
    let mut stmts = vec![
        bynk_ts::TsStmt::comment("Generated by bynk — do not edit by hand.", None),
        bynk_ts::TsStmt::comment(format!("{kind} {}", commons.commons.name.joined()), None),
    ];
    if !commons.commons.items.is_empty() {
        let runtime_import = runtime_import_for(&ctx.source_path, ctx.import_ext);
        let has_agent = commons
            .commons
            .items
            .iter()
            .any(|i| matches!(i, CommonsItem::Agent(_)));
        // v0.80: a file with any agent invariant imports the `invariantViolation`
        // fault helper used by the generated `commitState` gate. v0.116: a step
        // invariant (`transition`) uses the same fault helper, so a transition-only
        // agent must import it too.
        let has_agent_invariants = commons.commons.items.iter().any(|i| match i {
            CommonsItem::Agent(a) => !a.invariants.is_empty() || !a.transitions.is_empty(),
            _ => false,
        });
        // v0.153 (ADR 0177): a service HTTP handler imports `HttpResult`, and so
        // does any *free* `fn` / provider / capability whose signature names it
        // (the `?`-Option lift makes a bare `fn -> HttpResult[T]` emit
        // `HttpResult.NotFound`) — the structural scan covers both, closing the
        // free-fn gap the single-file path already handles.
        // P6.24a/P6.19: `h.kind`/`s.protocol` read through the IR-native
        // `IrHandlerKind`/`ProtocolIr` readers below — pure syntax, no body
        // lowering, safe regardless of the still-open `Question`/`Is` gap
        // (design/tracks/the-ir.md's own P6.3 correction).
        let has_http = commons.commons.items.iter().any(|i| match i {
            CommonsItem::Service(s) => s.handlers.iter().any(|h| {
                matches!(
                    bynk_model::lower_handler_kind_ir(&h.kind),
                    bynk_model::IrHandlerKind::Http { .. }
                )
            }),
            _ => false,
        }) || file_mentions_http_result(commons);
        // A `from queue` `on message` is the queue consumer (imports `QueueResult`);
        // a `from websocket` `on message` (slice 3b-iii) is the inbound handler and
        // is not a queue concern. #1319: a bare field/type-declaration mention
        // (the `ts_any` residual's own real-type-cast sites) is a second, real
        // way to need the import with no consumer handler at all — closed via
        // `file_mentions_queue_result`, the same structural-scan pattern
        // `file_mentions_http_result` already uses for its own type.
        let has_queue = commons.commons.items.iter().any(|i| match i {
            CommonsItem::Service(s) => {
                !matches!(
                    bynk_model::lower_protocol_ir_from_commons(&s.protocol, commons),
                    bynk_model::ProtocolIr::WebSocket { .. }
                ) && s.handlers.iter().any(|h| {
                    matches!(
                        bynk_model::lower_handler_kind_ir(&h.kind),
                        bynk_model::IrHandlerKind::Message
                    )
                })
            }
            _ => false,
        }) || file_mentions_queue_result(commons);
        let workers = matches!(ctx.target, BuildTarget::Workers);
        let mut parts: Vec<&str> = vec![
            "Ok",
            "Err",
            "Some",
            "None",
            "type Result",
            "type Option",
            "type ValidationError",
        ];
        // v0.22b: the codec types are imported only when the file uses the
        // `Json` codec (or names `DecodeError` in a signature) — keeping every
        // non-codec module's header byte-identical to v0.22a.
        let uses_codec = uses_json_codec(commons);
        let mentions_json_error = file_mentions_json_error(commons);
        if uses_codec || mentions_json_error {
            parts.push("type DecodeError");
        }
        // v0.102: a file naming `Connection[F]` imports the runtime interface.
        if file_mentions_connection(commons) {
            parts.push("type Connection");
        }
        if has_agent {
            // v0.9.2: agent-declaring files lower instantiation through the
            // `makeAgent` helper and a per-agent `StateRegistry`, and the
            // generated factory's signature names `DurableObjectNamespace`.
            parts.push("type __DurableObjectState");
            parts.push("type __DurableObjectNamespace");
            parts.push("__StateRegistry");
            parts.push("__makeAgent");
            parts.extend(inspect_agent_imports(workers && ctx.inspect_agents));
        } else if workers
            && commons
                .callees
                .values()
                .any(|c| matches!(c, bynk_check::checker::Callee::AgentInit(_)))
        {
            // #1820: a file of a split context that constructs only a
            // sibling file's agents passes `env` to its factory, so its
            // handlers' `deps.env` names the agents' namespaces too.
            parts.push("type __DurableObjectNamespace");
        }
        if has_agent_invariants {
            parts.push("__invariantViolation");
        }
        // #1678: on `workers` an agent's calls cross its Durable Object's
        // `fetch` through a wire table read by both ends.
        if workers && has_agent {
            parts.push("type __AgentWire");
            parts.push("__decodeAgentArgs");
            parts.push("__encodeAgentResult");
            // #1818: an agent that reads its own key decodes it on the DO side.
            let reads_key = |a: &AgentDecl| crate::emitter::emit::agent_reads_key(a);
            if commons
                .commons
                .items
                .iter()
                .any(|i| matches!(i, CommonsItem::Agent(a) if reads_key(a)))
            {
                parts.push("__decodeAgentKey");
            }
            let passes_through = commons.commons.items.iter().any(|i| match i {
                CommonsItem::Agent(a) => {
                    (reads_key(a) && serialisation::agent_wire_passes_through(&a.key_type))
                        || a.handlers.iter().any(|h| {
                            serialisation::agent_wire_passes_through(&h.return_type)
                                || h.params
                                    .iter()
                                    .any(|p| serialisation::agent_wire_passes_through(&p.type_ref))
                        })
                }
                _ => false,
            });
            if passes_through {
                parts.push("__AGENT_WIRE_PASS");
            }
        }
        // Events track, slice 0 (spine #936): an agent whose own handler body
        // emits directly needs its Workers-mode DO fetch dispatcher to
        // rebuild `deps.__eventsDispatch` from `env.EVENTS_FANOUT` (mirrors
        // the `#527` `given`-provider rebuild — see `emit_agent`) — a
        // function does not survive the JSON wire any better than a
        // provider does.
        let has_agent_uses_emit = workers
            && commons.commons.items.iter().any(|i| match i {
                CommonsItem::Agent(a) => a
                    .handlers
                    .iter()
                    .any(|h| block_uses_emit(&h.body, &commons.callees)),
                _ => false,
            });
        if has_agent_uses_emit {
            parts.push("__dispatchToEventsFanout");
        }
        // v0.96 (ADR 0124): an agent whose load-time validation gate fires imports
        // the `rehydrationViolation` fault helper.
        let has_rehydration_gate = commons.commons.items.iter().any(|i| match i {
            CommonsItem::Agent(a) => emit::agent_needs_rehydrate(a, &commons.types),
            _ => false,
        });
        // State-migrations S2 (#2224): a `migrate` step decodes its old shape
        // (a failure is a `RehydrationViolation`) and turns a throw into
        // `MigrationFailed`.
        let has_migration_step = commons.commons.items.iter().any(|i| match i {
            CommonsItem::Agent(a) => ctx
                .agent_schema_versions
                .get(&a.name.name)
                .is_some_and(|v| !v.steps.is_empty()),
            _ => false,
        });
        if has_rehydration_gate || has_migration_step {
            parts.push("__rehydrationViolation");
        }
        if has_migration_step {
            parts.push("__migrationFailed");
        }
        // State-migrations S1 (#2211): every agent reports stored keys its
        // definition no longer declares; an agent built with the schema lock
        // also checks its stored `__v` marker (Decision A: none without one).
        if has_agent {
            parts.push("__reportUnknownStoredKeys");
            // Per-entry storage slice 0 (#3189): faults on a `__layout` marker
            // a later build wrote.
            parts.push("__checkStoredLayout");
            // Per-entry storage (#3189): the codec between the stored record
            // and the wholesale-shaped state a handler reads.
            parts.push("__readStored");
            parts.push("__commitStored");
            // Cache reaping (#3189): a `Cache` op notes the clock it read.
            parts.push("__noteClock");
        }
        // Ordered store collections (#3254): the handle classes an agent's state is made
        // of, and the helpers that put them in wire form and fill them back.
        let mut state_names: Vec<&'static str> = commons
            .commons
            .items
            .iter()
            .filter_map(|i| match i {
                CommonsItem::Agent(a) => Some(emit::agent_state_runtime_names(a, &commons.types)),
                _ => None,
            })
            .flatten()
            .collect();
        state_names.sort_unstable();
        state_names.dedup();
        parts.extend(state_names);
        let has_versioned_agent = commons.commons.items.iter().any(|i| match i {
            CommonsItem::Agent(a) => ctx.agent_schema_versions.contains_key(&a.name.name),
            _ => false,
        });
        if has_versioned_agent {
            parts.push("__checkStoredSchemaVersion");
        }
        // State-migrations S4 (#2259 Decision D): on Workers, a versioned
        // agent's Durable Object answers `bynk migrate sweep`'s wake.
        if workers && has_versioned_agent {
            parts.push("__wakeForMigration");
        }
        // v0.104/v0.105 (real-time track slice 3b): on Workers a `store Map[K,
        // Connection]` persists the connection id; its entry ops re-resolve the live
        // socket via `resolveConnection` and read a connection's id via `connIdOf`.
        if workers
            && commons.commons.items.iter().any(|i| match i {
                CommonsItem::Agent(a) => emit::agent_has_held_storage(a),
                _ => false,
            })
        {
            parts.push("__resolveConnection");
            parts.push("__connIdOf");
        }
        // v0.104/v0.105 (real-time track slice 3b): on Workers a context hosting a
        // `from websocket` `on open` accepts the socket inside its Durable Object via
        // the hibernatable API — `acceptHibernatableConnection` (accept + tag + wrap),
        // a `WebSocketPair`, and the `101` upgrade response. (The service and its
        // hosting agent share the one Worker module, so these land in one
        // `handlers.ts`.)
        let hosts_ws_open = commons.commons.items.iter().any(|i| match i {
            CommonsItem::Service(s) => s.handlers.iter().any(|h| {
                matches!(
                    bynk_model::lower_handler_kind_ir(&h.kind),
                    bynk_model::IrHandlerKind::Open
                )
            }),
            _ => false,
        });
        if workers && hosts_ws_open {
            parts.push("__acceptHibernatableConnection");
            parts.push("__newWebSocketPair");
            parts.push("__webSocketUpgradeResponse");
        }
        // v0.106 (slice 3b-iii): a context with an inbound/close handler re-wraps
        // the firing socket as a `WorkersConnection` in `webSocketMessage`/
        // `webSocketClose`.
        let hosts_ws_inbound = commons.commons.items.iter().any(|i| match i {
            CommonsItem::Service(s) => {
                matches!(
                    bynk_model::lower_protocol_ir_from_commons(&s.protocol, commons),
                    bynk_model::ProtocolIr::WebSocket { .. }
                ) && s.handlers.iter().any(|h| {
                    matches!(
                        bynk_model::lower_handler_kind_ir(&h.kind),
                        bynk_model::IrHandlerKind::Message | bynk_model::IrHandlerKind::Close
                    )
                })
            }
            _ => false,
        });
        if workers && hosts_ws_inbound {
            parts.push("__WorkersConnection");
        }
        if has_http {
            // `HttpResult` is both a value (the constructor namespace) and a
            // type (the discriminated union). A bare named import brings both
            // in — `type HttpResult` would duplicate the identifier.
            parts.push(HTTP_RESULT);
        }
        if has_queue {
            // v0.44: `QueueResult` is both a value (the verdict namespace) and a
            // type; a bare named import brings both in.
            parts.push(QUEUE_RESULT);
        }
        if workers {
            parts.push("type __JsonValue");
            parts.push("type __BoundaryError");
            parts.push("type __ServiceBinding");
            parts.push("__callService");
            parts.push("__boundaryError");
        } else if uses_codec || has_agent {
            // v0.22b: the bundle-mode codec helpers reference JsonValue and
            // BoundaryError. v0.96 (ADR 0124): so do an agent's emitted
            // rehydration deserialisers and the gate's inline base checks — the
            // boundary helpers now emit on bundle too (for the rehydration gate).
            parts.push("type __JsonValue");
            parts.push("type __BoundaryError");
        }
        // #1476: `bytes()`/`icu()` fold in here directly now, in the same order
        // the old post-print `inject_runtime_imports` pass appended them (bytes
        // first, then icu) — `ctx.runtime_use` is already fully populated by the
        // time `write_header` runs (moved to the end of `emit_project`), so this
        // needs no post-print text surgery at all, just reading the same two
        // flags one function call later than before.
        //
        // Review of #1490: this drops `inject_runtime_imports`'s own
        // `missing_bindings` dedup (matching a `type Foo` group binding against
        // an already-bound bare `Foo` and vice versa) — safe only because
        // neither `BYTES_RUNTIME_IMPORTS`'s nor `MESSAGES_RUNTIME_IMPORTS`'s own
        // names overlap anything `parts` already carries above (confirmed by
        // direct inspection, not assumed). A future group that *does* overlap
        // needs that filter reinstated here, not just appended unconditionally
        // — `missing_bindings`'s own doc named exactly this case (the
        // test-scaffold module's own `Ok`/`Err` overlap) before this issue
        // deleted the only place that reasoning was written down.
        if ctx.runtime_use.bytes() {
            parts.extend(BYTES_RUNTIME_IMPORTS.trim_start_matches(", ").split(", "));
        }
        if ctx.runtime_use.bytes_ops() {
            parts.extend(
                BYTES_OPS_RUNTIME_IMPORTS
                    .trim_start_matches(", ")
                    .split(", "),
            );
        }
        let time = time_runtime_imports(&ctx.runtime_use);
        parts.extend(
            time.trim_start_matches(", ")
                .split(", ")
                .filter(|p| !p.is_empty()),
        );
        if ctx.runtime_use.iso_date() {
            parts.extend(
                ISO_DATE_RUNTIME_IMPORTS
                    .trim_start_matches(", ")
                    .split(", "),
            );
        }
        if ctx.runtime_use.yaml() {
            parts.extend(YAML_RUNTIME_IMPORTS.trim_start_matches(", ").split(", "));
        }
        if ctx.runtime_use.settle_all() {
            parts.extend(
                SETTLE_ALL_RUNTIME_IMPORTS
                    .trim_start_matches(", ")
                    .split(", "),
            );
        }
        let effects = effect_runtime_imports(&ctx.runtime_use);
        if !effects.is_empty() {
            parts.extend(effects.trim_start_matches(", ").split(", "));
        }
        if ctx.runtime_use.fault() {
            parts.extend(FAULT_RUNTIME_IMPORTS.trim_start_matches(", ").split(", "));
        }
        parts.extend(trace_step_imports(&ctx.runtime_use));
        if ctx.runtime_use.url_ops() {
            parts.extend(URL_OPS_RUNTIME_IMPORTS.trim_start_matches(", ").split(", "));
        }
        if ctx.runtime_use.request_headers() {
            parts.extend(
                REQUEST_HEADERS_RUNTIME_IMPORTS
                    .trim_start_matches(", ")
                    .split(", "),
            );
        }
        cookie_runtime_imports(&ctx.runtime_use, &mut parts);
        if ctx.runtime_use.request_query() {
            parts.extend(
                REQUEST_QUERY_RUNTIME_IMPORTS
                    .trim_start_matches(", ")
                    .split(", "),
            );
        }
        if ctx.runtime_use.query_string() {
            parts.extend(
                QUERY_STRING_RUNTIME_IMPORTS
                    .trim_start_matches(", ")
                    .split(", "),
            );
        }
        if ctx.runtime_use.form_codec() {
            parts.extend(
                FORM_CODEC_RUNTIME_IMPORTS
                    .trim_start_matches(", ")
                    .split(", "),
            );
        }
        if ctx.runtime_use.cookie_string() {
            parts.extend(
                COOKIE_STRING_RUNTIME_IMPORTS
                    .trim_start_matches(", ")
                    .split(", "),
            );
        }
        if ctx.runtime_use.multipart_form() {
            parts.extend(
                MULTIPART_FORM_RUNTIME_IMPORTS
                    .trim_start_matches(", ")
                    .split(", "),
            );
        }
        if ctx.runtime_use.eq() {
            parts.extend(EQ_RUNTIME_IMPORTS.trim_start_matches(", ").split(", "));
        }
        if ctx.runtime_use.int() {
            parts.extend(INT_RUNTIME_IMPORTS.trim_start_matches(", ").split(", "));
        }
        if ctx.runtime_use.icu() {
            parts.extend(
                MESSAGES_RUNTIME_IMPORTS
                    .trim_start_matches(", ")
                    .split(", "),
            );
        }
        let mut names: Vec<String> = parts.into_iter().map(str::to_string).collect();
        names.extend(
            ctx.runtime_use
                .cmp_imports()
                .split(", ")
                .filter(|s| !s.is_empty())
                .map(str::to_string),
        );
        stmts.extend(runtime_files::runtime_import_stmts(
            false,
            &names,
            &runtime_import,
        ));
        // #1486: no explicit trailing blank stmt here — the printer's
        // automatic top-level spacing policy supplies the single blank
        // before whatever follows the header (never import-kind at this
        // function's own call site, so never exempted from it).
    }
    stmts
}

/// Variant of write_header for single-file (no project context) emission.
fn write_header_single(
    out: &mut String,
    commons: &TypedCommons,
    runtime_use: &RuntimeUse,
    uses_http: bool,
    uses_queue: bool,
) {
    // Arc C, slice 30 (#1392): the same 2-comment-statement conversion
    // `write_header` above already established.
    out.push_str(&bynk_ts::print_stmt(
        &bynk_ts::TsStmt::comment("Generated by bynk — do not edit by hand.", None),
        0,
    ));
    out.push_str(&bynk_ts::print_stmt(
        &bynk_ts::TsStmt::comment(
            format!(
                "commons {}",
                bynk_project::display_unit_name(&commons.commons.name.joined())
            ),
            None,
        ),
        0,
    ));
    writeln!(out).unwrap();
    if !commons.commons.items.is_empty() {
        // v0.22b: codec imports only when the file uses the `Json` codec.
        let uses_codec = uses_json_codec(commons);
        let codec_imports = if uses_codec {
            ", type DecodeError, type __JsonValue, type __BoundaryError"
        } else if file_mentions_json_error(commons) {
            ", type DecodeError"
        } else {
            ""
        };
        // v0.110 (ADR 0142): the `Bytes` runtime helpers, imported only when a
        // `Bytes` value is constructed or compared in the body.
        let bytes_imports = if runtime_use.bytes() {
            BYTES_RUNTIME_IMPORTS
        } else {
            ""
        };
        // #2016: the `Bytes` operation helpers, imported only when called.
        let bytes_ops_imports = if runtime_use.bytes_ops() {
            BYTES_OPS_RUNTIME_IMPORTS
        } else {
            ""
        };
        // K3 (#2592): the `Instant` ISO-8601 helpers, imported only when called.
        let time_ops_imports = time_runtime_imports(runtime_use);
        // K2 (#2594): the percent-encoding helpers, imported only when called.
        let iso_date_imports = if runtime_use.iso_date() {
            ISO_DATE_RUNTIME_IMPORTS
        } else {
            ""
        };
        // #3214: the YAML parser, imported only by a module that calls `Yaml.decode`.
        let yaml_imports = if runtime_use.yaml() {
            YAML_RUNTIME_IMPORTS
        } else {
            ""
        };
        let url_ops_imports = if runtime_use.url_ops() {
            URL_OPS_RUNTIME_IMPORTS
        } else {
            ""
        };
        // #2640: the `par` join, imported only when a body has a `par` block.
        let settle_all_imports = if runtime_use.settle_all() {
            SETTLE_ALL_RUNTIME_IMPORTS
        } else {
            ""
        };
        // #2683, #2711, #3437: the `within`, `race` and `detach` helpers, imported only when a body uses one.
        let race_imports = effect_runtime_imports(runtime_use);
        // #2620: `unreachable`/`todo`'s helper, imported only when a body calls one.
        let fault_imports = if runtime_use.fault() {
            FAULT_RUNTIME_IMPORTS
        } else {
            ""
        };
        // #2341 (L4): the `RequestHeaders` constructor, imported only when a
        // body built one (`RequestHeaders.of`/`empty`).
        let request_headers_imports = if runtime_use.request_headers() {
            REQUEST_HEADERS_RUNTIME_IMPORTS
        } else {
            ""
        };
        // #3277: the `RequestCookies` constructor, likewise.
        let request_cookies_imports = if runtime_use.request_cookies() {
            REQUEST_COOKIES_RUNTIME_IMPORTS
        } else {
            ""
        };
        // #3386: the session-cookie issuer and deleter, likewise.
        let session_cookie_imports = if runtime_use.session_cookie() {
            SESSION_COOKIE_RUNTIME_IMPORTS
        } else {
            ""
        };
        // #2854: the `RequestQuery` constructor, likewise.
        let request_query_imports = if runtime_use.request_query() {
            REQUEST_QUERY_RUNTIME_IMPORTS
        } else {
            ""
        };
        let query_string_imports = if runtime_use.query_string() {
            QUERY_STRING_RUNTIME_IMPORTS
        } else {
            ""
        };
        let form_codec_imports = if runtime_use.form_codec() {
            FORM_CODEC_RUNTIME_IMPORTS
        } else {
            ""
        };
        let cookie_string_imports = if runtime_use.cookie_string() {
            COOKIE_STRING_RUNTIME_IMPORTS
        } else {
            ""
        };
        let multipart_form_imports = if runtime_use.multipart_form() {
            MULTIPART_FORM_RUNTIME_IMPORTS
        } else {
            ""
        };
        // #1652: the structural-equality walker, imported only when a
        // non-primitive `==`/`!=` lowered to it.
        let eq_imports = if runtime_use.eq() {
            EQ_RUNTIME_IMPORTS
        } else {
            ""
        };
        // #1657: the `Int` domain traps, imported only when a division or a
        // Float→`Int` conversion lowered to them.
        let int_imports = if runtime_use.int() {
            INT_RUNTIME_IMPORTS
        } else {
            ""
        };
        // Constrained generics slice 1: the comparator helpers behind `Ord`.
        let cmp_imports = runtime_use.cmp_imports();
        // v0.153 (ADR 0177): `HttpResult` is a value (its variant namespace) and
        // a type, so it imports without a `type` prefix — one binding serves
        // both `HttpResult.NotFound` and the `HttpResult<T>` annotation.
        let http_imports = if uses_http { ", HttpResult" } else { "" };
        // #1319: `QueueResult` is likewise a value (its verdict namespace) and
        // a type; a bare named import brings both in, matching `HttpResult`'s
        // own shape immediately above.
        let queue_imports = if uses_queue { ", QueueResult" } else { "" };
        // The fixed prefix plus each optional group joins as one comma-space
        // separated run either way — split back into individual names for
        // `TsDecl::Import.names` rather than re-deriving each optional
        // group's own name list a second time (the string-building above,
        // unchanged, already gets this exactly right).
        let inside = format!(
            "Ok, Err, Some, None, type Result, type Option, type ValidationError{codec_imports}{bytes_imports}{bytes_ops_imports}{time_ops_imports}{iso_date_imports}{yaml_imports}{settle_all_imports}{race_imports}{fault_imports}{url_ops_imports}{request_headers_imports}{request_cookies_imports}{session_cookie_imports}{form_codec_imports}{request_query_imports}{query_string_imports}{multipart_form_imports}{cookie_string_imports}{eq_imports}{int_imports}{cmp_imports}{http_imports}{queue_imports}"
        );
        let names: Vec<String> = inside.split(", ").map(str::to_string).collect();
        for stmt in runtime_files::runtime_import_stmts(false, &names, "./runtime.js") {
            out.push_str(&bynk_ts::print_stmt(&stmt, 0));
        }
        writeln!(out).unwrap();
    }
}

/// v0.110 (ADR 0142): the `Bytes` runtime helpers, appended to a module's
/// import list when the emitted body references them. `bytesEqual` backs `==`;
/// the base64/UTF-8 helpers back the kernel and codec.
pub const BYTES_RUNTIME_IMPORTS: &str =
    ", __bynkBytesEqual, __bynkBytesToBase64, __bynkBytesFromBase64, __bynkBytesDecodeUtf8";

/// #2341 (L4): the `RequestHeaders` constructor a test builds headers with.
pub const REQUEST_HEADERS_RUNTIME_IMPORTS: &str = ", __requestHeaders";
/// #2854: the `RequestQuery` constructor a test builds a raw query view with.
pub const REQUEST_QUERY_RUNTIME_IMPORTS: &str = ", __requestQuery";
/// #3277: the `RequestCookies` constructor a test builds a raw cookies view with,
/// and the header text a system-tier driver sends for one.
pub const REQUEST_COOKIES_RUNTIME_IMPORTS: &str = ", __requestCookies, __requestCookiesHeader";
/// #3386: what `Actor.issue(identity)` and `Actor.expire()` lower to, and #3446: what a typed
/// cookie write (`Record.field.cookie(v)`) encodes its value with.
pub const SESSION_COOKIE_RUNTIME_IMPORTS: &str =
    ", __issueSessionCookie, __expireSessionCookie, __sessionKeysOf, __cookieValue";
/// #3386: what a `system` driver mints a `SignedCookie` session cookie with.
pub const SESSION_SIGNING_RUNTIME_IMPORTS: &str = ", __signSessionCookie, __newSessionKey";
/// #2854: the typed-query serialiser a mixed typed+`Wire` system call and a typed
/// query driver turn a record's wire form into a query string with.
pub const QUERY_STRING_RUNTIME_IMPORTS: &str = ", __queryString";

/// #3276: the `FormData` a system driver sends a typed `@multipart` body as.
pub const MULTIPART_FORM_RUNTIME_IMPORTS: &str = ", __multipartForm";

/// #3276: the form codec behind `Form.decode[T]` / `Form.encode(v)`.
pub const FORM_CODEC_RUNTIME_IMPORTS: &str = ", __formDecode, __formString";

/// #3277: the `Cookie` header a system driver sends a typed `cookies` record as.
pub const COOKIE_STRING_RUNTIME_IMPORTS: &str = ", __cookieString";
/// #2854: the key suffix under which a route's typed `query` (position and
/// declared type) sits in the same per-route map as its body param.
pub const QUERY_ROUTE_SUFFIX: &str = "?query";
/// #2910: marks a route whose system drivers take a case's extra request
/// headers in a `__extraHeaders` slot (a route that declares no `headers:`).
pub const EXTRA_HEADERS_ROUTE_SUFFIX: &str = "?extra-headers";

/// #3276: marks a `@form` route in the system drivers' route-body map, so a typed
/// body in a mixed typed-and-`Wire` call is written as a form (`a=1&b=x`), not JSON.
pub const FORM_ROUTE_SUFFIX: &str = "?form";

/// #3277: marks a route's `cookies` record in the system drivers' route-body map, so a
/// typed record in a mixed typed-and-`Wire` call is sent as a `Cookie` header.
pub const COOKIES_ROUTE_SUFFIX: &str = "?cookies";

/// K3 (#2592): the `Instant` ISO-8601 helpers (`toIso`, `Instant.fromIso`),
/// appended only when the body calls one.
pub const TIME_OPS_RUNTIME_IMPORTS: &str = ", __bynkInstantToIso, __bynkInstantFromIso";

/// Hyperdrive H3 (#3320): the typed `Postgres` reads' row pass, a group apart
/// from the `Instant` helpers so a module that only reads ISO text keeps its
/// import list.
pub const PG_ROWS_RUNTIME_IMPORTS: &str = ", __bynkPgRow";

/// The runtime names a module takes from the `time` module: the `Instant`
/// helpers and the typed `Postgres` read pass, each only when the module
/// references it.
pub fn time_runtime_imports(ru: &runtime_use::RuntimeUse) -> String {
    let mut imports = String::new();
    if ru.time_ops() {
        imports.push_str(TIME_OPS_RUNTIME_IMPORTS);
    }
    if ru.pg_rows() {
        imports.push_str(PG_ROWS_RUNTIME_IMPORTS);
    }
    imports
}

/// #2867: the `IsoDate` predicate's check, appended only when a refinement check uses it.
pub const ISO_DATE_RUNTIME_IMPORTS: &str = ", __bynkIsIsoDate";

/// #3214: the YAML parser behind `Yaml.decode[T]`, appended only when a body has one.
pub const YAML_RUNTIME_IMPORTS: &str = ", __bynkYamlParse";

/// #2640: the join a `par { … }` block awaits, appended only when a body has one.
pub const SETTLE_ALL_RUNTIME_IMPORTS: &str = ", __bynkSettleAll";
/// #2683: the `within` race.
pub const WITHIN_RUNTIME_IMPORTS: &str = ", __bynkWithin";

/// #2711: the `race` helper.
pub const RACE_RUNTIME_IMPORTS: &str = ", __bynkRace";

/// #3437: the `detach` reporter.
pub const DETACH_RUNTIME_IMPORTS: &str = ", __bynkDetach";

/// The `within`, `race` and `detach` helpers a module uses, as one import
/// fragment (#2683, #2711, #3437).
pub fn effect_runtime_imports(ru: &RuntimeUse) -> &'static str {
    match (ru.within(), ru.race(), ru.detach()) {
        (false, false, false) => "",
        (true, false, false) => ", __bynkWithin",
        (false, true, false) => ", __bynkRace",
        (false, false, true) => ", __bynkDetach",
        (true, true, false) => ", __bynkWithin, __bynkRace",
        (true, false, true) => ", __bynkWithin, __bynkDetach",
        (false, true, true) => ", __bynkRace, __bynkDetach",
        (true, true, true) => ", __bynkWithin, __bynkRace, __bynkDetach",
    }
}

/// #2620: the deliberate-fault helper behind `unreachable`/`todo`.
pub const FAULT_RUNTIME_IMPORTS: &str = ", __fault";
/// #3581: what a `bynk dev` build's agent module imports to record an instance and
/// answer the inspect route.
fn inspect_agent_imports(on: bool) -> Vec<&'static str> {
    if on {
        vec![
            "__noteAgentInstance",
            "__inspectAgent",
            "__historyBefore",
            "__historyRecord",
        ]
    } else {
        Vec::new()
    }
}

/// #3580: the trace helpers a traced build's module imports, by what it used:
/// `__traceStep` around a call, `__traceBinding` around a cross-context call's
/// Service Binding.
fn trace_step_imports(runtime_use: &RuntimeUse) -> Vec<&'static str> {
    let mut names = Vec::new();
    if runtime_use.trace_step() {
        names.push("__traceStep");
    }
    if runtime_use.trace_binding() {
        names.push("__traceBinding");
    }
    names
}

/// K2 (#2594): the percent-encoding helpers (`percentEncode`,
/// `percentDecode`), appended only when the body calls one.
pub const URL_OPS_RUNTIME_IMPORTS: &str = ", __bynkPercentEncode, __bynkPercentDecode";

/// #2016: the `Bytes` operation helpers (`concat`, `compare`, hex, base64url),
/// appended only when the body calls one, so a module that merely builds or
/// compares `Bytes` keeps the [`BYTES_RUNTIME_IMPORTS`] list it had.
pub const BYTES_OPS_RUNTIME_IMPORTS: &str = ", __bynkBytesConcat, __bynkBytesCompare, __bynkBytesToHex, __bynkBytesFromHex, __bynkBytesToBase64Url, __bynkBytesFromBase64Url";

/// #1652 (runtime-semantics track §3.1): the structural-equality walker,
/// appended to a module's import list when `==`/`!=` on a non-primitive,
/// non-`Bytes` operand lowered to it (`lower_bin_op`).
pub const EQ_RUNTIME_IMPORTS: &str = ", __bynkEq";

/// #1657: the `Int` domain traps (`bynk-emit/runtime/src/int.ts`).
pub const INT_RUNTIME_IMPORTS: &str = ", __bynkIntDiv, __bynkToInt, __bynkIntDivFloor, __bynkIntRem, __bynkIntMod, __bynkIntPow, \
     __bynkIntAnd, __bynkIntOr, __bynkIntXor, __bynkIntAndNot, __bynkIntShl, __bynkIntShr";

/// message-bundles slice 3 (#878, Decision G): the ICU-formatting runtime
/// helpers, appended to a module's import list when an emitted `messages`
/// bundle's `render` references any of them (`emit_icu_placeholder`,
/// `bynk-emit/src/emitter/emit.rs`).
const MESSAGES_RUNTIME_IMPORTS: &str = ", __selectPluralArm, __formatIcuNumber, __formatIcuDate";

/// #914: the names an **inlined** boundary deserialiser builds directly, appended
/// to the import list of a module that curates its own — a Worker's `compose.ts`
/// and the test-scaffold modules. Every other module imports `Ok`/`Err`/`Result`
/// unconditionally, so most of this is inert there and never applied.
///
/// A codec for a *named* type delegates (`handlers.deserialise_Order(…)`) and needs
/// none of these; one for a base type or a `Bytes` inlines the construction. Two
/// arms — `Unit` and the runtime-owned error types — additionally annotate the
/// result (`Ok(undefined) as Result<void, BoundaryError>`), which `compose.ts`'s
/// structural list never carries; `Result` is in the group for those. The dedupe
/// every real caller applies this group through (`emitter/workers.rs`'s own
/// `append_missing_bindings`, `project/tests_emit.rs`'s own copy — #1486
/// retired this doc's own `inject_runtime_imports`, the pre-conversion
/// post-print-text-splice mechanism both are the tree-level successor to)
/// makes it free wherever it is already imported.
pub const BOUNDARY_CODEC_RUNTIME_IMPORTS: &str = ", Ok, Err, type Result, type __BoundaryError";

/// #914: the names the `Json.decode[T]` wrapper puts in the module — its own
/// `Result<T, DecodeError>` signature and the `JsonValue` it parses into
/// (`lower_json_codec_call`, `bynk-emit/src/emitter/lower.rs`).
///
/// A sibling group rather than part of [`BOUNDARY_CODEC_RUNTIME_IMPORTS`]: the
/// producer is the wrapper, not the codec, and it names these whichever arm the
/// inner deserialiser takes — including the delegating ones, which set no
/// boundary-codec flag at all. `Json.encode` needs nothing for its own wrapper
/// text either (it lowers to a bare `JSON.stringify`).
///
/// The delegating arm — `Json.decode[SomeRecord]` / `Json.encode(someRecord)`
/// — used to be broken in a test-scaffold module for an unrelated reason
/// (issue #917): the call lowers to a bare `deserialise_SomeRecord(…)` /
/// `serialise_SomeRecord(…)`, and no such codec was emitted anywhere — the
/// unit module exports the record's interface and no codec of its own. Fixed
/// by generating the test module's *own* closure for every root a case body's
/// `Json` call reaches for (`RuntimeUse::note_json_codec_root`, drained by
/// `tests_emit.rs`'s `emit_test_module`), namespace-qualifying the TS type
/// positions through the target/`uses` unit's own namespace import
/// (`RuntimeUse::json_codec_qual`) — the same caller-generates-its-own-codec
/// pattern `emit_consumed_context_helpers` uses for a workers cross-context
/// caller's consumed-boundary types (#661). See
/// `918_json_decode_in_test_case` (base type, delegation-free) and
/// `919_json_decode_named_record_in_test_case` (named record, delegating).
pub const JSON_CODEC_RUNTIME_IMPORTS: &str =
    ", Ok, Err, type Result, type __JsonValue, type DecodeError";

/// Emit the commons-level doc block (if any) at the current position.
/// #1478: real-node-internally already (a single `TsStmtKind::DocComment`,
/// the same shape [`emit_doc_block`] itself builds — inlined directly here
/// rather than calling that `out: &mut String`-shaped helper, since its own
/// signature and 14 real callers stay unchanged, #1333's own doc).
fn write_commons_doc(commons: &TypedCommons) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    if let Some(doc) = &commons.commons.documentation {
        stmts.push(bynk_ts::TsStmt::doc_comment(doc.as_str(), None));
        // #1486: no explicit trailing blank stmt here — `DocComment` (unlike
        // `Comment`) is never exempted from the printer's automatic
        // top-level spacing policy, so the single blank before whatever
        // follows is already supplied for free.
    }
    stmts
}

/// The module-level state-registry constant name for an agent class.
fn agent_registry_name(agent: &str) -> String {
    format!("__{agent}Registry")
}

/// The exported agent-construction factory name for an agent class.
pub fn agent_factory_name(agent: &str) -> String {
    format!("__make{agent}")
}

/// Takes the AST `BinOp` directly, on purpose. Phase 6 (P6.56, ADR 0381)
/// investigated converting this to an IR-side operator enum and declined:
/// its sole caller (`emitter/lower.rs`) holds an AST `BinOp` from
/// `ExprKind::BinOp` and separately compares `op == BinOp::Eq` a few lines
/// away, so converting here would only relocate the AST read into that
/// still-AST-walking caller, net zero. The IR cutover track (#1542, §10)
/// later confirmed the general form of that finding — retyping the
/// lowerer's reads one function at a time produces a second lowerer, not a
/// retype — and deleted the IR-side operator enum with the rest of the
/// expression IR (Slice D2).
fn ts_binop(op: BinOp) -> &'static str {
    match op {
        // `implies` has no single TS operator — `lower_bin_op` rewrites it to
        // `(!(P) || Q)` before reaching here, so this arm is never used.
        BinOp::Implies => "||",
        BinOp::Or => "||",
        BinOp::And => "&&",
        BinOp::Eq => "===",
        BinOp::NotEq => "!==",
        BinOp::Lt => "<",
        BinOp::LtEq => "<=",
        BinOp::Gt => ">",
        BinOp::GtEq => ">=",
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
    }
}

/// #1653: the TypeScript property name of a sum variant's payload field. A
/// variant object already carries the in-memory discriminant `tag`, so a
/// payload field of that name moves to `$tag` (`$` is not a Bynk identifier
/// character, so no other field can be spelled that way). Only the in-memory
/// shape changes: the wire key stays the field's own name, and the codec maps
/// between the two.
pub(crate) fn payload_prop(field: &str) -> String {
    if field == "tag" {
        "$tag".to_string()
    } else {
        field.to_string()
    }
}

/// The TypeScript spelling of a user identifier in a *binding or reference*
/// position. Defined in `bynk_check::emitted_names` so the language server's
/// inline values (#2476) name the same variable the emitter declares.
pub use bynk_check::emitted_names::ts_ident;

/// The one TypeScript string-literal escaper lives in `bynk_ts` (crate-hygiene A2);
/// the emitter keeps its historical name for it.
pub use bynk_ts::escape_ts_literal as escape_ts_string;

#[allow(dead_code)]
fn _unused_hashmap(_h: HashMap<String, ()>) {}

#[cfg(test)]
mod runtime_tests {
    use super::*;

    /// Every relative import in a per-module source names another module in the
    /// table, so any reachable subset can be closed over its own imports.
    #[test]
    fn per_module_runtime_imports_resolve_within_the_table() {
        let names: Vec<&str> = runtime_modules::RUNTIME_MODULES
            .iter()
            .map(|(n, _)| *n)
            .collect();
        for (name, src) in runtime_modules::RUNTIME_MODULES {
            for line in src.lines().filter(|l| {
                (l.starts_with("import ") || l.starts_with("} from ")) && l.contains('"')
            }) {
                let from = line.rsplit('"').nth(1).expect("import specifier");
                // ADR 0799 C: a module may import the first-party npm package
                // `bynk-std` declares for it, and nothing else outside the table.
                if bynk_check::firstparty::FIRST_PARTY_NPM.iter().any(|d| {
                    // Hyperdrive H5 (#3322): or a subpath of it
                    // (`mysql2/promise`, mysql2's promise API).
                    d.runtime_module == *name
                        && (d.package == from
                            || from
                                .strip_prefix(d.package)
                                .is_some_and(|r| r.starts_with('/')))
                }) {
                    continue;
                }
                let target = from.strip_prefix("./").and_then(|f| f.strip_suffix(".ts"));
                assert!(
                    target.is_some_and(|t| names.contains(&t) && t != *name),
                    "{name}: import {from:?} does not name another runtime module"
                );
            }
        }
    }

    /// ADR 0799 C: each first-party npm pin is exact and is the version the
    /// runtime's own tests and type-check run against (`runtime/package.json`),
    /// so what a project installs is what was tested, and the module it names
    /// really imports the package.
    #[test]
    fn first_party_pins_match_the_runtime() {
        let manifest =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/runtime/package.json"))
                .expect("runtime/package.json");
        for d in bynk_check::firstparty::FIRST_PARTY_NPM {
            assert!(
                manifest.contains(&format!("\"{}\": \"{}\"", d.package, d.version)),
                "`{}` is pinned at {} in bynk-std but not in runtime/package.json",
                d.package,
                d.version
            );
            let (_, src) = runtime_modules::RUNTIME_MODULES
                .iter()
                .find(|(n, _)| *n == d.runtime_module)
                .expect("the runtime module exists");
            assert!(
                src.contains(&format!("from \"{}\"", d.package))
                    || src.contains(&format!("from \"{}/", d.package)),
                "runtime module `{}` does not import `{}`",
                d.runtime_module,
                d.package
            );
        }
    }

    /// The `__` aliases emitted code imports live in the module that defines
    /// each name, so a per-module import line finds them there.
    #[test]
    fn runtime_import_splits_per_defining_module() {
        let names: Vec<String> = ["Ok", "type Result", "__callService", "type __JsonValue"]
            .map(String::from)
            .to_vec();
        let got: Vec<(bool, Vec<String>, String)> =
            runtime_files::split_runtime_import(false, &names, "../runtime.js")
                .into_iter()
                .map(|d| match d {
                    bynk_ts::TsDecl::Import {
                        type_only,
                        names,
                        from,
                    } => (type_only, names, from),
                    _ => unreachable!(),
                })
                .collect();
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            got,
            vec![
                (
                    false,
                    v(&["Ok", "type Result"]),
                    "../__bynk_runtime/result.js".to_string()
                ),
                (
                    false,
                    v(&["__callService", "type __JsonValue"]),
                    "../__bynk_runtime/boundary.js".to_string()
                ),
            ]
        );
    }

    /// #2443: `HttpResult` lives apart from the HTTP response helpers, so a
    /// bundle-target service that only names it does not reach `http` or
    /// (through its type edges) `boundary`.
    #[test]
    fn http_result_is_reached_without_the_http_helpers() {
        use std::collections::BTreeSet;
        let names = vec!["HttpResult".to_string()];
        let decls = runtime_files::split_runtime_import(false, &names, "./runtime.js");
        assert!(matches!(
            &decls[..],
            [bynk_ts::TsDecl::Import { from, .. }] if from == "./__bynk_runtime/httpresult.js"
        ));
        let reached: BTreeSet<(String, String)> =
            [("httpresult".to_string(), "js".to_string())].into();
        let paths: Vec<String> = runtime_files::closure_files(&reached)
            .iter()
            .map(|(p, _)| p.display().to_string())
            .collect();
        assert_eq!(paths, ["__bynk_runtime/httpresult.ts"]);
    }

    #[test]
    fn runtime_files_close_over_module_imports() {
        use std::collections::BTreeSet;
        let reached: BTreeSet<(String, String)> =
            [("equality".to_string(), "js".to_string())].into();
        let files = runtime_files::closure_files(&reached);
        let paths: Vec<String> = files.iter().map(|(p, _)| p.display().to_string()).collect();
        // equality -> bytes -> result, and equality -> decimal (#2622).
        assert_eq!(
            paths,
            [
                "__bynk_runtime/bytes.ts",
                "__bynk_runtime/decimal.ts",
                "__bynk_runtime/equality.ts",
                "__bynk_runtime/result.ts"
            ]
        );
        assert!(
            files.iter().all(|(_, s)| !s.contains(".ts\";")),
            "siblings are spelled for the target extension"
        );
        assert!(runtime_files::closure_files(&BTreeSet::new()).is_empty());
    }

    #[test]
    fn handwritten_runtime_imports_are_rewritten() {
        let src = "import { None, Some, type Option, type KVNamespace } from \"../runtime.js\";\nimport { x } from \"./other.js\";\n";
        let out = runtime_files::rewrite_runtime_imports(src);
        assert!(
            out.contains(
                "import { None, Some, type Option } from \"../__bynk_runtime/result.js\";"
            ),
            "{out}"
        );
        assert!(
            out.contains("import type { KVNamespace } from \"../__bynk_runtime/storage.js\";"),
            "{out}"
        );
        assert!(out.contains("import { x } from \"./other.js\";"), "{out}");
    }

    #[test]
    fn handwritten_runtime_import_forms_are_handled_or_left_alone() {
        let r = runtime_files::rewrite_runtime_imports;
        // Multi-line, unterminated, indented, trailing comment, CRLF.
        let multi = "import {\n  Ok,\n  Err,\n} from \"./runtime.js\";\nconst x = 1;\n";
        assert_eq!(
            r(multi),
            "import { Ok, Err } from \"./__bynk_runtime/result.js\";\nconst x = 1;\n"
        );
        assert_eq!(
            r("  import { Ok } from \"./runtime.js\" // keep\n"),
            "  import { Ok } from \"./__bynk_runtime/result.js\"; // keep\n"
        );
        assert_eq!(
            r("import { Ok } from \"./runtime.js\"\r\nx\r\n"),
            "import { Ok } from \"./__bynk_runtime/result.js\";\r\nx\r\n"
        );
        // An unknown name, a namespace import and a foreign specifier stay as written.
        for keep in [
            "import { Ok, Reslt } from \"./runtime.js\";\n",
            "import * as rt from \"./runtime.js\";\n",
            "import { a } from \"./lib/other.js\";\n",
        ] {
            assert_eq!(r(keep), keep);
        }
    }

    #[test]
    fn an_unknown_runtime_module_emits_nothing_instead_of_panicking() {
        use std::collections::BTreeSet;
        let reached: BTreeSet<(String, String)> = [("nope".to_string(), "js".to_string())].into();
        assert!(runtime_files::closure_files(&reached).is_empty());
    }

    #[test]
    fn per_module_runtime_carries_its_aliases() {
        let all: String = runtime_modules::RUNTIME_MODULES
            .iter()
            .map(|(_, s)| *s)
            .collect();
        for alias in [
            "__callService",
            "__matchPath",
            "__makeAgent",
            "__JsonValue",
            "__formatIcuDate",
        ] {
            assert!(
                all.contains(alias),
                "{alias} missing from the per-module runtime"
            );
        }
        let (_, http) = runtime_modules::RUNTIME_MODULES
            .iter()
            .find(|(n, _)| *n == "http")
            .unwrap();
        assert!(http.contains("matchPath as __matchPath"));
    }

    #[test]
    fn runtime_emits_all_required_exports() {
        let s: String = emit_all_runtime_modules()
            .into_iter()
            .map(|(_, src)| src)
            .collect();
        // Core types and constructors used by every emitted module.
        assert!(s.contains("export type Result<T, E>"));
        assert!(s.contains("export const Ok"));
        assert!(s.contains("export const Err"));
        assert!(s.contains("export type Option<T>"));
        assert!(s.contains("export const Some"));
        assert!(s.contains("export const None"));
        assert!(s.contains("export interface ValidationError"));
        // Durable Object surface used by agent classes.
        assert!(s.contains("export interface DurableObjectStorage"));
        assert!(s.contains("export interface DurableObjectState"));
        assert!(s.contains("export class InMemoryStorage"));
        assert!(s.contains("export function makeTestState"));
        // Discriminator must be `tag` to match emitted code.
        assert!(s.contains("tag: \"Ok\""));
        assert!(s.contains("tag: \"Err\""));
        assert!(s.contains("tag: \"Some\""));
        assert!(s.contains("tag: \"None\""));
    }

    #[test]
    fn tsconfig_is_well_formed_json() {
        let s = emit_tsconfig();
        // Spot-check the key fields; we don't reach for a JSON parser.
        assert!(s.contains("\"target\": \"ES2022\""));
        assert!(s.contains("\"strict\": true"));
        assert!(s.contains("\"include\""));
    }

    #[test]
    fn tsconfig_excludes_a_members_client_too() {
        // #3562: a group's member writes `<member>.bynk-client.ts`, which
        // `**/bynk-client.ts` does not match.
        let s = emit_tsconfig();
        assert!(s.contains("\"**/bynk-client.ts\""), "{s}");
        assert!(s.contains("\"**/*.bynk-client.ts\""), "{s}");
    }

    #[test]
    fn coverage_tsconfig_enables_source_maps() {
        // #854: the coverage remap consumes tsc's `.js.map`s, so the variant must
        // set `sourceMap` — a guard against a silent string-replace miss if the
        // base config's `outDir` line is ever reworded. The default stays map-free
        // so a normal `bynk test` / deployment build ships no `.js.map`s.
        let cov = emit_tsconfig_with_source_maps();
        assert!(
            cov.contains("\"sourceMap\": true"),
            "coverage config: {cov}"
        );
        assert!(cov.contains("\"outDir\": \"../out-js\""));
        assert!(!emit_tsconfig().contains("sourceMap"));
    }

    #[test]
    fn workers_dir_name_replaces_dots_with_dashes() {
        assert_eq!(
            bynk_project::worker_dir_name("commerce.payment"),
            "commerce-payment"
        );
        assert_eq!(bynk_project::worker_dir_name("a.b.c"), "a-b-c");
    }

    // Refactor track: characterisation pin for the canonical `escape_ts_string`.
    // It escapes backslash/quote/newline/tab and carriage return (`\r` → `\r`).
    #[test]
    fn escape_ts_string_escapes_cr() {
        assert_eq!(escape_ts_string("a\\b"), "a\\\\b");
        assert_eq!(escape_ts_string("a\"b"), "a\\\"b");
        assert_eq!(escape_ts_string("a\nb"), "a\\nb");
        assert_eq!(escape_ts_string("a\tb"), "a\\tb");
        assert_eq!(escape_ts_string("a\rb"), "a\\rb"); // CR escaped here; raw in project copy
    }

    #[test]
    fn runtime_import_depth_resolves_correctly() {
        assert_eq!(
            runtime_import_for(Path::new("compose.ts"), ImportExt::Js),
            "./runtime.js"
        );
        assert_eq!(
            runtime_import_for(Path::new("commerce/payment.ts"), ImportExt::Js),
            "../runtime.js"
        );
        assert_eq!(
            runtime_import_for(Path::new("commerce/orders/types.ts"), ImportExt::Js),
            "../../runtime.js"
        );
        assert_eq!(
            runtime_import_for(Path::new("tests/commerce_payment.test.ts"), ImportExt::Js),
            "../runtime.js"
        );
    }
}

/// Which conditional runtime helpers a module's import line ends up carrying.
///
/// These drive the single-file `emit()` path end-to-end (parse → resolve → check
/// → emit), so they exercise the real producers rather than the accumulator in
/// isolation. Before `RuntimeUse`, the decision was `body.contains("__bynkBytes")`
/// — a scan of the generated text — and `escapes_a_marker_in_a_string_literal`
/// below is the case that got wrong.
#[cfg(test)]
mod conditional_runtime_import_tests {
    use crate::testkit::{emit_bundle, emit_source};

    /// The runtime import lines of the emitted module, joined: one `import …
    /// from "./__bynk_runtime/<module>.js"` per module it uses.
    fn runtime_import_line(ts: &str) -> String {
        ts.lines()
            .filter(|l| l.starts_with("import ") && l.contains("__bynk_runtime/"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn bytes_helpers_are_imported_when_a_bytes_value_is_built() {
        let ts = emit_source(
            "commons b\n\nfn decode(s: String) -> Option[Bytes] {\n  Bytes.fromBase64(s)\n}\n",
        );
        assert!(
            runtime_import_line(&ts).contains("__bynkBytesFromBase64"),
            "{ts}"
        );
    }

    #[test]
    fn bytes_helpers_are_imported_for_content_equality() {
        let ts = emit_source("commons b\n\nfn same(a: Bytes, b: Bytes) -> Bool {\n  a == b\n}\n");
        assert!(
            runtime_import_line(&ts).contains("__bynkBytesEqual"),
            "{ts}"
        );
    }

    /// #2016: the operation helpers are their own group. A module that only
    /// builds or compares `Bytes` keeps its import list, and one that calls an
    /// operation gets that group without the base one.
    #[test]
    fn bytes_ops_helpers_are_imported_only_when_an_operation_is_called() {
        let base = emit_source("commons b\n\nfn same(a: Bytes, b: Bytes) -> Bool {\n  a == b\n}\n");
        assert!(
            !runtime_import_line(&base).contains("__bynkBytesConcat"),
            "{base}"
        );
        let ops = emit_source("commons b\n\nfn hex(b: Bytes) -> String {\n  b.toHex()\n}\n");
        let line = runtime_import_line(&ops);
        assert!(line.contains("__bynkBytesToHex"), "{ops}");
        assert!(!line.contains("__bynkBytesEqual"), "{ops}");
    }

    /// K3 (#2592): the `Instant` ISO-8601 helpers come in only when called,
    /// and each lowers to its own helper.
    #[test]
    fn instant_iso_helpers_are_imported_only_when_called() {
        let base = emit_source("commons t\n\nfn ms(i: Instant) -> Int {\n  i.toEpochMillis()\n}\n");
        assert!(
            !runtime_import_line(&base).contains("__bynkInstant"),
            "{base}"
        );
        let ts = emit_source(
            "commons t\n\nfn iso(i: Instant) -> String {\n  i.toIso()\n}\n\nfn read(s: String) -> Option[Instant] {\n  Instant.fromIso(s)\n}\n",
        );
        let line = runtime_import_line(&ts);
        assert!(line.contains("__bynkInstantToIso"), "{ts}");
        assert!(line.contains("__bynkInstantFromIso"), "{ts}");
        assert!(ts.contains("__bynkInstantToIso(i)"), "{ts}");
        assert!(ts.contains("__bynkInstantFromIso(s)"), "{ts}");
    }

    /// K2 (#2594): the percent-encoding helpers come in only when called.
    #[test]
    fn percent_helpers_are_imported_only_when_called() {
        let base = emit_source("commons u\n\nfn n(s: String) -> Int {\n  s.length()\n}\n");
        assert!(
            !runtime_import_line(&base).contains("__bynkPercent"),
            "{base}"
        );
        let ts = emit_source(
            "commons u\n\nfn e(s: String) -> String {\n  s.percentEncode()\n}\n\nfn d(s: String) -> Option[String] {\n  s.percentDecode()\n}\n",
        );
        let line = runtime_import_line(&ts);
        assert!(
            line.contains("__bynkPercentEncode") && line.contains("__bynkPercentDecode"),
            "{ts}"
        );
        assert!(
            ts.contains("__bynkPercentEncode(s)") && ts.contains("__bynkPercentDecode(s)"),
            "{ts}"
        );
    }

    /// #2016: `slice` lowers inline, so it pulls in no helper at all.
    #[test]
    fn bytes_slice_needs_no_helper() {
        let ts = emit_source("commons b\n\nfn tag(b: Bytes) -> Bytes {\n  b.slice(0, 4)\n}\n");
        assert!(!runtime_import_line(&ts).contains("__bynkBytes"), "{ts}");
    }

    #[test]
    fn bytes_helpers_are_absent_from_a_module_that_never_uses_bytes() {
        let ts = emit_source("commons b\n\nfn double(n: Int) -> Int {\n  n * 2\n}\n");
        assert!(!runtime_import_line(&ts).contains("__bynkBytes"), "{ts}");
    }

    /// The regression this replaced a text scan for: a `Bytes` helper name
    /// appearing inside a user **string literal** is not a reference to the
    /// helper, and must not pull the import in. `body.contains("__bynkBytes")`
    /// could not tell the two apart, because the literal is emitted verbatim
    /// into the same buffer it scanned.
    #[test]
    fn escapes_a_marker_in_a_string_literal() {
        let ts = emit_source("commons b\n\nfn label() -> String {\n  \"__bynkBytesEqual\"\n}\n");
        assert!(
            ts.contains("\"__bynkBytesEqual\""),
            "the literal should survive into the body: {ts}"
        );
        assert!(
            !runtime_import_line(&ts).contains("__bynkBytes"),
            "a marker inside a string literal is not a helper reference: {ts}"
        );
    }

    // -- the ICU formatters ---------------------------------------------------

    const ICU_HELPERS: [&str; 3] = ["__selectPluralArm", "__formatIcuNumber", "__formatIcuDate"];

    /// The case the per-arm recording exists for. A `select` placeholder lowers to
    /// `Object.hasOwn` over an arm table and calls no formatter, so a bundle whose
    /// only ICU construct is a `select` must import none of the three — recording
    /// once per placeholder instead of per arm would import all three here.
    #[test]
    fn a_select_only_bundle_imports_no_icu_formatter() {
        let ts = emit_bundle(
            "messages \"en\" @reference {\n  \"greeting\" => \"{g, select, male {He} female {She} other {They}} liked this.\"\n}\n",
        );
        assert!(
            ts.contains("globalThis.Object.hasOwn"),
            "the select arm table should have been emitted, else this proves nothing: {ts}"
        );
        for helper in ICU_HELPERS {
            assert!(
                !runtime_import_line(&ts).contains(helper),
                "a select-only bundle calls no formatter, so `{helper}` must not be imported: {ts}"
            );
        }
    }

    /// The opposite direction: a `plural` placeholder does call a formatter, and
    /// the three are imported as a group.
    #[test]
    fn a_plural_bundle_imports_the_icu_formatters() {
        let ts = emit_bundle(
            "messages \"en\" @reference {\n  \"cart\" => \"You have {n, plural, one {# item} other {# items}} in your cart\"\n}\n",
        );
        assert!(
            ts.contains("__selectPluralArm("),
            "the plural dispatch should have been emitted: {ts}"
        );
        for helper in ICU_HELPERS {
            assert!(
                runtime_import_line(&ts).contains(helper),
                "`{helper}` should be imported for a plural bundle: {ts}"
            );
        }
    }

    /// A bundle with no ICU dispatch at all — a plain `{name}` placeholder goes
    /// through `renderArg`, not a formatter.
    #[test]
    fn a_plain_placeholder_bundle_imports_no_icu_formatter() {
        let ts =
            emit_bundle("messages \"en\" @reference {\n  \"hello\" => \"Hello, {name}!\"\n}\n");
        for helper in ICU_HELPERS {
            assert!(
                !runtime_import_line(&ts).contains(helper),
                "`{helper}` must not be imported for a bundle with no ICU dispatch: {ts}"
            );
        }
    }
}

/// P6.32 (design/tracks/the-ir.md §6a): pins the marker-parameterised
/// `type_ref_mentions` against the exact truth table its three former
/// hand-rolled copies (`file_mentions_json_error`/`_http_result`/
/// `_connection`) each implemented independently — in particular, that a
/// marker's own wrapper variant stops the recursion (matching the original
/// `=> true` arms) rather than also searching that variant's own inner type,
/// and that a non-matching wrapper variant recurses rather than reporting
/// `false` outright.
#[cfg(test)]
mod type_ref_mentions_tests {
    use super::*;

    fn base(b: BaseType) -> TypeRef {
        TypeRef::Base(b, bynk_syntax::span::Span::new(0, 0))
    }

    fn http_result(inner: TypeRef) -> TypeRef {
        TypeRef::HttpResult(Box::new(inner), bynk_syntax::span::Span::new(0, 0))
    }

    fn connection(inner: TypeRef) -> TypeRef {
        TypeRef::Connection(Box::new(inner), bynk_syntax::span::Span::new(0, 0))
    }

    fn json_error() -> TypeRef {
        TypeRef::DecodeError(bynk_syntax::span::Span::new(0, 0))
    }

    #[test]
    fn a_markers_own_wrapper_matches_regardless_of_its_inner_type() {
        // `HttpResult[String]` matches the `HttpResult` marker even though its
        // own inner type (`String`) does not itself mention `HttpResult` —
        // the wrapper variant itself is the match, mirroring each original
        // function's own unconditional `=> true` arm.
        let t = http_result(base(BaseType::String));
        assert!(type_ref_mentions(&t, TypeRefMarker::HttpResult));
        assert!(!type_ref_mentions(&t, TypeRefMarker::Connection));
        assert!(!type_ref_mentions(&t, TypeRefMarker::DecodeError));
    }

    #[test]
    fn a_non_matching_wrapper_still_recurses_into_its_inner_type() {
        // `HttpResult[Connection[String]]` does not match the `HttpResult`
        // marker itself when the marker is `Connection` — it must still find
        // the `Connection` nested one level in.
        let t = http_result(connection(base(BaseType::String)));
        assert!(type_ref_mentions(&t, TypeRefMarker::Connection));
        assert!(type_ref_mentions(&t, TypeRefMarker::HttpResult));
        assert!(!type_ref_mentions(&t, TypeRefMarker::DecodeError));
    }

    #[test]
    fn json_error_has_no_inner_type_to_recurse_into() {
        let t = json_error();
        assert!(type_ref_mentions(&t, TypeRefMarker::DecodeError));
        assert!(!type_ref_mentions(&t, TypeRefMarker::HttpResult));
        assert!(!type_ref_mentions(&t, TypeRefMarker::Connection));
    }

    #[test]
    fn a_plain_base_type_matches_no_marker() {
        let t = base(BaseType::Int);
        assert!(!type_ref_mentions(&t, TypeRefMarker::DecodeError));
        assert!(!type_ref_mentions(&t, TypeRefMarker::HttpResult));
        assert!(!type_ref_mentions(&t, TypeRefMarker::Connection));
    }
}

/// #1832: the import collector reaches a match-arm guard, so a function named
/// only in a guard is imported.
#[cfg(test)]
mod guard_import_tests {
    use crate::testkit::emit_project;

    #[test]
    fn a_function_named_only_in_a_guard_is_imported() {
        let out = emit_project(
            &[
                ("d/lim.bynk", "commons d.lim\n\nfn lim() -> Int { 9 }\n"),
                (
                    "d/app.bynk",
                    "commons d.app\n\nuses d.lim\n\nfn g(n: Int) -> Int {\n  match n {\n    k if k > lim() => 1\n    _ => 0\n  }\n}\n",
                ),
            ],
            bynk_check::project_model::BuildTarget::Bundle,
        );
        let app = out.text("d/app.ts");
        assert!(
            app.lines()
                .any(|l| l.starts_with("import") && l.contains("lim") && l.contains("./lim.js")),
            "{app}"
        );
    }
}
