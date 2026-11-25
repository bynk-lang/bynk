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
//! lives in `imports`, and the header (`runtime_import_for`) in `runtime_header`.
//! The project driver (`emit_project` and the agent and table helpers) lives in
//! `project`, the module's own boundary codecs (`ty_to_type_ref`) in `boundary`, and
//! the helpers for consumed contexts (`unexported_type_renames`, `rename_in_decl`)
//! in `consumed`. This module re-exports all of them. Anything not reachable from a path
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
use std::path::PathBuf;
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
use bynk_check::project_model::BuildTarget;
use bynk_model::TypeShape;
use bynk_model::{lower_store_field_shape_ir, lower_type_shape_ir};
use bynk_project::UnitKind;
use bynk_syntax::ast::{
    BaseType, BinOp, CommonsItem, Expr, ExprKind, FnDecl, FnName, Ident, LambdaExpr, MatchBody,
    TypeBody, TypeDecl, TypeRef, rounding_mode,
};
use ctx::UnitFacts;
use ts_types::TyRender;

pub mod bindings;
pub mod client;
pub mod commons_barrel;
pub(crate) mod compose;
pub mod containers;
pub mod contracts;
pub(crate) mod events_fanout;
pub(crate) mod harness_names;
pub mod http_client;
mod observability;
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
pub use observability::{observability_block, tuning_blocks};
pub use secrets::emit_secrets_manifest;
pub use toml_doc::print_toml_document;
pub use workers_entry::{
    emit_worker_entry, field_spec_source, form_body_specs, renamed_specs_source,
    table_has_plain_json,
};
pub use workers_group::{GroupEntry, GroupMember, emit_group_entry};
pub use wrangler::{ClassTombstones, emit_wrangler_toml};

mod boundary;
mod claim_predicate;
mod cmp;
mod codec;
mod consumed;
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
mod project;
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
use boundary::emit_json_codec_helpers;
pub use boundary::ty_to_type_ref;
pub(crate) use consumed::rename_in_decl;
pub use consumed::unexported_type_renames;
pub(crate) use imports::commons_alias;
pub use imports::{cross_commons_import_specifier_for_path, ts_specifier};
use imports::{
    emit_consumed_type_reexports, emit_context_rebrands, emit_cross_context_namespace_imports,
    emit_project_imports, external_references_of, extra_import_stmts, used_external_names,
};
use project::agent_registry_name;
pub(crate) use project::walk_unit_table_bodies;
pub use project::{
    agent_cross_context_targets, agent_factory_name, call_handler_needs_exec, emit_project,
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
        "\"outDir\": \"./out-js\",",
        "\"sourceMap\": true,\n    \"outDir\": \"./out-js\",",
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
    "outDir": "./out-js",
    "rootDir": "."
  },
  "include": ["**/*.ts"],
  "exclude": ["node_modules", "out-js", "**/bynk-client.ts", "**/*.bynk-client.ts"]
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

/// #3911: marks a route's typed `headers` record in the system drivers' route-body map, so
/// the record reaches a driver as its wire form, which the driver sends as request headers.
pub const HEADERS_ROUTE_SUFFIX: &str = "?headers";

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
        assert!(cov.contains("\"outDir\": \"./out-js\""));
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
