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
//! model lives in [`crate::emit_model`]. The import and rebrand family
//! (`ts_specifier`, `cross_commons_import_specifier_for_path`, `commons_alias`)
//! lives in `imports`, and the header (`runtime_import_for`) in `runtime_header`;
//! this module re-exports both. Anything not reachable from a path
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

#[cfg(test)]
use bynk_ts::SourceMapBuilder;
#[cfg(test)]
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

// P7.5 (#1307): relocated from `emitter/source_map.rs` to `bynk-ts`,
// unchanged — `bynk-ts/src/source_map.rs`'s own module doc has the full
// grounding for why this API stays as-is while `bynk-ts`'s own printer
// gets a second, simpler way to use the same type.
use bynk_ts::{
    TsType, decimal_signed_coef, pred_condition_and_message, pred_condition_and_message_for,
};

use crate::emit_model::EmitProjectCtx;
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
    MatchBody, MessagesDecl, ServiceDecl, TypeBody, TypeDecl, TypeRef, rounding_mode,
};
use ctx::UnitFacts;
use ts_types::{TyRender, asset_ts_type};

pub mod bindings;
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
pub mod workers_emit;
pub(crate) mod workers_entry;
pub mod workers_group;
pub mod wrangler;

pub use bindings::WorkerBindings;
pub use compose::bundle::emit_composition_root;
pub use compose::closure::{bare_singletons_of_context, native_platforms_of_context};
pub use compose::workers::emit_worker_compose;
pub use compose::{
    ComposeFacts, ComposePlan, ComposeUnit, ComposeUnitFacts, ConsumedSurface, EventPlan,
    EventSubscription, ExecNeeds,
};
pub use containers::{container_class_names, emit_containers_ts};
pub use events_fanout::{emit_events_fanout_do, emit_group_events_fanout_do};
pub use secrets::emit_secrets_manifest;
pub use toml_doc::print_toml_document;
pub use workers_entry::{
    emit_worker_entry, field_spec_source, form_body_specs, renamed_specs_source,
    table_has_plain_json,
};
pub use workers_group::{GroupEntry, GroupMember, emit_group_entry};
pub use wrangler::{ClassTombstones, emit_wrangler_toml};

mod claim_predicate;
mod cmp;
mod codec;
pub mod ctx;
mod default_value;
mod imports;
pub(crate) mod internal_fault;
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
pub(crate) use imports::commons_alias;
pub use imports::{cross_commons_import_specifier_for_path, ts_specifier};
use imports::{
    emit_consumed_type_reexports, emit_context_rebrands, emit_cross_context_namespace_imports,
    emit_project_imports, external_references_of, extra_import_stmts, used_external_names,
};
pub use runtime_files::{
    RUNTIME_DIR, closure_files, emit_all_runtime_modules, rewrite_runtime_imports,
};
pub use runtime_header::runtime_import_for;
use runtime_header::{header_single, write_header};

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

/// #2887: a codec helper of type `n` (`prefix` is `__serialise_` or
/// `__deserialise_`) as an import specifier: for a type this unit renames, the
/// helper the module exports for the real type, bound under the alias's name.
fn codec_import_spec(ctx: &EmitProjectCtx, prefix: &str, n: &str) -> String {
    match ctx.consumed_type_aliases.get(n) {
        Some(original) => format!("{prefix}{original} as {prefix}{n}"),
        None => format!("{prefix}{n}"),
    }
}

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
}
