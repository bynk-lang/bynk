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
use std::path::{Path, PathBuf};
use std::sync::Arc;

// P7.5 (#1307): relocated from `emitter/source_map.rs` to `bynk-ts`,
// unchanged — `bynk-ts/src/source_map.rs`'s own module doc has the full
// grounding for why this API stays as-is while `bynk-ts`'s own printer
// gets a second, simpler way to use the same type.
use bynk_ts::{
    SourceMapBuilder, TsType, decimal_signed_coef, pred_condition_and_message,
    pred_condition_and_message_for,
};

use crate::emit_model::{EmitProjectCtx, ExtraImport, ImportExt};
use bynk_check::builtin_names::map_query;
use bynk_check::builtin_names::methods::RAW;
use bynk_check::builtin_names::types::*;
use bynk_check::checker::{CheckedProgram, NamedKind, Ty, TyId, TypedCommons, Types};
use bynk_check::kernel_op::{FormStatic, JsonStatic, StaticOp, YamlStatic};
use bynk_check::project_model::BuildTarget;
use bynk_check::symbols::UnitTable;
use bynk_model::TypeShape;
use bynk_model::walk_block_exprs;
use bynk_model::{
    lower_capability_ops_ir, lower_protocol_ir, lower_service_handler_signature_ir,
    lower_store_field_shape_ir, lower_type_shape_ir,
};
use bynk_project::UnitKind;
use bynk_syntax::ast::{
    AgentDecl, BaseType, BinOp, CommonsItem, Expr, ExprKind, FnDecl, FnName, Ident, LambdaExpr,
    MatchBody, MessagesDecl, PredKind, ServiceDecl, TypeBody, TypeDecl, TypeRef, rounding_mode,
};
use ctx::UnitFacts;
use ts_types::{TyRender, asset_ts_type, ts_base};

pub mod client;
pub mod commons_barrel;
pub(crate) mod compose;
pub mod containers;
pub mod contracts;
pub(crate) mod events_fanout;
pub(crate) mod harness_names;
pub mod http_client;
pub mod secrets;
pub mod serialisation;
pub mod toml_doc;
pub(crate) mod workers;
pub(crate) mod workers_email;
pub(crate) mod workers_entry;
pub mod workers_group;
pub mod wrangler;

pub use compose::bundle::emit_composition_root;
pub use compose::workers::emit_worker_compose;
pub use compose::{
    ComposeFacts, ComposePlan, ComposeUnit, ComposeUnitFacts, ConsumedSurface, EventPlan,
    EventSubscription, ExecNeeds,
};
pub use containers::{container_class_names, emit_containers_ts};
pub use events_fanout::{Publisher, emit_events_fanout_do, emit_group_events_fanout_do};
pub use secrets::emit_secrets_manifest;
pub use toml_doc::print_toml_document;
pub use workers_entry::{
    emit_worker_entry, field_spec_source, form_body_specs, renamed_specs_source,
    table_has_plain_json,
};
pub use workers_group::{GroupEntry, GroupMember, emit_group_entry};
pub use wrangler::{ClassTombstones, emit_wrangler_toml};

mod cmp;
mod codec;
pub mod ctx;
mod default_value;
mod internal_fault;
mod lower;
pub(crate) mod lower_call;
pub(crate) mod lower_ctx;
mod lower_list;
pub(crate) mod lower_stmt;
pub(crate) mod runtime_header;
pub(crate) mod runtime_use;
mod tco;
pub mod ts_types;
mod zero_value;
pub use internal_fault::{InternalFault, MISSING_CHECKER_FACT};
pub(crate) use lower::*;
pub use runtime_use::RuntimeUse;
pub mod emit;
pub(crate) use bynk_check::icu::{self, *};
pub(crate) use bynk_check::websocket;
pub use emit::*;

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

/// Emit TypeScript source for the typed commons (single-file mode).
///
/// Takes a [`CheckedProgram`] rather than a bare `TypedCommons` (T3.7, R3.10):
/// the only way to obtain one is [`bynk_check::checker::certify`], so this
/// function can no longer be called with an unchecked or partially-checked
/// program by construction.
///
/// 1A.34: a checker fact the emitter needed and did not find is an
/// [`InternalFault`] (see [`emit_project`]); the text built alongside is then unused.
pub(crate) fn emit(program: &CheckedProgram) -> Result<String, Vec<InternalFault>> {
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
    body_stmts.extend(emit_json_codec_helpers(
        commons,
        &dummy_ctx,
        &HashSet::new(),
        &HashSet::new(),
    ));
    let mut stmts = header_single(commons, &body_stmts);
    stmts.extend(body_stmts);
    let out = bynk_ts::print(&bynk_ts::TsProgram { stmts }, "", "", "").text;
    let faults = dummy_ctx.runtime_use.take_faults();
    if faults.is_empty() {
        Ok(out)
    } else {
        Err(faults)
    }
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
        agent_given_deps_names: HashMap::new(),
        extra_imports: Vec::new(),
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

/// Append one item's statements to `body`, giving the first the item's span: the checkpoint
/// every line up to the next span maps back to.
fn extend_with_span(
    body: &mut Vec<bynk_ts::TsStmt>,
    mut stmts: Vec<bynk_ts::TsStmt>,
    span: bynk_syntax::span::Span,
) {
    if let Some(first) = stmts.first_mut() {
        first.span = Some(span);
    }
    body.extend(stmts);
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
///
/// 1A.34: a checker fact the emitter needed and did not find is an
/// [`InternalFault`], not an `any`, an `undefined` or a panic. Lowering carries
/// on past each, so the `Err` holds every fault of the unit; the program built
/// alongside is then never used.
pub fn emit_project(
    program: &CheckedProgram,
    ctx: &EmitProjectCtx,
    source_text: &str,
    source_name: &str,
) -> Result<(bynk_ts::TsProgram, Option<String>), Vec<InternalFault>> {
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
    // The body first: the names this file imports from elsewhere (a sibling file, a
    // used commons, a consumed context) are the ones the body uses and does not
    // declare, read off the tree below (1A.36b-2, #3737 Decision A).
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
        .filter_map(CommonsItem::as_messages)
        .collect();
    if let Some(reference) = messages_blocks
        .iter()
        .find(|m| m.annotations.iter().any(|a| a.name.name == "reference"))
    {
        let mut item_stmts = emit_messages_bundle(&messages_blocks, reference);
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
                if let Some(item_stmts) = emit_provider(p, commons, ctx) {
                    extend_with_span(&mut body, item_stmts, p.span);
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
                let item_stmts = emit_service(s, &protocol, &signatures, commons, ctx);
                extend_with_span(&mut body, item_stmts, s.span);
            }
            CommonsItem::Agent(a) => {
                let state = emit::agent_state_ir(a, program);
                let mut item_stmts = emit_agent(a, &state, commons, ctx, source_text);
                if let Some(first) = item_stmts.first_mut() {
                    first.span = Some(a.span);
                }
                body.extend(item_stmts);
            }
            CommonsItem::Type(_)
            | CommonsItem::Fn(_)
            | CommonsItem::Actor(_)
            | CommonsItem::Messages(_)
            | CommonsItem::Event(_)
            | CommonsItem::Asset(_)
            | CommonsItem::Embed(_)
            | CommonsItem::Const(_) => {}
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
            CommonsItem::Type(_)
            | CommonsItem::Fn(_)
            | CommonsItem::Capability(_)
            | CommonsItem::Provider(_)
            | CommonsItem::Service(_)
            | CommonsItem::Actor(_)
            | CommonsItem::Messages(_)
            | CommonsItem::Event(_)
            | CommonsItem::Asset(_)
            | CommonsItem::Embed(_)
            | CommonsItem::Const(_) => None,
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
    let json_stmts = emit_json_codec_helpers(commons, ctx, &boundary_names, &boundary_insts);
    body.extend(json_stmts);
    // 1A.36b-2: what the module uses and does not declare, over the whole of it: the
    // imports the driver adds for the Durable Object deps (`ctx.extra_imports`) and the
    // boundary helpers' own imports bind names the body spells.
    let used = {
        let mut imports = extra_import_stmts(ctx);
        imports.extend(boundary_imports.iter().cloned());
        let bound = bynk_ts::TsProgram { stmts: imports }.imported_names();
        used_external_names(commons, ctx, &body, &bound)
    };
    let references = external_references_of(&used, commons, ctx);
    let mut project_imports = emit_project_imports(commons, ctx, &references);
    let mut cross_context_imports = emit_cross_context_namespace_imports(commons, ctx, &used);
    // #1486: the boundary between `emit_project_imports`'s own last
    // statement and `emit_cross_context_namespace_imports`'s own first
    // (when non-empty) must reproduce the pre-#1486 code's own unconditional
    // rule exactly — no blank, *except* when `references` is non-empty,
    // which wants exactly one separating "this file's own sibling imports"
    // from "this file's cross-context namespace imports" — regardless of
    // either side's real `TsStmtKind`, which the printer's automatic
    // "no blank between adjacent imports" exemption cannot see through:
    // `emit_project_imports`'s own last statement is not always a real
    // import decl, and the printer's "no blank between adjacent imports"
    // exemption only applies when both sides are. (Its `extra_imports` tail was
    // pre-formatted `Raw` text until 1A.38a; it is import nodes now.)
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
    let mut program_stmts = write_header(commons, ctx, &stmts);
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
    let mut program = bynk_ts::TsProgram {
        stmts: program_stmts,
    };
    // 1A.36b-2: what the driver adds (`extra_imports`) and the boundary helpers bring in is
    // there for a body that may never reach it, so each import is kept only if the tree uses it.
    program.prune_unused_imports();
    // The generated `file` name: the source basename with `.bynk` → `.ts`.
    let generated_file = Path::new(source_name)
        .file_stem()
        .map(|s| format!("{}.ts", s.to_string_lossy()))
        .unwrap_or_else(|| "module.ts".to_string());
    let printed = bynk_ts::print(&program, source_name, source_text, &generated_file);
    let faults = ctx.runtime_use.take_faults();
    if faults.is_empty() {
        Ok((program, printed.source_map))
    } else {
        Err(faults)
    }
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
            let Some(bynk_check::checker::Callee::Intrinsic { op, .. }) =
                commons.callees.get(&e.id)
            else {
                return;
            };
            // #3214: `Yaml.decode[T]` decodes through the same codec helpers.
            match op {
                StaticOp::Json(JsonStatic::Decode | JsonStatic::DecodeSql)
                | StaticOp::Yaml(YamlStatic::Decode)
                | StaticOp::Form(FormStatic::Decode) => {
                    if let Some(Ty::Result(t, _)) = commons.expr_ty(e.id).as_deref() {
                        push_ground(*t, &mut roots);
                    }
                }
                StaticOp::Json(JsonStatic::Encode | JsonStatic::EncodePlain)
                | StaticOp::Form(FormStatic::Encode) => {
                    if let Some(a) = args.first()
                        && let Some(t) = commons.expr_types.get(&a.id).map(|te| te.ty)
                    {
                        push_ground(t, &mut roots);
                    }
                }
                StaticOp::List(_)
                | StaticOp::Map(_)
                | StaticOp::Set(_)
                | StaticOp::Int(_)
                | StaticOp::Float(_)
                | StaticOp::Decimal(_)
                | StaticOp::Duration(_)
                | StaticOp::Instant(_)
                | StaticOp::Bytes(_)
                | StaticOp::TokenClaims(_)
                | StaticOp::RequestHeaders(_)
                | StaticOp::RequestQuery(_)
                | StaticOp::RequestCookies(_)
                | StaticOp::Stream(_)
                | StaticOp::HttpResult(_)
                | StaticOp::QueueResult(_)
                | StaticOp::TestConnection(_)
                | StaticOp::TestRun(_) => {}
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
                CommonsItem::Type(_)
                | CommonsItem::Capability(_)
                | CommonsItem::Actor(_)
                | CommonsItem::Messages(_)
                | CommonsItem::Event(_)
                | CommonsItem::Asset(_)
                | CommonsItem::Embed(_)
                | CommonsItem::Const(_) => {}
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
/// `emit_consumed_context_helpers` just used.
fn emit_json_codec_helpers(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    skip_names: &HashSet<String>,
    skip_insts: &HashSet<String>,
) -> Vec<bynk_ts::TsStmt> {
    use serialisation::{collect_codec_closure, emit_generic_helpers, emit_helpers_for_owner};
    let mut roots = collect_json_codec_roots(commons);
    if roots.is_empty() {
        return Vec::new();
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
    stmts
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
            .filter_map(CommonsItem::as_service)
            .map(|s| (s.name.name.clone(), s.clone()))
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
        .filter_map(CommonsItem::as_agent)
        .map(|a| (a.name.name.clone(), a.clone()))
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
        .map(|n| (n.clone(), commons_alias(n)))
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
            CommonsItem::Type(_)
            | CommonsItem::Fn(_)
            | CommonsItem::Capability(_)
            | CommonsItem::Actor(_)
            | CommonsItem::Messages(_)
            | CommonsItem::Event(_)
            | CommonsItem::Asset(_)
            | CommonsItem::Embed(_)
            | CommonsItem::Const(_) => {}
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
        .filter_map(CommonsItem::as_service)
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
                    bynk_ts::TsType::named_with_args(commons_alias(name), generic_args),
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
                params: vec![bynk_ts::TsParam::new(
                    "value".to_string(),
                    Some(bynk_ts::TsType::named(ts_base_name)),
                )],
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
                            object: Box::new(bynk_ts::TsExpr::Ident(commons_alias(name))),
                            property: "of".to_string(),
                        }),
                        args: vec![bynk_ts::TsExpr::Ident("value".to_string())],
                    }),
                    None,
                )],
                span: None,
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
                    params: vec![bynk_ts::TsParam::new(
                        "value".to_string(),
                        Some(bynk_ts::TsType::named(ts_base_name)),
                    )],
                    return_type: Some(bynk_ts::TsType::named(name.clone())),
                    doc: None,
                    inline: true,
                    body: vec![bynk_ts::TsStmt::return_stmt(
                        Some(bynk_ts::TsExpr::Call {
                            callee: Box::new(bynk_ts::TsExpr::Member {
                                object: Box::new(bynk_ts::TsExpr::Ident(commons_alias(name))),
                                property: "unsafe".to_string(),
                            }),
                            args: vec![bynk_ts::TsExpr::Ident("value".to_string())],
                        }),
                        None,
                    )],
                    span: None,
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
                    init: bynk_ts::TsExpr::Ident(commons_alias(name)),
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
        TypeBody::Record(_) | TypeBody::Sum(_) => None,
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
    /// #1820: names imported from a sibling file as types only: a capability
    /// is an `interface`, which has no value to import once the module is
    /// stripped to JavaScript.
    types_by_sibling: HashMap<PathBuf, HashSet<String>>,
}

impl ExternalReferences {
    fn is_empty(&self) -> bool {
        self.by_commons.is_empty() && self.by_sibling.is_empty() && self.types_by_sibling.is_empty()
    }
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

/// Whether `name` is declared elsewhere than this file and so imported: a name of a used
/// commons, of a sibling file (a type, a function or a capability) or of a consumed context.
fn resolves_externally(name: &str, local_to_file: &HashSet<String>, ctx: &EmitProjectCtx) -> bool {
    let mut found = ExternalReferences::default();
    record_external_name(name, local_to_file, ctx, &mut found);
    !found.is_empty()
}

/// [`record_name_ref`], a sibling file's capability (an `interface`, imported as a type only,
/// #1820) and a sibling file's agent factory (`__make<Agent>`, which the file exports, #1820).
fn record_external_name(
    name: &str,
    local_to_file: &HashSet<String>,
    ctx: &EmitProjectCtx,
    out: &mut ExternalReferences,
) {
    record_name_ref(name, local_to_file, ctx, out);
    if let Some(agent) = name.strip_prefix("__make")
        && !local_to_file.contains(agent)
        && let Some(path) = ctx.file_decl_index.agents.get(agent)
        && path != &ctx.source_path
    {
        out.by_sibling
            .entry(path.clone())
            .or_default()
            .insert(name.to_string());
    }
    if !local_to_file.contains(name)
        && let Some(path) = ctx.file_decl_index.capabilities.get(name)
        && path != &ctx.source_path
    {
        out.types_by_sibling
            .entry(path.clone())
            .or_default()
            .insert(name.to_string());
    }
}

/// The names `body` uses and does not declare, that are not bound by `bound` (the module's other
/// imports) and are not the runtime's: the candidates for the type imports and the namespace
/// imports (1A.36b-2). A name a used commons' type is imported under (`__CommonsLine`,
/// `unbranded_commons_qual`) is read as the name it aliases.
///
/// A name that resolves to an import is never hidden by the global tables (`Request`/`Response`
/// of `bynk`).
fn used_external_names(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    body: &[bynk_ts::TsStmt],
    bound: &std::collections::BTreeSet<String>,
) -> std::collections::BTreeSet<String> {
    let local_to_file = local_names(commons);
    // A TypeScript type keyword (`any`) is never an import, even where a used commons has a
    // function of that name.
    let resolves = |n: &str| {
        !TS_TYPE_KEYWORDS.contains(&n)
            && resolves_externally(external_alias_target(n), &local_to_file, ctx)
    };
    bynk_ts::unbound_names_in(body, resolves)
        .into_iter()
        .filter(|n| !bound.contains(n))
        .collect()
}

/// The lower-case names TypeScript reads as types, which no import brings in.
const TS_TYPE_KEYWORDS: &[&str] = &[
    "any",
    "unknown",
    "never",
    "void",
    "string",
    "number",
    "boolean",
    "object",
    "symbol",
    "bigint",
    "null",
    "undefined",
];

/// The prefix of a used commons type's import alias.
const COMMONS_ALIAS_PREFIX: &str = "__Commons";

/// The import alias of a used commons' `X` (`__CommonsX`). The one spelling: the import, the
/// type alias and every value reference name it through here, so they cannot drift apart
/// (1A.39b, Decision C).
pub(crate) fn commons_alias(name: &str) -> String {
    format!("{COMMONS_ALIAS_PREFIX}{name}")
}

/// `__CommonsX` is the import alias of a used commons' `X`: the inverse of [`commons_alias`].
fn external_alias_target(name: &str) -> &str {
    name.strip_prefix(COMMONS_ALIAS_PREFIX).unwrap_or(name)
}

/// The imports the names `used` ask for, each resolved to its home by [`record_name_ref`].
fn external_references_of(
    used: &std::collections::BTreeSet<String>,
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> ExternalReferences {
    let local_to_file = local_names(commons);
    let mut refs = ExternalReferences::default();
    for name in used {
        record_external_name(external_alias_target(name), &local_to_file, ctx, &mut refs);
    }
    refs
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
    used: &std::collections::BTreeSet<String>,
) -> Vec<bynk_ts::TsStmt> {
    let info = &ctx.cross_context;
    // 1A.36b-2: a consumed context's namespace is imported iff the module's tree uses it
    // (`ns.__makeSurface`, `ns.Code`, `ns.Cap`).
    let mut candidates: std::collections::BTreeSet<String> =
        info.consumed_contexts.iter().cloned().collect();
    candidates.extend(info.consumed_services.keys().cloned());
    candidates.extend(cross_context_cap_namespaces(commons, ctx));
    let needed: std::collections::BTreeSet<String> = candidates
        .into_iter()
        .filter(|q| used.contains(&qualified_to_ns(q)))
        .collect();
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
            // #3807: the smallest path, so the output does not depend on hash order. Any
            // single file is still wrong for a context whose declarations span several.
            .and_then(|m| m.values().min().cloned())
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
        .filter_map(CommonsItem::as_service)
        .filter_map(|s| {
            let bynk_model::ProtocolIr::Events { event, .. } =
                bynk_model::lower_protocol_ir_from_commons(&s.protocol, commons)
            else {
                return None;
            };
            let Ty::Named { name, .. } = &*commons.tys().get(event) else {
                return None;
            };
            Some(name.clone())
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
                    parts.push(format!("{n} as {}", commons_alias(n)));
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
    stmts.extend(extra_import_stmts(ctx));
    // #1486: every statement in this function's own return, whatever
    // section it came from (sibling imports, cross-unit imports, the
    // rebrand-aliasing pair, `extra_imports`), sits directly under its
    // predecessor with no blank between them — this function never inserted
    // one under the pre-#1486 flat-print regime (confirmed: no explicit blank
    // `TsStmt` anywhere in its own body), so every internal adjacency is forced
    // regardless of `TsStmtKind`. The very first statement is left alone — its
    // own blank-or-not relative to whatever precedes this function's call
    // site is `emit_project`'s own call-site decision (the header/`emit_
    // project_imports` boundary, and the project/cross-context import
    // boundary, both forced explicitly there instead).
    for stmt in stmts.iter_mut().skip(1) {
        stmt.no_blank_before = true;
    }
    stmts
}

/// #527: imports the DO-side agent-deps expressions need (binding modules,
/// other Workers' handlers, the locale `render`). The project driver decides
/// which; each is an import node here.
fn extra_import_stmts(ctx: &EmitProjectCtx) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = Vec::new();
    for import in &ctx.extra_imports {
        let decl = match import {
            ExtraImport::Namespace { alias, from } => bynk_ts::TsDecl::ImportNamespace {
                type_only: false,
                alias: alias.clone(),
                from: from.clone(),
            },
            ExtraImport::Named {
                names,
                from,
                type_only,
            } => bynk_ts::TsDecl::Import {
                type_only: *type_only,
                names: names.clone(),
                from: from.clone(),
            },
        };
        stmts.push(bynk_ts::TsStmt::decl(decl, None));
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

/// A module's header: the two-line banner, and the runtime import its tree asks
/// for (1A.36b-1). `body` is every statement the module has besides the header,
/// built first, so the import is what the tree uses and cannot disagree with it
/// (see [`runtime_header`]). A unit the driver builds (`Test`, `Integration`)
/// gets the banner only: its imports are the driver's.
fn write_header(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    body: &[bynk_ts::TsStmt],
) -> Vec<bynk_ts::TsStmt> {
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
    if !commons.commons.items.is_empty() && runtime_header::derives_runtime_imports(ctx.unit_kind) {
        let names = runtime_header::derived_runtime_names(body);
        stmts.extend(runtime_files::runtime_import_stmts(
            false,
            &names,
            &runtime_import_for(&ctx.source_path, ctx.import_ext),
        ));
    }
    stmts
}

/// Variant of write_header for single-file (no project context) emission: the
/// same banner and the same derivation, over the module's built `body`. The banner and the
/// imports are statements of the one program the body joins (1A.39a), so the printer's top-level
/// spacing supplies the blank lines after each.
fn header_single(commons: &TypedCommons, body: &[bynk_ts::TsStmt]) -> Vec<bynk_ts::TsStmt> {
    // Arc C, slice 30 (#1392): the same 2-comment-statement conversion
    // `write_header` above already established.
    let mut stmts = vec![
        bynk_ts::TsStmt::comment("Generated by bynk — do not edit by hand.", None),
        bynk_ts::TsStmt::comment(
            format!(
                "commons {}",
                bynk_project::display_unit_name(&commons.commons.name.joined())
            ),
            None,
        ),
    ];
    if !commons.commons.items.is_empty() {
        let names = runtime_header::derived_runtime_names(body);
        let imports = runtime_files::runtime_import_stmts(false, &names, "./runtime.js");
        // A module with no runtime imports keeps the blank line its (empty) import block has
        // always left, beside the one after the banner. The printer supplies the second.
        if imports.is_empty() {
            let mut blank = bynk_ts::TsStmt::blank(None);
            blank.no_blank_before = true;
            stmts.push(blank);
        }
        stmts.extend(imports);
    }
    stmts
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
                    bynk_ts::TsDecl::ImportNamespace { .. }
                    | bynk_ts::TsDecl::ImportDefault { .. }
                    | bynk_ts::TsDecl::ReExport { .. }
                    | bynk_ts::TsDecl::ExportNames { .. }
                    | bynk_ts::TsDecl::ReExportAll { .. }
                    | bynk_ts::TsDecl::Export(_)
                    | bynk_ts::TsDecl::Interface { .. }
                    | bynk_ts::TsDecl::ConstDecl { .. }
                    | bynk_ts::TsDecl::Class { .. }
                    | bynk_ts::TsDecl::Function { .. }
                    | bynk_ts::TsDecl::TypeAlias { .. }
                    | bynk_ts::TsDecl::ExportDefault(_)
                    | bynk_ts::TsDecl::DeclareConst { .. } => unreachable!(),
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
    use crate::testkit::emit_source;

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
}
