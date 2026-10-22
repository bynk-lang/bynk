//! TypeScript emission (spec §7, v0.1 §6, v0.2 §6).
//!
//! Walks the typed AST and writes a single TypeScript module.
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
use bynk_ts::{SourceMapBuilder, TsType};

use crate::project::{BuildTarget, EmitProjectCtx, ImportExt, UnitKind, UnitTable};
use bynk_check::builtin_names::map_query;
use bynk_check::builtin_names::methods::{
    FOLD_EFF, FOR_EACH, PAR_TRAVERSE, PAR_TRAVERSE_ALL, PAR_TRAVERSE_TRY, RAW, TRAVERSE_ALL,
    TRAVERSE_TRY,
};
use bynk_check::builtin_names::types::*;
use bynk_check::checker::{CheckedProgram, ExprId, NamedKind, Ty, TyId, TypedCommons, Types};
use bynk_ir::TypeShape;
use bynk_ir::{block_uses_emit, walk_block_exprs};
use bynk_lower::{
    lower_capability_ops_ir, lower_protocol_ir, lower_service_handler_signature_ir,
    lower_store_field_shape_ir, lower_type_shape_ir,
};
use bynk_syntax::ast::{
    AgentDecl, BaseType, BinOp, Block, CommonsItem, Expr, ExprKind, FnDecl, FnName, Ident,
    InterpPart, MatchBody, MessagesDecl, ObservationMatcher, Param, Pattern, PredKind, ServiceDecl,
    Statement, TypeBody, TypeDecl, TypeRef, expr_children,
};

pub mod client;
pub mod contracts;
pub(crate) mod events_fanout;
pub mod secrets;
pub(crate) mod serialisation;
pub mod toml_doc;
pub(crate) mod workers;
pub(crate) mod workers_entry;
pub mod wrangler;

pub(crate) use events_fanout::emit_events_fanout_do;
pub(crate) use secrets::emit_secrets_manifest;
pub use toml_doc::print_toml_document;
pub(crate) use workers::emit_worker_compose;
pub(crate) use workers_entry::emit_worker_entry;
pub(crate) use wrangler::{ClassTombstones, emit_wrangler_toml};

mod lower;
pub(crate) mod runtime_use;
pub(crate) use lower::*;
pub(crate) use runtime_use::RuntimeUse;
pub(crate) mod emit;
pub(crate) use bynk_check::icu::{self, *};
pub(crate) use bynk_check::websocket;
pub(crate) use emit::*;

const INDENT_STEP: usize = 2;

/// Emit the contents of `out/runtime.ts`. This module ships with every
/// project so the per-context / per-test emissions can `import { Ok, Err,
/// Some, None, ... }` from a single source. It includes:
///
/// - `Result`/`Option` discriminated unions (using `tag` for the
///   discriminant — same shape user sum types lower to).
/// - `ValidationError` (the record shape refined-value constructors return).
/// - The `DurableObjectState`/`DurableObjectStorage` interfaces that agent
///   classes consume, plus an `InMemoryStorage` implementation and a
///   `makeTestState(name)` factory for use in test execution.
///
/// The content is identical across projects — there is no per-project
/// tailoring. Dead code is harmless; tsc handles it.
pub fn emit_runtime_module() -> String {
    RUNTIME_TS.to_string()
}

/// The embedded runtime. This is a BUILD OUTPUT, not a hand-edited file: it is
/// bundled from the focused TypeScript modules in `bynk-emit/runtime/src` by
/// that package's `scripts/bundle.mjs`. Edit the modules there and run
/// `npm run bundle` (CI's `runtime` job guards against drift); never edit this
/// file by hand. Keeping it a committed artifact means `cargo build` stays
/// Node-free and the emitter stays lockstep with the runtime it embeds.
const RUNTIME_TS: &str = include_str!("emitter/runtime.ts");

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
/// identical to this shape by hand; `cargo test -p bynkc --test
/// events_workers_wiring` and `events_envelope_behaviour` both exercise every
/// hop and would fail on a real mismatch.
///
/// #973: `runtime/src/boundary.ts`'s `deserialiseEventEnvelope` is a related
/// but distinct hand-written piece — it validates the *inner* `envelope`
/// object's shape at the receiving `/_bynk/event/` route, not this outer
/// wire wrapper. Keep its field list in sync with the `envelope: { ... }`
/// portion of this shape by hand; nothing generates either from the other.
pub(crate) const EVENTS_WIRE_EVENT_TS_TYPE: &str = "{ type: string; payload: unknown; envelope: { eventId: string; publisherId: string; emittedAt: number; schemaVersion: number } }";

/// Emit the contents of `out/tsconfig.json`. The CLI uses `tsc -p` against
/// this when running `bynkc test`; users can also drive `tsc` against it
/// directly to produce JS for deployment.
///
/// I4 (#1970): a context's generated TypeScript client (`bynk-client.ts`) is
/// excluded. It is for a foreign Worker's build, where `@bynk/wire-client`
/// resolves; the Bynk build neither imports it nor has that package.
/// Excluding a file drops the default `node_modules` exclusion, so it is
/// restated.
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

/// The `bynkc test --coverage` variant (#854): the same config with `sourceMap`
/// enabled, so `tsc` emits the `.js.map`s the coverage remap consumes (hop 1,
/// `.js` → emitted `.ts`). Kept coverage-only rather than folded into the
/// default so a normal `bynkc test` / deployment `tsc` run ships no `.js.map`s.
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
  "exclude": ["node_modules", "**/bynk-client.ts"]
}
"#;

/// Compute the runtime import specifier for a module at `from_source`. For a
/// file at `commerce/payment.ts` the runtime sits two levels up, so this
/// returns `../runtime.js`; for a top-level file it returns `./runtime.js`.
pub(crate) fn runtime_import_for(from_source: &Path, ext: ImportExt) -> String {
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
pub(crate) fn extend_printed_at(out: &mut String, stmts: Vec<bynk_ts::TsStmt>, depth: usize) {
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
    // Free functions afterward.
    for item in &commons.commons.items {
        if let CommonsItem::Fn(f) = item
            && let FnName::Free(_) = &f.name
        {
            body_stmts.extend(emit_free_fn(f, commons, false, &dummy_ctx.runtime_use));
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
        import_ext: crate::project::ImportExt::Js,
        contracts: false,
        source_path: PathBuf::new(),
        commons_name: String::new(),
        file_decl_index: crate::project::FileDeclIndex {
            types: HashMap::new(),
            fns: HashMap::new(),
            methods: HashMap::new(),
            agents: HashMap::new(),
            capabilities: HashMap::new(),
        },
        imported_from: HashMap::new(),
        imported_from_kind: HashMap::new(),
        imported_decl_paths: HashMap::new(),
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
        imported_methods: HashMap::new(),
        runtime_use: Default::default(),
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
pub(crate) fn emit_project(
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
    for item in &commons.commons.items {
        if let CommonsItem::Fn(f) = item
            && let FnName::Free(_) = &f.name
        {
            let mut item_stmts = emit_free_fn(f, commons, ctx.contracts, &ctx.runtime_use);
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
                let mut item_stmts = emit_capability(c, &ops, commons);
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
                let mut item_stmts = emit_agent(a, &state, commons, ctx);
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
        stmts.extend(emit_consumed_type_reexports(&references, ctx));
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

/// v0.79: does this block contain a `~>` send anywhere — including nested
/// branches, match arms, and lambdas? Gates execution-context threading
/// (`deps.__exec`) so a context that never sends keeps byte-identical output.
///
/// A `~>` send is a [`Statement`] variant, not an [`ExprKind`] one, and a bare
/// `{ … }` block is only parseable in a handful of positions (an `if`/`else`
/// body, a `match` arm, a lambda body) — never as an arbitrary sub-expression
/// — so `Block`/`If`/`Match`/`Lambda` were already the complete reachable set
/// and the old `_ => false` tail never actually dropped a send. It is
/// rewritten to recurse over `expr_children`, the total child iterator,
/// anyway: a `Statement`-only construct like this is exactly the shape that
/// silently drifts if a later `ExprKind` variant *does* start admitting a
/// nested block and this list isn't updated to match — see
/// `block_writes_state`, whose traversal was converted alongside this one for
/// the same reason. Both now also enumerate `ExprKind` explicitly instead of
/// ending in a `_` arm, so that drift is a build failure rather than a silent
/// miss.
///
/// A `match` arm's guard is walked too (#1769). A guard is an expression, so
/// it cannot hold a `~>` itself, but it can hold an `if` whose branch blocks
/// do (`Some(x) if if x > 0 { … } else { false } => …`, a `then` block that
/// sends and then yields `true`): the handler is effectful, so the send
/// type-checks, and missing it emitted
/// `deps.__exec.waitUntil(…)` against a `deps` that had no `__exec`.
pub(crate) fn block_uses_send(b: &Block) -> bool {
    fn stmt(s: &Statement) -> bool {
        match s {
            Statement::Send(_) => true,
            Statement::Let(l) | Statement::EffectLet(l) => expr(&l.value),
            Statement::Expect(a) => expr(&a.value),
            Statement::Do(d) => expr(&d.value),
            Statement::Assign(a) => expr(&a.value),
            Statement::Advance(a) => expr(&a.amount),
        }
    }
    fn expr(e: &Expr) -> bool {
        match &e.kind {
            ExprKind::Block(b) => block_uses_send(b),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => expr(cond) || block_uses_send(then_block) || block_uses_send(else_block),
            ExprKind::Match { discriminant, arms } => {
                expr(discriminant)
                    || arms.iter().any(|a| {
                        a.guard.as_ref().is_some_and(expr)
                            || match &a.body {
                                MatchBody::Expr(e) => expr(e),
                                MatchBody::Block(b) => block_uses_send(b),
                            }
                    })
            }
            // No variant below carries a `Block` *field*, so `expr_children`'s
            // total descent is complete for it — a block reached through a
            // child (a braced lambda body, say) comes back as an `Expr` and
            // re-enters this match at the `Block` arm above. A *new* variant
            // that holds a `Block` directly must be hand-matched up there
            // alongside `Block`/`If`/`Match`: appending it here loses the
            // `Statement::Send` tag (`expr_children` flattens a block to its
            // statements' values), and with it `deps.__exec` threading for a
            // context that does send.
            ExprKind::IntLit { .. }
            | ExprKind::FloatLit { .. }
            | ExprKind::DurationLit { .. }
            | ExprKind::StrLit(_)
            | ExprKind::InterpStr(_)
            | ExprKind::BoolLit(_)
            | ExprKind::Ident(_)
            | ExprKind::Call { .. }
            | ExprKind::Lambda(_)
            | ExprKind::BinOp(..)
            | ExprKind::UnaryOp(..)
            | ExprKind::Paren(_)
            | ExprKind::Ok(_)
            | ExprKind::Err(_)
            | ExprKind::Question(_)
            | ExprKind::RecordConstruction { .. }
            | ExprKind::FieldAccess { .. }
            | ExprKind::MethodCall { .. }
            | ExprKind::Is { .. }
            | ExprKind::Some(_)
            | ExprKind::None
            | ExprKind::UnitLit
            | ExprKind::RecordSpread { .. }
            | ExprKind::EffectPure(_)
            | ExprKind::Expect(_)
            | ExprKind::Val { .. }
            | ExprKind::Wire(_)
            | ExprKind::ListLit(_)
            | ExprKind::Observation(_)
            | ExprKind::Faults(_)
            | ExprKind::Trace { .. } => expr_children(e).into_iter().any(expr),
        }
    }
    b.statements.iter().any(stmt) || expr(&b.tail)
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
pub(crate) fn agent_cross_context_targets(
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
    JsonError,
    HttpResult,
    Connection,
    /// Closes the `ts_any` residual (#1319): `has_queue` (`write_header`'s
    /// own `QueueResult` import gate) only detected a real `from queue on
    /// message` consumer handler, never a bare field/type-declaration
    /// mention — the one gap among the four runtime-owned error types that
    /// `JsonError`/`HttpResult` already closed via this same marker
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
        TypeRef::JsonError(_) => marker == TypeRefMarker::JsonError,
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
        | TypeRef::List(a, _) => type_ref_mentions(a, marker),
        TypeRef::Fn(params, ret, _) => {
            params.iter().any(|p| type_ref_mentions(p, marker)) || type_ref_mentions(ret, marker)
        }
        // v0.157 (ADR 0183): recurse into a generic application's arguments.
        TypeRef::App { args, .. } => args.iter().any(|a| type_ref_mentions(a, marker)),
        TypeRef::QueueResult(_) => marker == TypeRefMarker::QueueResult,
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::Asset(_)
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
        CommonsItem::Actor(_) | CommonsItem::Messages(_) | CommonsItem::Asset(_) => false,
    })
}

/// v0.22b: whether any signature or type declaration in this file names
/// `JsonError` — drives the conditional `type JsonError` runtime import.
fn file_mentions_json_error(commons: &TypedCommons) -> bool {
    commons_mentions_type(commons, TypeRefMarker::JsonError)
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
        | CommonsItem::Asset(_) => false,
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
pub(crate) fn ty_to_type_ref(t: TyId, tys: &Arc<Types>) -> Option<TypeRef> {
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
        Ty::Map(k, v) => TypeRef::Map(
            Box::new(ty_to_type_ref(*k, tys)?),
            Box::new(ty_to_type_ref(*v, tys)?),
            sp,
        ),
        Ty::Unit => TypeRef::Unit(sp),
        Ty::ValidationError => TypeRef::ValidationError(sp),
        Ty::JsonError => TypeRef::JsonError(sp),
        // R4.3: `Ty::Error` has no codec — same as the other non-boundary
        // types below, but for the additional reason that a checked program
        // should never contain one at a codec-generation site.
        Ty::Error
        | Ty::Effect(_)
        | Ty::Query(_)
        | Ty::Stream(_)
        | Ty::Connection(_)
        | Ty::HttpResult(_)
        | Ty::HttpOutcome(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
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
        let mut visit = |e: &Expr| {
            let ExprKind::MethodCall { args, .. } = &e.kind else {
                return;
            };
            let Some(bynk_check::checker::Callee::Intrinsic { ns, op }) =
                commons.callees.get(&e.id)
            else {
                return;
            };
            if *ns != JSON {
                return;
            }
            match op.as_str() {
                "decode" => {
                    if let Some(Ty::Result(t, _)) = commons.expr_ty(e.id).as_deref()
                        && let Some(tr) = ty_to_type_ref(*t, tys)
                    {
                        roots.push(tr);
                    }
                }
                "encode" => {
                    if let Some(a) = args.first()
                        && let Some(t) = commons.expr_types.get(&a.id).map(|te| te.ty)
                        && let Some(tr) = ty_to_type_ref(t, tys)
                    {
                        roots.push(tr);
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
    let roots = collect_json_codec_roots(commons);
    if roots.is_empty() {
        return (Vec::new(), Vec::new());
    }
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
pub(crate) fn agent_call_boundary_roots(agents: &HashMap<String, AgentDecl>) -> Vec<TypeRef> {
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
        if crate::emitter::emit::agent_reads_key(&agents[name]) {
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
            agent_call_boundary_roots(&agents)
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
                    names: parts.clone(),
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
                .flat_map(|n| [format!("__serialise_{n}"), format!("__deserialise_{n}")])
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
        let bynk_ir::ProtocolIr::Events { event, .. } =
            bynk_lower::lower_protocol_ir(&svc.protocol, program)
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
            qual.insert(renamed.clone(), format!("{owner_ns}{ts_name}"));
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
/// names the codecs [`emit_consumed_context_helpers`] generates under the
/// same names. Computed once per consumed context, where the emitter's
/// `CrossContextInfo` is assembled, from the inputs the generator uses.
pub(crate) fn unexported_type_renames(
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
/// `handlers` too, the way a `uses`d commons type is through its rebrand
/// alias. Type-only (`export { type Batch }`): `compose.ts` uses the name in
/// type position only, a consumed refined alias has no value to re-export
/// (TS1205 under `isolatedModules`), and a value re-export would deepen the
/// caller's dependency on the callee's module. Only names the module imports
/// are re-exported, so each is bound here.
fn emit_consumed_type_reexports(
    refs: &ExternalReferences,
    ctx: &EmitProjectCtx,
) -> Vec<bynk_ts::TsStmt> {
    if ctx.target != BuildTarget::Workers {
        return Vec::new();
    }
    let mut names: Vec<String> = refs
        .by_commons
        .iter()
        .filter(|(unit, _)| !ctx.consumed_adapters.contains(unit.as_str()))
        .flat_map(|(_, set)| set.iter())
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
            // — which `bynkc check` accepts — fails `tsc`. The methods aren't in
            // this context's own `commons` (only imported *types* are merged);
            // they arrive via `ctx.imported_methods`, keyed by type name.
            if let Some(methods) = ctx.imported_methods.get(name) {
                entries.extend(emit_forwarded_methods(name, methods, &commons.ty_intern));
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
                for h in &s.handlers {
                    for p in &h.params {
                        collect_refs_in_typeref(&p.type_ref, &local_to_file, ctx, &mut refs);
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
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
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
            Statement::Send(s) => {
                collect_refs_in_expr(&s.value, local_to_file, commons, ctx, out);
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
        }
    }
    collect_refs_in_expr(&b.tail, local_to_file, commons, ctx, out);
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
            } else if commons.assets.contains_key(&id.name) {
                // #1883, slice 1b: a bare asset name (an `Asset` argument,
                // not just `style.url`, which the field-access arm already
                // records) imports the declaring module's constant.
                record_name_ref(&id.name, local_to_file, ctx, out);
            }
        }
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
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
        ExprKind::ListLit(elems) => {
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
        ExprKind::RecordConstruction { type_name, fields } => {
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
            args,
            ..
        } => {
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
        | Ty::Query(t)
        | Ty::Stream(t)
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
        | Ty::Base(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::ValidationError
        | Ty::JsonError
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
                    let bynk_ir::ProtocolIr::Events { event, .. } =
                        bynk_lower::lower_protocol_ir_from_commons(&s.protocol, commons)
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
                        BuildTarget::Workers => crate::project::worker_handlers_source_path(q),
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
                let bynk_ir::ProtocolIr::Events { event, .. } =
                    bynk_lower::lower_protocol_ir_from_commons(&s.protocol, commons)
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
                    parts.push(format!("type {}", ts_ident(n)));
                } else {
                    parts.push(ts_ident(n));
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
pub(crate) fn ts_specifier(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Compute a relative import specifier from this file's location to a
/// specific source file in another commons. `target_source` is the project-
/// relative path of the target `.bynk` file. The result is suitable for
/// `import { ... } from "..."` in NodeNext/strict TypeScript.
pub(crate) fn cross_commons_import_specifier_for_path(
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
        bynk_ts::TsStmt::comment("Generated by bynkc — do not edit by hand.", None),
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
                    bynk_lower::lower_handler_kind_ir(&h.kind),
                    bynk_ir::IrHandlerKind::Http { .. }
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
                    bynk_lower::lower_protocol_ir_from_commons(&s.protocol, commons),
                    bynk_ir::ProtocolIr::WebSocket { .. }
                ) && s.handlers.iter().any(|h| {
                    matches!(
                        bynk_lower::lower_handler_kind_ir(&h.kind),
                        bynk_ir::IrHandlerKind::Message
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
        // `Json` codec (or names `JsonError` in a signature) — keeping every
        // non-codec module's header byte-identical to v0.22a.
        let uses_codec = !collect_json_codec_roots(commons).is_empty();
        let mentions_json_error = file_mentions_json_error(commons);
        if uses_codec || mentions_json_error {
            parts.push("type JsonError");
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
        }
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
                    bynk_lower::lower_handler_kind_ir(&h.kind),
                    bynk_ir::IrHandlerKind::Open
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
                    bynk_lower::lower_protocol_ir_from_commons(&s.protocol, commons),
                    bynk_ir::ProtocolIr::WebSocket { .. }
                ) && s.handlers.iter().any(|h| {
                    matches!(
                        bynk_lower::lower_handler_kind_ir(&h.kind),
                        bynk_ir::IrHandlerKind::Message | bynk_ir::IrHandlerKind::Close
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
        stmts.push(bynk_ts::TsStmt::decl(
            bynk_ts::TsDecl::Import {
                type_only: false,
                names: parts.into_iter().map(str::to_string).collect(),
                from: runtime_import,
            },
            None,
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
        &bynk_ts::TsStmt::comment("Generated by bynkc — do not edit by hand.", None),
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
        let uses_codec = !collect_json_codec_roots(commons).is_empty();
        let codec_imports = if uses_codec {
            ", type JsonError, type __JsonValue, type __BoundaryError"
        } else if file_mentions_json_error(commons) {
            ", type JsonError"
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
            "Ok, Err, Some, None, type Result, type Option, type ValidationError{codec_imports}{bytes_imports}{bytes_ops_imports}{eq_imports}{int_imports}{http_imports}{queue_imports}"
        );
        out.push_str(&bynk_ts::print_stmt(
            &bynk_ts::TsStmt::decl(
                bynk_ts::TsDecl::Import {
                    type_only: false,
                    names: inside.split(", ").map(str::to_string).collect(),
                    from: "./runtime.js".to_string(),
                },
                None,
            ),
            0,
        ));
        writeln!(out).unwrap();
    }
}

/// v0.110 (ADR 0142): the `Bytes` runtime helpers, appended to a module's
/// import list when the emitted body references them. `bytesEqual` backs `==`;
/// the base64/UTF-8 helpers back the kernel and codec.
pub(crate) const BYTES_RUNTIME_IMPORTS: &str =
    ", __bynkBytesEqual, __bynkBytesToBase64, __bynkBytesFromBase64, __bynkBytesDecodeUtf8";

/// #2016: the `Bytes` operation helpers (`concat`, `compare`, hex, base64url),
/// appended only when the body calls one, so a module that merely builds or
/// compares `Bytes` keeps the [`BYTES_RUNTIME_IMPORTS`] list it had.
pub(crate) const BYTES_OPS_RUNTIME_IMPORTS: &str = ", __bynkBytesConcat, __bynkBytesCompare, __bynkBytesToHex, __bynkBytesFromHex, __bynkBytesToBase64Url, __bynkBytesFromBase64Url";

/// #1652 (runtime-semantics track §3.1): the structural-equality walker,
/// appended to a module's import list when `==`/`!=` on a non-primitive,
/// non-`Bytes` operand lowered to it (`lower_bin_op`).
pub(crate) const EQ_RUNTIME_IMPORTS: &str = ", __bynkEq";

/// #1657: the `Int` domain traps (`bynk-emit/runtime/src/int.ts`).
pub(crate) const INT_RUNTIME_IMPORTS: &str = ", __bynkIntDiv, __bynkToInt";

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
pub(crate) const BOUNDARY_CODEC_RUNTIME_IMPORTS: &str =
    ", Ok, Err, type Result, type __BoundaryError";

/// #914: the names the `Json.decode[T]` wrapper puts in the module — its own
/// `Result<T, JsonError>` signature and the `JsonValue` it parses into
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
pub(crate) const JSON_CODEC_RUNTIME_IMPORTS: &str =
    ", Ok, Err, type Result, type __JsonValue, type JsonError";

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
pub(crate) fn agent_factory_name(agent: &str) -> String {
    format!("__make{agent}")
}

/// Lowering state that is genuinely **module-invariant**: the same value at
/// every body-lowering site within one emitted module, and never written by the
/// recursive lowering itself. Grouped out of [`LowerCtx`] so a new lowering kind
/// inherits the whole set wholesale instead of re-deriving each default.
///
/// Nothing the lowering mutates *per body* may move in here — see the
/// scratch-state fields at the bottom of [`LowerCtx`]. A `ModuleCtx` is still
/// built fresh alongside each `LowerCtx` today, but filing a per-body counter
/// under a name that says "module" is exactly how such state starts leaking
/// between handler bodies.
pub(crate) struct ModuleCtx<'a> {
    /// Typed-commons handle (used to look up receiver types for method-call
    /// UFCS lowering).
    commons: &'a TypedCommons,
    /// Cross-context info for v0.6 cross-context call lowering.
    cross_context: &'a bynk_check::resolver::CrossContextInfo,
    /// The emitted module's conditional-runtime-helper accumulator.
    ///
    /// The `Bytes` lowerings (kernel, `==`, base64 codec) call
    /// [`RuntimeUse::note_bytes`] through this, so the module's import line is
    /// decided from what lowering actually emitted rather than by scanning the
    /// generated text for the helper's own name.
    ///
    /// Required rather than optional: a missing accumulator would make
    /// `note_bytes` a silent no-op, which is exactly the failure this replaces —
    /// a module that references `__bynkBytesEqual` without importing it. A
    /// lowering whose imports are decided elsewhere (the test scaffolds) owns a
    /// throwaway one, which reads as the deliberate choice it is.
    runtime_use: &'a RuntimeUse,
    /// v0.8 build target. In workers mode cross-context calls lower to
    /// `callService(...)` instead of `deps.surface.<key>.<method>(...)`.
    target: BuildTarget,
    /// #527: agent → method → the method's `given` caps (mirrors
    /// [`crate::project::EmitProjectCtx::agent_method_givens`]). Consulted by
    /// the agent-call lowering to record capability requirements.
    agent_method_givens: HashMap<String, HashMap<String, Vec<bynk_ir::CapRefIr>>>,
    /// Events slice 3b (#978): each locally-declared event's resolved
    /// `@schema(N)` version (mirrors
    /// [`crate::project::EmitProjectCtx::event_schema_versions`]). Default-
    /// empty like `agent_method_givens`, not a required constructor
    /// parameter like `runtime_use` — a miss here degrades to `schemaVersion:
    /// 1`, exactly every event's behaviour before this map existed, not a
    /// hard failure the way a missing `runtime_use` would be.
    event_schema_versions: HashMap<String, i64>,
    /// #2058: the namespace each consumed context's type is named under in
    /// this module (mirrors [`crate::emitter::emit::consumed_type_ns`]).
    /// Default-empty: a miss names the type bare, as before.
    consumed_type_ns: HashMap<String, String>,
    /// #934: true when the unit being emitted is the reserved first-party
    /// `bynk` adapter itself. `bynk` is a reserved namespace, so a capability
    /// literally named `Idempotency` declared *in this unit* is unambiguously
    /// the real one — used alongside `CrossContextInfo::flattened_caps` (the
    /// consumed-from-elsewhere case) to confirm a flattened `Idempotency`
    /// call is genuinely first-party before scoping its key, not a same-named
    /// capability some other adapter or context happens to declare.
    in_bynk_unit: bool,
    /// #1961: [`crate::project::EmitProjectCtx::agent_called_contexts`].
    agent_called_contexts: Vec<String>,
    /// #2045: [`crate::project::EmitProjectCtx::consumed_context_types`]. A
    /// variant of one of these sums is built inline, not through the callee's
    /// companion object, so the callee's module is never a value import.
    consumed_context_types: HashSet<String>,
}

impl<'a> ModuleCtx<'a> {
    fn new(
        commons: &'a TypedCommons,
        cross_context: &'a bynk_check::resolver::CrossContextInfo,
        runtime_use: &'a RuntimeUse,
    ) -> Self {
        Self {
            commons,
            cross_context,
            runtime_use,
            target: BuildTarget::Bundle,
            agent_method_givens: HashMap::new(),
            event_schema_versions: HashMap::new(),
            consumed_type_ns: HashMap::new(),
            in_bynk_unit: false,
            agent_called_contexts: Vec::new(),
            consumed_context_types: HashSet::new(),
        }
    }
}

/// The lowering state shared by the four **capability-bearing** body kinds — a
/// service handler, a composed provider op, an agent handler, and a websocket
/// lifecycle DO method. Every other kind carries no `HandlerShared` at all, and
/// the [`LowerCtx`] accessors below hand those kinds the same defaults the flat
/// struct used to give them (an empty capability set, no scope, `deps`).
pub(crate) struct HandlerShared {
    /// Names of capabilities in scope as `given C1, C2, ...`. Used to lower
    /// `Capability.op(args)` calls to `deps.Capability.op(args)`.
    capabilities: HashSet<String>,
    /// #934: the calling handler's own qualified name (`<unit>.<service or
    /// agent>.<handler>`, e.g. `shop.reserve.ordering.call`). Read only by the
    /// `Idempotency.dedup`/`remember` lowering, which prefixes the
    /// developer-supplied key with it so two unrelated handlers using the same
    /// literal key never collide (design/tracks/idempotency-capability.md
    /// §3.4). `None` anywhere a capability call cannot occur (a plain method, a
    /// free fn, an invariant/transition predicate, a static field initialiser) —
    /// those kinds carry no `HandlerShared`, and the accessor reports `None`.
    handler_scope: Option<String>,
    /// Events track, slice 2 (spine #936): the qualified name of the unit
    /// this body is emitted into (`ctx.commons_name`), read by the
    /// `Events.emit[E](event)` lowering to mint the envelope's
    /// `publisherId`. Context-scoped rather than agent-scoped: `Events.emit`
    /// is legal from a plain, keyless service handler with no agent
    /// instance to report, so this is the only identity available
    /// uniformly at every legal emission site (an amendment to
    /// design/bynk-design-notes.md §7's "the publisher is the emitting
    /// agent" framing — see the events-envelope ADR). Always populated
    /// alongside `handler_scope` at every construction site; never `None`
    /// in practice for a body that could contain an `Events.emit` call.
    owning_context: String,
    /// v0.12: the receiver expression a capability call resolves against —
    /// `deps` in a handler body, `this.deps` in a composed provider body.
    cap_deps_expr: String,
    /// The consumed contexts the current handler calls across the boundary.
    /// Non-empty drives whether `deps` gets a `surface` field type; in
    /// workers mode it is also exactly the Service Bindings the handler's
    /// `deps.env` type names (#1961).
    cross_contexts_called: std::collections::BTreeSet<String>,
    /// v0.9.2: set when the body instantiates a local agent. In workers mode
    /// this drives `env` (carrying the DO namespaces) into the handler's deps
    /// type so the agent factory can reach its Durable Object binding.
    agents_instantiated: bool,
    /// #527: capabilities required by agent methods this body calls, keyed by
    /// deps key. After body lowering these widen the handler's deps *type* to
    /// match the runtime value compose builds (which always carried them).
    agent_given_caps_used: std::collections::BTreeMap<String, bynk_ir::CapRefIr>,
}

impl Default for HandlerShared {
    fn default() -> Self {
        Self {
            capabilities: HashSet::new(),
            handler_scope: None,
            owning_context: String::new(),
            cap_deps_expr: "deps".to_string(),
            cross_contexts_called: std::collections::BTreeSet::new(),
            agents_instantiated: false,
            agent_given_caps_used: std::collections::BTreeMap::new(),
        }
    }
}

/// The lowering state shared by the three **generated test-scaffold** body
/// kinds — a `stub`/`where`/`requires` predicate value, a unit test case, and an
/// integration test case.
#[derive(Default)]
pub(crate) struct TestShared {
    /// True when lowering **any** generated test-scaffold body — distinct from
    /// `assert_loc`, which is only ever `Some` for the two real `case` bodies
    /// and carries an unrelated payload (a diagnostic location). Kept as its own
    /// field (Locale capability track, slice 1, #844 review) rather than
    /// overloading `assert_loc.is_some()`, since that conflated "has a location"
    /// with "is test scaffolding" for a caller that has no location to give.
    test_scaffold: bool,
    /// v0.59: the source text and project-relative path of the file the body
    /// came from, so an `assert` can emit a real `path:line:col` location (for
    /// `--format json` click-through) rather than a bare byte offset. Stays
    /// `None` for a predicate scaffold, which emits no `assert`.
    assert_loc: Option<AssertLoc>,
}

/// The `store`-agent sub-state of an [`BodyMode::AgentHandler`] body: present
/// only when the hosting agent is a `store` agent, absent for a plain
/// state-record agent (whose handler reads `currentState`/`self.state` instead).
pub(crate) struct AgentStoreState {
    /// v0.81 (storage track): the name of the mutable working-state variable
    /// (`__state`) and the set of `Cell` field names. A bare `Cell` read lowers
    /// to `<var>.<cell>`, and a `cell := v` write lowers to `<var>.<cell> = <v>`
    /// — read-your-writes via the in-memory record, flushed once at handler end
    /// (ADR 0109).
    state: (String, HashSet<String>),
    /// v0.82 (ADR 0110): the agent's `store` `Map` field names. A method call
    /// whose receiver is one lowers to an entry operation over `__state.<map>`
    /// (a JSON-serialisable `Record<string, V>`), staged in the working record
    /// and flushed at commit like any other state field.
    maps: HashSet<String>,
    /// v0.83: the agent's `store` `Set` field names. A method call whose
    /// receiver is one lowers to an entry operation over `__state.<set>` (a
    /// `Record<string, boolean>`), staged in the working record.
    sets: HashSet<String>,
    /// v0.87 (ADR 0113): the agent's `store` `Cache` fields (name → ttl millis).
    /// A method call whose receiver is one lowers to an entry op over
    /// `__state.<cache>` (a `Record<string, { v, exp }>`), applying TTL expiry
    /// against the injected `Clock`.
    caches: HashMap<String, i64>,
    /// v0.95 (ADR 0121): the agent's `store` `Log` fields (name → optional
    /// `@retain` millis). `<log>.append` pushes `{ t: now(), v }` to
    /// `__state.<log>` (an array) and prunes past the retain horizon; the
    /// time-window roots / builders lower to a query pipeline over the array.
    logs: HashMap<String, Option<i64>>,
    /// v0.93 (ADR 0118): the agent's `@indexed` secondary indexes (map name →
    /// the value-record fields indexed on). A mutating op on the map maintains a
    /// sibling posting-list `Record<string, string[]>` per field (`<map>__idx_<f>`);
    /// an equality `filter` on an indexed field routes to a posting lookup.
    indexes: HashMap<String, Vec<String>>,
    /// v0.104/v0.105 (real-time track slice 3b): the agent's held `store Map[K,
    /// Connection]` fields (name → the connection's **frame type** `F`, e.g.
    /// `ServerFrame`). On Workers these persist `K → connId` in the durable state
    /// record; a method call whose receiver is one lowers to an entry op over
    /// `__state.<map>` (the connId record) with `connIdOf`/`resolveConnection<F>` —
    /// not the plain `Record<string, V>` ops (held maps are excluded from
    /// [`AgentStoreState::maps`]).
    held_maps: HashMap<String, String>,
    /// Review of #1460: a plain `store Map[K, V]` field's own value type `V`
    /// (name → its rendered TS type), for `lower_query_method`'s own
    /// collection-kernel sites (`distinctBy`/`joinOn`/`leftJoin`/`groupBy`)
    /// when the receiver is a plain map's own value scan lifted into the
    /// general query vocabulary — mirrors `held_maps`'s own construction
    /// exactly, just for the non-held case.
    map_values: HashMap<String, String>,
    /// Review of #1460: a `store Log[T]` field's own element type `T` (name →
    /// its rendered TS type), for the identical reason `map_values` exists —
    /// a Log's value scan is also lifted into the general query vocabulary.
    log_values: HashMap<String, String>,
}

/// What [`LowerCtx`] is lowering *right now*. One variant per real body-emission
/// site; each carries exactly the state that site populates and nothing else, so
/// "not applicable to this kind" is expressed in the type rather than left
/// indistinguishable from "deliberately defaulted".
pub(crate) enum BodyMode {
    /// A type's method body (`emit_method`).
    Method,
    /// A free function body (`emit_free_fn`).
    FreeFn,
    /// An agent field's static initialiser expression (`emit_agent`).
    StaticInit,
    /// v0.80: an agent invariant predicate. Carries the name of the
    /// proposed-state variable (the `commitState` parameter) and the set of
    /// state field names — a bare ident matching a state field lowers to
    /// `<var>.<field>`, since invariants read state fields directly (§14).
    Invariant {
        name: String,
        fields: HashSet<String>,
    },
    /// v0.116 (testing track slice 4): a `transition` predicate. Carries the JS
    /// names bound to the contextual `old` and `new` state records. The Bynk
    /// identifiers `old`/`new` lower to these (`new` is a JS reserved word, so
    /// both are renamed), and field access `old.<field>` reads off the `old`
    /// record.
    Transition { old: String, new: String },
    /// A service handler body (`emit_service`).
    ServiceHandler {
        handler: HandlerShared,
        /// v0.47: the `by` binder whose `.identity` is threaded through `deps`
        /// (so `<binder>.identity` lowers to `deps.identity` rather than the
        /// unit-value `undefined`).
        deps_identity_binder: Option<String>,
        /// v0.52: when lowering a multi-actor sum handler body, the `by` binder
        /// that names the resolved-actor value (threaded through `deps`, so the
        /// binder ident lowers to `deps.who` — the tagged union the body
        /// `match`es).
        actor_sum_binder: Option<String>,
    },
    /// A composed provider's operation body (`emit_provider`).
    ProviderOp { handler: HandlerShared },
    /// An agent handler body (`emit_agent`).
    AgentHandler {
        handler: HandlerShared,
        /// True when lowering an agent handler body. Used to rewrite
        /// `self.<keyField>` access into the appropriate local.
        in_agent_handler: bool,
        /// The name of the agent's `key id` field (so `self.<id>` resolves).
        agent_key_field: Option<String>,
        /// The `store`-agent working-record state, when the hosting agent is a
        /// `store` agent. Boxed: it is by far the largest payload in this enum,
        /// and every other body kind would otherwise pay for it.
        store: Option<Box<AgentStoreState>>,
    },
    /// A websocket lifecycle method on the hosting Durable Object
    /// (`emit_ws_do_method`).
    WsDoMethod {
        handler: HandlerShared,
        /// v0.47: as [`BodyMode::ServiceHandler::deps_identity_binder`].
        deps_identity_binder: Option<String>,
        /// v0.104 (real-time track slice 3b): when lowering a `from websocket`
        /// `on open` body **into its hosting Durable Object** (the agent the
        /// upgrade transfers the connection to), the name of that agent. A
        /// transfer call `<Agent>(<key>).method(args)` whose `<Agent>` is this
        /// self-agent lowers to a direct `this.method(args, deps)` self-call
        /// rather than the cross-instance `__make<Agent>(key)` factory — the
        /// connection is already in this DO, so it never crosses an RPC boundary
        /// (DECISION A).
        ws_self_agent: Option<String>,
    },
    /// A `stub`/`where`/`requires` predicate value lowered via
    /// `lower_block_to_async_body` — test/property/contract scaffolding, never a
    /// real production provider body.
    PredicateScaffold { test: TestShared },
    /// A unit test `case` body (`lower_test_case_body`).
    TestCase {
        test: TestShared,
        /// v0.117 (testing track slice 5): the name of the recorded-call trace
        /// object (`__obs`), over which an observation (`Cap.op called …`) and
        /// `trace(Cap.op)` are lowered.
        observation_trace: Option<String>,
        /// v0.7: the target context's local service names. A `service.call(args)`
        /// or `service(args)` invocation where `service` is in this set lowers to
        /// `<service>.call(args, deps)` so the test wires its `deps` through.
        test_services: HashSet<String>,
        /// v0.182 (#664): the ordered handler kinds of each test service, so a
        /// cron (`svc.schedule("…")`) or queue (`svc.message(m)`) address can
        /// recover the position index the emitted key encodes (`cron_<svc>_<i>` /
        /// `queue_…`). http keys are a pure function of verb + path and need no
        /// lookup here. P6.37 (design/tracks/the-ir.md §6a): `IrHandlerKind`
        /// (P6.24a's own pure, unconditional mirror), not the raw AST
        /// `HandlerKind` this field used to store.
        test_service_handlers: HashMap<String, Vec<bynk_ir::IrHandlerKind>>,
        /// I3 (#1971), #2020: the `(service, key)` handlers that bind an actor
        /// sum, whose `deps.who` a call-site principal builds. `key` is
        /// `"call"` for an `on call` handler and the emitted method name
        /// (`http_handler_method_name_ir`) for an http route.
        sum_handlers: HashSet<(String, String)>,
        /// #1994: the target has a handler that sends with `~>`, so `deps`
        /// carries the `__exec` double and each addressed service call is
        /// wrapped in `__bynkSettle(deps, …)`.
        settle_sends: bool,
    },
    /// An integration test `case` body (`lower_integration_case_body`).
    IntegrationCase {
        test: TestShared,
        /// v0.182 (Slice B, #667): the target's http service names. An http
        /// address on one of these lowers to a driver call
        /// (`__sysdrive_<svc>_<key>(args, sub)`) that drives a real
        /// `worker.fetch` with a signed credential, instead of the unit-tier
        /// direct handler call. Empty at the unit tier.
        system_http_services: std::collections::HashSet<String>,
        /// #707: the declared `(service, method, path)` http routes of the system
        /// target. A `(method, path)` call whose method is absent here but whose
        /// path is present is a **wrong-method** call — it drives the `405`
        /// fall-through through the generic `__sysdrive_wrongmethod_<svc>` driver.
        system_http_routes: std::collections::HashSet<(String, String, String)>,
        /// #708: for each declared `(service, method, path)` route that has a
        /// body param, the body's zero-based position among the route's
        /// positional call args (i.e. within `args[1..]`, matching handler-param
        /// declaration order) and its declared type. The raw driver
        /// (`__sysdrive_raw_*`, Slice C) forwards every slot as a `string`; a
        /// `Wire(…)` arg already lowers to that raw string, but a *typed* arg
        /// mixed into the same call must be converted: the body slot serialises
        /// through the same wire codec the typed driver uses
        /// (`JSON.stringify(serialise_expr_via(...))`), any other (path) slot
        /// just coerces via `String(...)`. Absent for a bodyless route.
        system_http_route_body:
            HashMap<(String, String, String), (usize, bynk_syntax::ast::TypeRef)>,
        /// #708: the type namespace (`<target>.`) `serialise_expr_via` needs to
        /// resolve a body param's custom codec when converting a typed arg for
        /// the raw driver. Mirrors the `type_ns` `emit_system_http_support`
        /// computes from the same suite target.
        system_http_type_ns: String,
        /// #2019: the refinement-guarded routes and the claims each
        /// refinement actor signs, for a call acting as a refinement.
        system_http_claims: Box<SystemHttpClaims>,
        /// #2136: the target's cron and queue services, driven at `system`
        /// through the Worker's `scheduled` / `queue` entry points.
        system_jobs: SystemJobServices,
    },
}

/// #2136: the cron and queue services a `system`-tier case drives through the
/// participant Worker's real `scheduled` and `queue` entry points. A
/// `svc.schedule("<expr>", …)` address on a cron service here lowers to
/// `__sysdrive_schedule_<svc>_<index>(…)`, and `q.message(m)` on a queue
/// service to `__sysdrive_message_<q>(m)` (`tests_emit::emit_system_job_support`).
#[derive(Debug, Clone, Default)]
pub struct SystemJobServices {
    /// Each cron service's schedule expressions, in declaration order (the
    /// position is the handler's `cron_<svc>_<index>` index).
    pub cron: HashMap<String, Vec<String>>,
    /// The queue services.
    pub queue: HashSet<String>,
}

/// #2019: what a `system`-tier call needs to sign the claims a refinement
/// actor's predicate checks (`actor Admin = User where hasClaim("admin")`).
/// A route guarded by a refinement has drivers that take a trailing claims
/// object, signed into the JWT next to `sub`; a call acting as a refinement
/// actor passes that actor's witness claims, and any other call passes `{}`,
/// so the Worker's seam answers `403` exactly as it would a real token
/// lacking the claim.
#[derive(Debug, Clone, Default)]
pub struct SystemHttpClaims {
    /// The `(service, method, path)` routes whose `by` actor is a refinement.
    pub routes: HashSet<(String, String, String)>,
    /// Each refinement actor's witness claims, as a JavaScript object literal
    /// (`bynk_check::actors::claim_witness_js`).
    pub actors: HashMap<String, String>,
    /// #2155: each actor-sum route with a credentialed member, and its
    /// primary member — the actor a call with no principal acts as. Its
    /// signing drivers take the acting actor's name.
    pub sum_routes: HashMap<(String, String, String), String>,
}

/// Per-body lowering context: what module we are emitting into ([`ModuleCtx`]),
/// what kind of body we are lowering ([`BodyMode`]), and the scratch state the
/// recursive lowering accumulates as it goes.
///
/// Everything below `mode` is deliberately **not** in `ModuleCtx`: a fresh
/// `LowerCtx` is built at every body-emission site and never reused across two
/// bodies, so these are implicitly reset per body today. Moving any of them up a
/// level would leak state between handlers in the same module — most visibly the
/// `next_tmp` counter, which would stop restarting `__r0` at each function and
/// so rename every generated temp in the emitted TypeScript.
pub(crate) struct LowerCtx<'a> {
    module: ModuleCtx<'a>,
    mode: BodyMode,
    /// Agent names declared in the surrounding context. Drives lowering of
    /// `Agent(key)` (to `new Agent(makeTestState(String(key)))`) and of
    /// `agent_instance.method(args)` (to `instance.method(args, deps)`) in
    /// service and agent-handler bodies. Populated by the caller for non-test
    /// emission and from the *test's own* agent set in test emission — which is
    /// why this is not a [`ModuleCtx`] field despite being module-wide at the
    /// nine non-test sites.
    pub local_agents: HashSet<String>,
    /// v0.154 (ADR 0178): the enclosing function/handler's resolved return type,
    /// set at each body-emission site that has one. The `?` lowering reads it to
    /// decide whether a declared error embedding (`embeds E as V`) converts the
    /// propagated `Err` — via the same `embedding_for` rule the checker used.
    /// Genuinely cross-cutting rather than kind-specific: it is saved/restored
    /// around lambda bodies, `?`-embedding and match arms *within* whichever
    /// kind is being lowered.
    return_ty: Option<bynk_check::checker::TyId>,
    next_tmp: u32,
    /// #908: a stack of per-block frames tracking `let`/`let <-` names that
    /// needed a fresh emitted identifier because the name was already bound
    /// by an enclosing (or the same) block's `let` — the checker allows
    /// re-binding a name (`let x = 1; let x = x + 1`, a deliberate ML-family
    /// idiom, ADR 0064), but each `let` still lowers to its own `const`, so
    /// without renaming a same-block re-`let` collides with the first
    /// (TS2451), and a nested block's re-`let` — while a legal *redeclaration*
    /// on its own — would put an RHS read of the outer binding in its own
    /// declaration's temporal dead zone. Pushed/popped in lock-step with
    /// [`emit_block_inner`] — the single choke point every block (function,
    /// lambda, if/else branch, match arm) lowers through — so a read
    /// (`lower_ident`, and the agent-dispatch receiver text) resolves a name
    /// by walking the stack innermost-out and falls back to the natural
    /// `ts_ident` name when no frame renamed it.
    pub shadow_scopes: Vec<HashMap<String, String>>,
    /// #2024: identifiers the generated scaffold around this body already
    /// binds in the body's own JS scope — a test runner's `deps`, `__obs`, and
    /// the target's destructured services, agents, functions and assets. A
    /// `let` whose natural name is one of these gets a fresh name from
    /// [`Self::bind_local_name`], as a re-`let` does: the scaffold binding is
    /// not a Bynk local (so [`Self::is_local`] ignores it), but a second
    /// `const` of its name in that scope is `TS2451` under `tsc`.
    pub harness_bound: HashSet<String>,
    /// When an `is` receiver is not a simple, repeatable lvalue (e.g. a call
    /// like `parse(x) is Ok(n)`), it is evaluated once into a temp; the temp
    /// name is cached here keyed by the receiver expression's span so the
    /// `.tag` check and every pattern binding reference the *same* single
    /// evaluation. Simple receivers (idents / field chains) are never cached
    /// and continue to be rendered inline as before.
    is_receiver_temps: HashMap<bynk_syntax::span::Span, String>,
    /// #1654: while the right operand of an `is`-binding `&&`/`implies` is
    /// lowered (inside the IIFE that keeps it lazy), an `is` receiver temp is
    /// *declared* here (`let __rN!: T;`) and only *assigned* in place. The
    /// caller emits these declarations before the whole condition, so an `if`
    /// then-branch's binding that reads the temp (`const m = __r1 as Q;`)
    /// finds it in scope. `None` outside such a right operand. #1751: also set
    /// around the right operand of a plain `&&`/`||`/`implies`.
    pub(crate) is_temp_hoist: Option<Vec<String>>,
    /// #1752 (generalising #1751): the `is` tests, by span, whose tag test was
    /// emitted out of line: inside a short-circuit right operand that lowered
    /// to an arrow or a hoisted `if`. TypeScript cannot carry such a test's
    /// narrowing to a binding read elsewhere, so `emit_is_test_bindings` reads
    /// those bindings through a cast to the variant the checker proved.
    pub(crate) unnarrowed_is_tests: HashSet<bynk_syntax::span::Span>,
    /// Variable bindings that point at agent instances. Updated by the
    /// statement emitter when it sees `let x = AgentName(key)`. Used by
    /// the method-call lowering so `x.method(args)` resolves through
    /// the agent's class rather than via the receiver-namespace lookup.
    pub local_agent_vars: HashMap<String, String>,
    /// v0.182 (#664): while lowering an `EffectLet` whose value addresses a
    /// service handler, the call-site principal's identity expression (already
    /// lowered), if the statement carries `by <Actor>(<identity>)`. The
    /// address-call lowering reads it to build the handler's `deps.identity`.
    /// `None` for a unit-identity actor or a non-principal statement.
    pub call_site_identity: Option<String>,
    /// I3 (#1971): the call-site principal's actor name, beside
    /// [`Self::call_site_identity`], for a `call` sum's `deps.who`.
    pub call_site_actor: Option<String>,
    /// #706: the call-site principal is `by Nobody` — drive the route with no
    /// `Authorization` header so the real auth seam rejects it (`401` →
    /// `Rejected(Unauthorized)`). Routes a `system` http address to the no-auth
    /// driver. `false` for any other (or no) principal.
    pub call_site_no_credential: bool,
    /// #2135: the call-site principal is `by Forged` or `by Stale` — drive a
    /// `Signature` route with a signature its seam refuses (`"forged"` /
    /// `"stale"`, the driver's name). `None` for any other principal.
    pub call_site_bad_signature: Option<&'static str>,
    /// Slice 1 (ADR 0103): the source-map builder for the file being emitted, if
    /// any. The deep lowering chain records `(generated offset → source span)`
    /// checkpoints here; `emit_project` owns the `RefCell` and threads a shared
    /// borrow in. `None` for the single-file `emit()` path and any body emitted
    /// outside a project, where no map is produced.
    pub source_map: Option<&'a RefCell<SourceMapBuilder>>,
    /// T2.2 (R6.4): set at the two statement sites that emit a literal `await`
    /// (`EffectLet`, `Do`) and read-and-reset around a value-position `match`/`if`
    /// IIFE's own body construction — the flag a synchronous arrow reads to decide
    /// whether it must become `async` and be awaited at its call site. Replaces a
    /// scan of the built string for the substring `"await "`, which over-matched
    /// on a self-contained `async (...) => {...}` embedded as an arm's value (an
    /// iterator terminal like `forEach`) without anything in *this* arrow's own
    /// scope needing to await. Not isolated around a lambda body — a nested
    /// effectful lambda still marks the enclosing IIFE async, exactly as the old
    /// scan did (its own body text also contained `"await "`); closing that is a
    /// separate, unscoped defect, not this one.
    pub(crate) emitted_await: bool,
    /// T2.3 (R6.3): set when lowering a `?` pushes a propagating early-return
    /// statement (`if (...) return ...;`) into the current `Pre`. Read (and
    /// reset) around a short-circuit operand's own lowering in `lower_bin_op`/
    /// `lower_and_with_is`, so those can tell a hoisted `?` apart from an
    /// ordinary hoisted statement: a plain `(() => { ...; return expr; })()`
    /// wrap is safe for the latter (nothing inside needs to escape the arrow)
    /// but captures the former's `return` instead of letting it exit the
    /// enclosing function — the residual gap `hoist_if_as_statement` (built for
    /// T2.1's `if`-hoisting) also closes here, once this flag says it's needed.
    pub(crate) emitted_early_return: bool,
    /// #1750: where a block's tail value goes. `None` (the default) is a
    /// `return`, the right sink for a function, lambda or arrow body. `Some` is
    /// set while a value-position `if`/`match`/block that contains a `?` is
    /// emitted as a real statement rather than an arrow: each tail assigns the
    /// slot and breaks out of the labelled block wrapping the statement, so
    /// the `?`'s own `return` still exits the enclosing function. Every arrow
    /// and function boundary resets it to `None` for its own body.
    pub(crate) tail_slot: Option<TailSlot>,
}

/// #1750: the slot and label a statement-form value expression's tails assign
/// and break to. See [`LowerCtx::tail_slot`].
#[derive(Clone)]
pub(crate) struct TailSlot {
    pub(crate) slot: String,
    pub(crate) label: String,
}

/// v0.59: the source context an `assert` lowering needs to turn its span into a
/// `path:line:col` location. Owned (cloned once per test-case body) to keep the
/// lowering free of extra lifetime threading; test-file sources are small and
/// this is compile-time only.
#[derive(Clone)]
pub(crate) struct AssertLoc {
    pub source: String,
    pub rel_path: String,
}

impl<'a> LowerCtx<'a> {
    fn new(module: ModuleCtx<'a>, mode: BodyMode) -> Self {
        Self {
            module,
            mode,
            local_agents: HashSet::new(),
            return_ty: None,
            // Every field below is per-body scratch state: a fresh `LowerCtx` is
            // built at each body-emission site and never reused, so these must
            // re-initialise here on every construction. In particular `next_tmp`
            // restarting at 0 is what makes each emitted function's temps begin
            // at `__r0`.
            next_tmp: 0,
            shadow_scopes: vec![HashMap::new()],
            harness_bound: HashSet::new(),
            is_receiver_temps: HashMap::new(),
            is_temp_hoist: None,
            unnarrowed_is_tests: HashSet::new(),
            local_agent_vars: HashMap::new(),
            call_site_identity: None,
            call_site_actor: None,
            call_site_no_credential: false,
            call_site_bad_signature: None,
            source_map: None,
            emitted_await: false,
            emitted_early_return: false,
            tail_slot: None,
        }
    }

    // ---- `ModuleCtx` passthroughs -----------------------------------------
    //
    // Returned with the `'a` module lifetime rather than the `&self` borrow, so
    // a `&mut self` lowering step can hold onto a commons/runtime handle across
    // its own recursive calls exactly as it did when these were plain fields.

    /// Typed-commons handle for the module being emitted.
    pub(crate) fn commons(&self) -> &'a TypedCommons {
        self.module.commons
    }

    /// Cross-context info for v0.6 cross-context call lowering.
    pub(crate) fn cross_context(&self) -> &'a bynk_check::resolver::CrossContextInfo {
        self.module.cross_context
    }

    /// The emitted module's conditional-runtime-helper accumulator.
    pub(crate) fn runtime_use(&self) -> &'a RuntimeUse {
        self.module.runtime_use
    }

    /// v0.8 build target.
    pub(crate) fn target(&self) -> BuildTarget {
        self.module.target
    }

    /// #2045: whether `name` is a consumed context's type on `workers`.
    pub(crate) fn is_consumed_context_type(&self, name: &str) -> bool {
        self.module.consumed_context_types.contains(name)
    }

    /// #934: true when the unit being emitted is the first-party `bynk` adapter.
    pub(crate) fn in_bynk_unit(&self) -> bool {
        self.module.in_bynk_unit
    }

    // ---- capability-bearing (`HandlerShared`) state ------------------------
    //
    // Every accessor here reports the same default a non-handler kind used to
    // get from the flat struct (no capabilities, no scope, `deps`), so a caller
    // that does not care which kind it is in reads unchanged.

    fn handler(&self) -> Option<&HandlerShared> {
        match &self.mode {
            BodyMode::ServiceHandler { handler, .. }
            | BodyMode::ProviderOp { handler }
            | BodyMode::AgentHandler { handler, .. }
            | BodyMode::WsDoMethod { handler, .. } => Some(handler),
            _ => None,
        }
    }

    fn handler_mut(&mut self) -> Option<&mut HandlerShared> {
        match &mut self.mode {
            BodyMode::ServiceHandler { handler, .. }
            | BodyMode::ProviderOp { handler }
            | BodyMode::AgentHandler { handler, .. }
            | BodyMode::WsDoMethod { handler, .. } => Some(handler),
            _ => None,
        }
    }

    /// Whether `name` is a capability in scope as `given C1, C2, ...`.
    pub(crate) fn has_capability(&self, name: &str) -> bool {
        self.handler()
            .is_some_and(|h| h.capabilities.contains(name))
    }

    /// #934: the calling handler's own qualified name, if a capability call can
    /// occur in this body at all. `None` for every non-handler kind — the
    /// `Idempotency` key-scoping lowering treats that as a compiler bug and
    /// panics, exactly as it did when this was a flat `Option` field.
    pub(crate) fn handler_scope(&self) -> Option<&str> {
        self.handler().and_then(|h| h.handler_scope.as_deref())
    }

    /// Events track, slice 2: the qualified name of the unit this body is
    /// emitted into, for the `Events.emit[E](event)` lowering's
    /// `publisherId`. `None` for a body kind that carries no `HandlerShared`
    /// at all (an `Events.emit` call cannot occur there).
    pub(crate) fn owning_context(&self) -> Option<&str> {
        self.handler().map(|h| h.owning_context.as_str())
    }

    /// v0.12: the receiver expression a capability call resolves against.
    pub(crate) fn cap_deps_expr(&self) -> &str {
        self.handler().map_or("deps", |h| h.cap_deps_expr.as_str())
    }

    /// Note that this body called a service of `consumed`. A no-op in a body
    /// kind that carries no deps shape to widen (a plain method, a predicate, a
    /// test case) — those never read it back.
    pub(crate) fn note_cross_context_used(&mut self, consumed: &str) {
        if let Some(h) = self.handler_mut() {
            h.cross_contexts_called.insert(consumed.to_string());
        }
    }

    /// True if this handler made at least one cross-context call.
    pub(crate) fn cross_context_used(&self) -> bool {
        self.handler()
            .is_some_and(|h| !h.cross_contexts_called.is_empty())
    }

    /// The consumed contexts this handler's `deps.env` must bind, in name
    /// order (#1961): those it calls, plus — when it constructs an agent, whose
    /// methods it hands its `deps` — those the context's agents call.
    pub(crate) fn cross_contexts_called(&self) -> Vec<String> {
        let mut called: std::collections::BTreeSet<String> = self
            .handler()
            .map(|h| h.cross_contexts_called.clone())
            .unwrap_or_default();
        if self.agents_instantiated() {
            called.extend(self.module.agent_called_contexts.iter().cloned());
        }
        called.into_iter().collect()
    }

    /// v0.9.2: true if this body instantiated a local agent.
    pub(crate) fn agents_instantiated(&self) -> bool {
        self.handler().is_some_and(|h| h.agents_instantiated)
    }

    /// #527: capabilities required by agent methods this body calls.
    pub(crate) fn agent_given_caps_used(
        &self,
    ) -> Option<&std::collections::BTreeMap<String, bynk_ir::CapRefIr>> {
        self.handler().map(|h| &h.agent_given_caps_used)
    }

    // ---- test-scaffold (`TestShared`) state --------------------------------

    fn test(&self) -> Option<&TestShared> {
        match &self.mode {
            BodyMode::PredicateScaffold { test }
            | BodyMode::TestCase { test, .. }
            | BodyMode::IntegrationCase { test, .. } => Some(test),
            _ => None,
        }
    }

    /// True when lowering **generated test-scaffold** TypeScript (a test-case
    /// body, or a `stub`/`where`/`requires` predicate value), where branded
    /// types are destructured into `any`-typed value bindings rather than
    /// referenced as types. Callers that emit a branded `as`-cast consult this
    /// to pick `unchecked_construct_test` (→ `(v as any)`) over the production
    /// `(v as T)` form, which cannot resolve `T` in the test module's scope.
    pub(crate) fn in_test_scaffold(&self) -> bool {
        self.test().is_some_and(|t| t.test_scaffold)
    }

    /// v0.59: the test body's source context, for `assert`/`expect` locations.
    pub(crate) fn assert_loc(&self) -> Option<&AssertLoc> {
        self.test().and_then(|t| t.assert_loc.as_ref())
    }

    // ---- single-kind state -------------------------------------------------

    /// v0.80: inside an invariant predicate, the proposed-state variable and the
    /// agent's state field names.
    pub(crate) fn invariant_state(&self) -> Option<(&str, &HashSet<String>)> {
        match &self.mode {
            BodyMode::Invariant { name, fields } => Some((name.as_str(), fields)),
            _ => None,
        }
    }

    /// v0.116: inside a `transition` predicate, the JS names bound to the
    /// contextual `old`/`new` state records.
    pub(crate) fn transition_states(&self) -> Option<(&str, &str)> {
        match &self.mode {
            BodyMode::Transition { old, new } => Some((old.as_str(), new.as_str())),
            _ => None,
        }
    }

    /// v0.117: the recorded-call trace object a test case's observations read.
    pub(crate) fn observation_trace(&self) -> Option<&str> {
        match &self.mode {
            BodyMode::TestCase {
                observation_trace, ..
            } => observation_trace.as_deref(),
            _ => None,
        }
    }

    fn agent_store(&self) -> Option<&AgentStoreState> {
        match &self.mode {
            BodyMode::AgentHandler { store, .. } => store.as_deref(),
            _ => None,
        }
    }

    /// v0.81: the mutable working-state variable a `store`-agent handler stages
    /// its writes into. `__state` is the name every real site uses; the fallback
    /// keeps the (defensive) non-store paths rendering as they did before.
    pub(crate) fn agent_store_var(&self) -> &str {
        self.agent_store().map_or("__state", |s| s.state.0.as_str())
    }

    /// v0.81: the working-state variable plus the `Cell` field names it holds.
    pub(crate) fn agent_store_cells(&self) -> Option<(&str, &HashSet<String>)> {
        self.agent_store().map(|s| (s.state.0.as_str(), &s.state.1))
    }

    /// v0.82: whether `name` is a persisted `store Map` field (held connection
    /// maps are deliberately excluded — they use the connId lowering).
    pub(crate) fn is_agent_store_map(&self, name: &str) -> bool {
        self.agent_store().is_some_and(|s| s.maps.contains(name))
    }

    /// v0.83: whether `name` is a `store Set` field.
    pub(crate) fn is_agent_store_set(&self, name: &str) -> bool {
        self.agent_store().is_some_and(|s| s.sets.contains(name))
    }

    /// v0.87: the ttl (millis) of the `store Cache` field `name`, if it is one.
    pub(crate) fn agent_store_cache_ttl(&self, name: &str) -> Option<i64> {
        self.agent_store().and_then(|s| s.caches.get(name).copied())
    }

    /// v0.95: the `@retain` horizon of the `store Log` field `name`, if it is
    /// one. The outer `Option` is "is a log"; the inner is "has a retain".
    pub(crate) fn agent_store_log_retain(&self, name: &str) -> Option<Option<i64>> {
        self.agent_store().and_then(|s| s.logs.get(name).copied())
    }

    /// v0.95: whether `name` is a `store Log` field.
    pub(crate) fn is_agent_store_log(&self, name: &str) -> bool {
        self.agent_store()
            .is_some_and(|s| s.logs.contains_key(name))
    }

    /// v0.93: the value-record fields the `store Map` `name` is `@indexed(by:)`
    /// on — empty when it has no secondary index.
    pub(crate) fn agent_store_index_fields(&self, name: &str) -> Vec<String> {
        self.agent_store()
            .and_then(|s| s.indexes.get(name).cloned())
            .unwrap_or_default()
    }

    /// v0.105: the connection **frame type** of the held `store Map[K,
    /// Connection]` field `name`, if it is one.
    pub(crate) fn agent_held_map_frame(&self, name: &str) -> Option<&String> {
        self.agent_store().and_then(|s| s.held_maps.get(name))
    }

    /// v0.105: whether `name` is a held `store Map[K, Connection]` field.
    pub(crate) fn is_agent_held_map(&self, name: &str) -> bool {
        self.agent_store()
            .is_some_and(|s| s.held_maps.contains_key(name))
    }

    /// Review of #1460: the value type `V` of the plain `store Map[K, V]`
    /// field `name`, if it is one (`None` for a held map — see
    /// [`Self::agent_held_map_frame`] instead).
    pub(crate) fn agent_store_map_value_ts(&self, name: &str) -> Option<&String> {
        self.agent_store().and_then(|s| s.map_values.get(name))
    }

    /// Review of #1460: the element type `T` of the `store Log[T]` field
    /// `name`, if it is one.
    pub(crate) fn agent_store_log_value_ts(&self, name: &str) -> Option<&String> {
        self.agent_store().and_then(|s| s.log_values.get(name))
    }

    /// True when lowering an agent handler body — drives the `self.<keyField>`
    /// rewrite.
    pub(crate) fn in_agent_handler(&self) -> bool {
        match &self.mode {
            BodyMode::AgentHandler {
                in_agent_handler, ..
            } => *in_agent_handler,
            _ => false,
        }
    }

    /// The name of the agent's `key id` field, inside an agent handler body.
    pub(crate) fn agent_key_field(&self) -> Option<&str> {
        match &self.mode {
            BodyMode::AgentHandler {
                agent_key_field, ..
            } => agent_key_field.as_deref(),
            _ => None,
        }
    }

    /// v0.104: the agent hosting the websocket lifecycle body being lowered.
    pub(crate) fn ws_self_agent(&self) -> Option<&str> {
        match &self.mode {
            BodyMode::WsDoMethod { ws_self_agent, .. } => ws_self_agent.as_deref(),
            _ => None,
        }
    }

    /// v0.47: the `by` binder whose `.identity` is threaded through `deps`.
    pub(crate) fn deps_identity_binder(&self) -> Option<&str> {
        match &self.mode {
            BodyMode::ServiceHandler {
                deps_identity_binder,
                ..
            }
            | BodyMode::WsDoMethod {
                deps_identity_binder,
                ..
            } => deps_identity_binder.as_deref(),
            _ => None,
        }
    }

    /// v0.52: the multi-actor sum handler's resolved-actor binder.
    pub(crate) fn actor_sum_binder(&self) -> Option<&str> {
        match &self.mode {
            BodyMode::ServiceHandler {
                actor_sum_binder, ..
            } => actor_sum_binder.as_deref(),
            _ => None,
        }
    }

    /// v0.7: whether `name` is a local service of the test case's target context.
    pub(crate) fn is_test_service(&self, name: &str) -> bool {
        match &self.mode {
            BodyMode::TestCase { test_services, .. } => test_services.contains(name),
            _ => false,
        }
    }

    /// I3 (#1971), #2020: whether the test service `service`'s handler at
    /// `key` (`"call"`, or an http handler's emitted method name) binds an
    /// actor sum, so a call-site principal builds its `deps.who`.
    pub(crate) fn is_sum_handler(&self, service: &str, key: &str) -> bool {
        match &self.mode {
            BodyMode::TestCase { sum_handlers, .. } => {
                sum_handlers.contains(&(service.to_string(), key.to_string()))
            }
            _ => false,
        }
    }

    /// #1975: whether this is a test `case` body.
    pub(crate) fn in_test_case(&self) -> bool {
        matches!(&self.mode, BodyMode::TestCase { .. })
    }

    /// #1994: whether an addressed test-service call settles the `~>` sends
    /// its handler handed `deps.__exec`.
    pub(crate) fn test_settles_sends(&self) -> bool {
        matches!(
            &self.mode,
            BodyMode::TestCase {
                settle_sends: true,
                ..
            }
        )
    }

    /// v0.182 (#664): the ordered handler kinds of the test service `name`.
    pub(crate) fn test_service_handlers(&self, name: &str) -> Option<&[bynk_ir::IrHandlerKind]> {
        match &self.mode {
            BodyMode::TestCase {
                test_service_handlers,
                ..
            } => test_service_handlers.get(name).map(Vec::as_slice),
            _ => None,
        }
    }

    /// v0.182 (Slice B, #667): whether `name` is an http service of the system
    /// target being driven.
    pub(crate) fn is_system_http_service(&self, name: &str) -> bool {
        match &self.mode {
            BodyMode::IntegrationCase {
                system_http_services,
                ..
            } => system_http_services.contains(name),
            _ => false,
        }
    }

    /// #2136: the `__sysdrive_schedule_<svc>_<index>` driver for a cron address
    /// `svc.schedule("<expr>", …)` in a `system` case, when `svc` is a cron
    /// service of the target that declares `expr`.
    pub(crate) fn system_schedule_driver(&self, service: &str, expr: &str) -> Option<String> {
        match &self.mode {
            BodyMode::IntegrationCase { system_jobs, .. } => {
                let idx = system_jobs
                    .cron
                    .get(service)?
                    .iter()
                    .position(|e| e == expr)?;
                Some(format!("__sysdrive_schedule_{service}_{idx}"))
            }
            _ => None,
        }
    }

    /// #2136: the `__sysdrive_message_<q>` driver for a queue address
    /// `q.message(m)` in a `system` case, when `q` is a queue service of the
    /// target.
    pub(crate) fn system_message_driver(&self, service: &str) -> Option<String> {
        match &self.mode {
            BodyMode::IntegrationCase { system_jobs, .. }
                if system_jobs.queue.contains(service) =>
            {
                Some(format!("__sysdrive_message_{service}"))
            }
            _ => None,
        }
    }

    /// #707: whether `(service, verb, path)` is a declared route of the system
    /// target — an undeclared one drives the `405` fall-through.
    pub(crate) fn has_system_http_route(&self, route: &(String, String, String)) -> bool {
        match &self.mode {
            BodyMode::IntegrationCase {
                system_http_routes, ..
            } => system_http_routes.contains(route),
            _ => false,
        }
    }

    /// #708: the body param position and declared type of a system http route.
    pub(crate) fn system_http_route_body(
        &self,
        route: &(String, String, String),
    ) -> Option<&(usize, bynk_syntax::ast::TypeRef)> {
        match &self.mode {
            BodyMode::IntegrationCase {
                system_http_route_body,
                ..
            } => system_http_route_body.get(route),
            _ => None,
        }
    }

    /// #708: the type namespace a system http body param's codec resolves in.
    pub(crate) fn system_http_type_ns(&self) -> &str {
        match &self.mode {
            BodyMode::IntegrationCase {
                system_http_type_ns,
                ..
            } => system_http_type_ns.as_str(),
            _ => "",
        }
    }

    /// #2019: the claims argument a `system`-tier call on `route` passes, if
    /// the route is refinement-guarded: the acting refinement's witness
    /// claims, or `{}` for any other actor.
    pub(crate) fn system_http_claims_arg(
        &self,
        route: &(String, String, String),
    ) -> Option<String> {
        match &self.mode {
            BodyMode::IntegrationCase {
                system_http_claims, ..
            } if system_http_claims.routes.contains(route) => Some(
                self.call_site_actor
                    .as_ref()
                    .and_then(|a| system_http_claims.actors.get(a))
                    .cloned()
                    .unwrap_or_else(|| "{}".to_string()),
            ),
            _ => None,
        }
    }

    /// #2155: the acting actor's name, as a string literal, for a signing
    /// driver of an actor-sum route — the call-site principal, else the
    /// sum's primary member. `None` for any other route.
    pub(crate) fn system_http_sum_actor_arg(
        &self,
        route: &(String, String, String),
    ) -> Option<String> {
        match &self.mode {
            BodyMode::IntegrationCase {
                system_http_claims, ..
            } => system_http_claims
                .sum_routes
                .get(route)
                .map(|primary| format!("{:?}", self.call_site_actor.as_deref().unwrap_or(primary))),
            _ => None,
        }
    }

    /// Events track, slice 0 (spine #936): true when a bare `Events`
    /// receiver in this unit is genuinely the first-party `bynk.Events`
    /// capability — declared here because this unit *is* `bynk`, or
    /// flattened in from it (`consumes bynk { Events }`) — not some other,
    /// unrelated capability that merely happens to share the name. Mirrors
    /// #934's `Idempotency` distinction (`is_first_party` at the
    /// `Idempotency.dedup`/`remember` lowering site). Both the call-site
    /// interception (`lower.rs`) and the `__events` buffer declaration
    /// (`block_uses_emit`'s gate in `emit.rs`) must agree on this, or a
    /// custom same-named `Events` capability's calls get silently rewritten
    /// into a buffer nothing constructs a provider for.
    pub(crate) fn is_first_party_events(&self) -> bool {
        self.in_bynk_unit()
            || self
                .cross_context()
                .flattened_caps
                .get("Events")
                .map(String::as_str)
                == Some("bynk")
    }

    /// #2058: `name` as this module spells it in a type position: qualified
    /// under its context's namespace when a consumed context declares it.
    pub(crate) fn consumed_type_name(&self, name: &str) -> String {
        match self.module.consumed_type_ns.get(name) {
            Some(ns) => [ns.as_str(), ".", name].concat(),
            None => name.to_string(),
        }
    }

    /// Events slice 3b (#978): the declared `@schema(N)` version of the
    /// locally-declared event `name`, or `1` if it has none (including if
    /// `name` isn't a locally-declared event at all — `Events.emit[E]` only
    /// ever names an owned event, checker-enforced, so a miss here can only
    /// mean a broken build already reported elsewhere, and this degrades to
    /// today's pre-existing output rather than panicking).
    pub(crate) fn event_schema_version(&self, name: &str) -> i64 {
        self.module
            .event_schema_versions
            .get(name)
            .copied()
            .unwrap_or(1)
    }

    /// Attach the file's source-map builder (slice 1, ADR 0103). Builder-style so
    /// the emission sites with no builder leave `LowerCtx::new(module, mode)`
    /// untouched — only the project-emission path that has one calls this.
    fn with_source_map(mut self, map: Option<&'a RefCell<SourceMapBuilder>>) -> Self {
        self.source_map = map;
        self
    }

    /// Record that this lowering emitted a reference to the `Bytes` runtime
    /// helpers, so the module imports them.
    fn note_bytes(&self) {
        self.runtime_use().note_bytes();
    }

    /// Record that this lowering emitted a `Bytes` operation helper (#2016), so
    /// the module imports that group.
    fn note_bytes_ops(&self) {
        self.runtime_use().note_bytes_ops();
    }

    /// Record that this lowering constructed an `HttpResult` variant (#1934),
    /// so a test module imports it.
    fn note_http_result(&self) {
        self.runtime_use().note_http_result();
    }

    /// Record that this lowering emitted a reference to `__bynkEq` (#1652), so
    /// the module imports it.
    fn note_eq(&self) {
        self.runtime_use().note_eq();
    }

    /// Record that this lowering emitted an `Int` domain trap (#1657), so the
    /// module imports the helpers.
    fn note_int(&self) {
        self.runtime_use().note_int();
    }

    /// Record a checkpoint: generated text from `out_len` onward originates at
    /// `span`, until the next checkpoint (ADR 0103 D2, nearest-enclosing). A
    /// no-op when no builder is attached. `out_len` is the buffer length *before*
    /// the statement's text is appended.
    ///
    /// `out_len` only means something relative to the *top-level module
    /// buffer* the attached builder is tracking. A caller building an IIFE
    /// into its own local `String` — `lower_if`'s value-position wrapper,
    /// `build_match_iife`'s — before splicing it elsewhere must not call this
    /// with that buffer's own length; see [`Self::without_source_map`].
    fn record_span(&self, out_len: usize, span: bynk_syntax::span::Span) {
        if let Some(map) = self.source_map {
            map.borrow_mut().record(out_len, span);
        }
    }

    /// #4 review: run `f` with source-map recording suppressed, restoring it
    /// after. For lowering into a local IIFE buffer that will later be
    /// spliced into the real module text at some other offset — `record_span`
    /// has no way to know that offset, so a checkpoint taken here would
    /// silently corrupt the map with a position relative to the wrong
    /// buffer. `SourceMapBuilder::merge` already solves the equivalent
    /// problem one level up (a handler/test body's own local buffer, spliced
    /// into the module) by recording into a *sub*-builder and rebasing at the
    /// splice — but that needs a builder that outlives the call, and
    /// `source_map` is `Option<&'a RefCell<SourceMapBuilder>>` tied to the
    /// whole emission's lifetime, so a function-local sub-builder can't be
    /// substituted in. Suppressing instead of mis-recording means the
    /// nearest-enclosing-checkpoint rule (ADR 0103 D2) falls back to whatever
    /// was correctly mapped just before the IIFE started, rather than a wrong
    /// one silently taking over — degraded stepping through the IIFE's own
    /// lines in `bynkc test --inspect`, not a corrupted map.
    fn without_source_map<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let saved = self.source_map.take();
        let result = f(self);
        self.source_map = saved;
        result
    }
    /// v0.9.2: lower an agent instantiation `AgentName(key)` to its factory
    /// call. Bundle/test mode passes only the key; workers mode also threads
    /// `deps.env` so the factory can reach the agent's DO namespace.
    fn agent_construct(&mut self, agent: &str, key_expr: &str) -> String {
        if let Some(h) = self.handler_mut() {
            h.agents_instantiated = true;
        }
        let factory = agent_factory_name(agent);
        if matches!(self.target(), BuildTarget::Workers) {
            format!("{factory}({key_expr}, deps.env)")
        } else {
            format!("{factory}({key_expr})")
        }
    }

    /// #527: note that the body calls `agent.method`, folding the method's
    /// `given` capabilities into this handler's requirement set. A no-op in a
    /// body kind that has no deps shape to widen — those never read it back.
    pub(crate) fn record_agent_call(&mut self, agent: &str, method: &str) {
        let givens = self
            .module
            .agent_method_givens
            .get(agent)
            .and_then(|m| m.get(method))
            .cloned()
            .unwrap_or_default();
        if let Some(h) = self.handler_mut() {
            for c in givens {
                h.agent_given_caps_used.entry(c.name.clone()).or_insert(c);
            }
        }
    }
    fn fresh(&mut self) -> String {
        let n = self.next_tmp;
        self.next_tmp += 1;
        format!("__r{n}")
    }
    /// #908: bind a `let`/`let <-` LHS to its emitted JS identifier. Returns
    /// the natural `ts_ident` name unless `name` is already bound *anywhere*
    /// in the enclosing block chain — not only the current block. A nested
    /// block re-`let`-ing an outer name is ordinary, valid lexical shadowing
    /// in JS on its own, but this `let`'s own RHS may still read the outer
    /// binding (`let n = n + 1` one block in); a plain `const n` there
    /// would put the read inside its own declaration's temporal dead zone
    /// (JS hoists a block's `let`/`const` names to the top of that block),
    /// turning a correct read of the outer value into a TDZ ReferenceError.
    /// Renaming whenever *any* enclosing frame already has the name sidesteps
    /// that regardless of whether this particular RHS reads it. Allocates a
    /// fresh name via [`Self::fresh`] when so. `_` never collides (each is
    /// already a fresh throwaway) and is never registered, since it is never
    /// read.
    pub(crate) fn bind_local_name(&mut self, name: &str) -> String {
        if name == "_" {
            return self.fresh();
        }
        let natural = ts_ident(name);
        let js_name = if self.shadow_scopes.iter().any(|f| f.contains_key(name))
            || self.harness_bound.contains(&natural)
        {
            self.fresh()
        } else {
            natural
        };
        self.shadow_scopes
            .last_mut()
            .expect("shadow_scopes always has a root frame")
            .insert(name.to_string(), js_name.clone());
        js_name
    }
    /// #908: the emitted JS identifier currently bound to a local name, if a
    /// `let` re-bind renamed it somewhere in the enclosing block chain.
    /// Walked innermost-out so a nested block sees an outer rename that was
    /// still active when it was entered. `None` means no rename applies —
    /// callers fall back to the natural `ts_ident` name.
    pub(crate) fn resolved_local_name(&self, name: &str) -> Option<String> {
        self.shadow_scopes
            .iter()
            .rev()
            .find_map(|f| f.get(name).cloned())
    }
    /// Whether `name` is bound by an enclosing local (a `let`, match-arm/`is`
    /// binding, or lambda param) rather than free to refer to a store field.
    /// A local always wins: store-field dispatch by bare receiver name must
    /// check this first, or a parameter/binding that happens to share a store
    /// field's name is silently treated as the store field.
    pub(crate) fn is_local(&self, name: &str) -> bool {
        self.shadow_scopes.iter().any(|f| f.contains_key(name))
    }
    /// #908: register a non-`let` binder (a match-arm/`is` pattern binding, or
    /// a lambda param) into the current frame under its natural `ts_ident`
    /// name — never renamed, since each such binder already lowers inside its
    /// own JS block/arrow scope with no risk of colliding with a sibling
    /// declaration of the same name. Without this, a read inside the binder's
    /// scope would fall through [`Self::resolved_local_name`]'s stack walk
    /// past this (unregistered) declaration to an outer `let` rename that is
    /// no longer the right value here — silently wrong output, not a `tsc`
    /// error. Every construct that introduces a binder outside `bind_local_name`
    /// (match arms, `is`, lambda params) must call this for each name it binds.
    pub(crate) fn declare_binder(&mut self, name: &str) {
        if name == "_" {
            return;
        }
        self.shadow_scopes
            .last_mut()
            .expect("shadow_scopes always has a root frame")
            .insert(name.to_string(), ts_ident(name));
    }
    /// Return a stable textual reference to an `is` receiver, used by the
    /// `.tag` check in `lower_is`. A simple, repeatable lvalue is lowered
    /// inline exactly as before (preserving rewrites such as `self.state` or
    /// capability access). A complex receiver (anything `is_simple_is_receiver`
    /// rejects — e.g. a call) is evaluated once into a fresh temp
    /// hoisted into the returned `Lowered` and cached by span, so the bindings
    /// gathered later reference the same evaluation rather than re-running the
    /// expression.
    fn is_receiver_ref(&mut self, value: &Expr) -> Lowered {
        if let Some(t) = self.is_receiver_temps.get(&value.span) {
            return Lowered::bare(t.clone());
        }
        let mut pre = Pre::new();
        let lowered = pre.lower(value, self);
        if is_simple_is_receiver(value) {
            return pre.finish(lowered);
        }
        let tmp = self.fresh();
        self.declare_is_temp(value, &tmp, &lowered, &mut pre);
        self.is_receiver_temps.insert(value.span, tmp.clone());
        pre.finish(tmp)
    }

    /// Declare an `is` receiver temp holding `lowered`: a `const` in place, or,
    /// inside a hoisting right operand (`is_temp_hoist`), a hoisted
    /// definite-assignment declaration plus an in-place assignment. The
    /// assignment stays where the receiver was evaluated, so the right operand
    /// is still evaluated only when the left operand holds.
    fn declare_is_temp(&mut self, value: &Expr, tmp: &str, lowered: &str, pre: &mut Pre) {
        let ty = self
            .commons()
            .expr_types
            .get(&value.id)
            .map(|te| ts_ty(te.ty, self.commons().tys()));
        match (self.is_temp_hoist.as_mut(), ty) {
            (Some(hoist), Some(ty)) => {
                hoist.push(format!("let {tmp}!: {ty};"));
                pre.push(format!("{tmp} = {lowered};"));
            }
            _ => pre.push(format!("const {tmp} = {lowered};")),
        }
    }

    /// v0.13: like `is_receiver_ref` but always lifts to a temp, even for a
    /// simple ident. A refined `is`-narrowing re-binds the value's name to the
    /// branded refined type (`const n = <temp> as Quantity`); that shadowing
    /// const cannot reference the same name (TDZ), so the value is captured in a
    /// temp first and both the check and the binding read the temp.
    fn is_receiver_ref_forced(&mut self, value: &Expr) -> Lowered {
        if let Some(t) = self.is_receiver_temps.get(&value.span) {
            return Lowered::bare(t.clone());
        }
        let mut pre = Pre::new();
        let lowered = pre.lower(value, self);
        let tmp = self.fresh();
        self.declare_is_temp(value, &tmp, &lowered, &mut pre);
        self.is_receiver_temps.insert(value.span, tmp.clone());
        pre.finish(tmp)
    }

    /// v0.13: true when `value is Name` is a *refinement* check — the value is a
    /// base/refined value and `Name` is a refined type — rather than a sum
    /// variant test. Mirrors the checker's disambiguation.
    ///
    /// P6.56 (design/tracks/the-ir.md §6b): `name_refined` reads only
    /// `TypeBody`'s own discriminant, never `base`/`refinement` — the two
    /// fields `TypeShape::Refined` would add real construction cost for
    /// (`refined_or_opaque_base`'s own doc comment has the full reasoning).
    /// Investigated and declined on the identical grounds.
    fn is_refined_is_check(&self, value: &Expr, name: &str) -> bool {
        let value_baseish = matches!(
            self.commons().expr_ty(value.id).as_deref(),
            Some(Ty::Base(_))
                | Some(Ty::Named {
                    kind: NamedKind::Refined(_),
                    ..
                })
        );
        let name_refined = matches!(
            self.commons().types.get(name).map(|d| &d.body),
            Some(TypeBody::Refined { .. })
        );
        value_baseish && name_refined
    }
    /// Read-only counterpart for the binding gatherer (which returns no
    /// `Lowered`, so it has nowhere to hoist and cannot lift). If the receiver was already lifted to a temp during
    /// condition lowering, reuse that temp; otherwise it must be a simple
    /// repeatable lvalue, rendered inline. The "lower the condition before
    /// gathering its bindings" ordering in `emit_if_tail`, `lower_and_with_is`
    /// and the value-position `if` IIFE guarantees the temp exists before this
    /// is called for complex receivers; `value_text_for_is` panics if it does
    /// not (#1668).
    fn is_receiver_text(&self, value: &Expr) -> String {
        if let Some(t) = self.is_receiver_temps.get(&value.span) {
            return t.clone();
        }
        value_text_for_is(value)
    }
    fn receiver_namespace(&self, e: &Expr) -> Option<String> {
        let ty = self.commons().expr_ty(e.id)?;
        if let Ty::Named { name, .. } = &*ty {
            Some(name.clone())
        } else {
            None
        }
    }
    /// Resolve the payload field name for the i-th positional binding of
    /// a variant. Built-ins are recognised by name; user variants are
    /// looked up via the type tables.
    ///
    /// P6.56 (design/tracks/the-ir.md §6b): see `sum_owner_of_variant`'s own
    /// doc comment (`emitter.rs`) — a `TypeShape::Sum`-routed conversion was
    /// investigated and declined here on the identical grounds.
    fn positional_field_name(
        &self,
        discriminant_ty: Option<TyId>,
        variant: &str,
        idx: usize,
        tys: &Arc<Types>,
    ) -> String {
        match (variant, idx) {
            ("Ok", 0) | ("Some", 0) => return "value".to_string(),
            ("Err", 0) => return "error".to_string(),
            _ => {}
        }
        // v0.52: a multi-actor sum arm binds the resolved actor's identity,
        // carried in the `identity` field of the tagged object.
        let disc_node = discriminant_ty.map(|t| tys.get(t));
        if let Some(Ty::ActorSum(_)) = disc_node.as_deref() {
            return "identity".to_string();
        }
        // #1934: the payloads a case binds from a queue verdict and a
        // boundary rejection, named as `queue.ts` and `http.ts` name them.
        match disc_node.as_deref() {
            Some(Ty::QueueResult) if variant == "Retry" && idx == 0 => {
                return "reason".to_string();
            }
            Some(Ty::BoundaryRejection) if idx == 0 => return "detail".to_string(),
            _ => {}
        }
        if let Some(Ty::Named {
            kind: NamedKind::Sum,
            name,
            ..
        }) = disc_node.as_deref()
            && let Some(decl) = self.commons().types.get(name)
            && let TypeBody::Sum(s) = &decl.body
            && let Some(v) = s.variants.iter().find(|v| v.name.name == variant)
            && let Some(f) = v.payload.get(idx)
        {
            return payload_prop(&f.name.name);
        }
        // #1883, slice 3: an `HttpResult` variant's payload fields, named as
        // the runtime union in `bynk-emit/runtime/src/http.ts` names them
        // (`Raw(body, ct)`, `BadRequest(m)`, `Found(url)`); the single-field
        // fallback below names them all `value`, which `tsc` rejects for
        // every variant but `Ok`, `Created` and `Accepted`. A typed user sum
        // is resolved above, first. `Ty::Error` is a program that failed
        // checking, which is still lowered and its output discarded.
        if matches!(
            disc_node.as_deref(),
            None | Some(Ty::HttpResult(_)) | Some(Ty::Error)
        ) && let Some(v) = bynk_syntax::ast::http_variant(variant)
        {
            use bynk_syntax::ast::HttpVariantPayload as P;
            let fields: &[&str] = match v.payload {
                P::None => &[],
                P::Value => &["value"],
                P::Message => &["message"],
                P::Location => &["location"],
                P::Streamed => &["stream"],
                P::Raw => &["body", "contentType"],
            };
            if let Some(field) = fields.get(idx) {
                return (*field).to_string();
            }
        }
        // Single-field fallback. The checker rejects mixed bindings already.
        "value".to_string()
    }

    /// The type of a variant's `idx`-th payload field, when resolvable — used to
    /// recurse field-name resolution through nested payload patterns (ADR 0169).
    /// Precise for `Result`/`Option`/`HttpResult` and user sums; `None` otherwise
    /// (callers fall back to the single-field `"value"` name).
    fn payload_field_ty(
        &self,
        ty: Option<TyId>,
        variant: &str,
        idx: usize,
        tys: &Arc<Types>,
    ) -> Option<TyId> {
        match ty.map(|t| tys.get(t)).as_deref() {
            Some(Ty::Result(t, e)) => match (variant, idx) {
                ("Ok", 0) => Some(*t),
                ("Err", 0) => Some(*e),
                _ => None,
            },
            Some(Ty::HttpResult(t)) if variant == "Ok" && idx == 0 => Some(*t),
            // #1934: `Handled(Created(i))` resolves `i`'s field through the
            // `HttpResult` it carries.
            Some(Ty::HttpOutcome(t)) if idx == 0 => match variant {
                "Rejected" => Some(tys.intern(Ty::BoundaryRejection)),
                "Handled" => Some(tys.intern(Ty::HttpResult(*t))),
                _ => None,
            },
            Some(Ty::Option(t)) if variant == "Some" && idx == 0 => Some(*t),
            Some(Ty::Named {
                kind: NamedKind::Sum,
                name,
                args,
            }) => {
                let decl = self.commons().types.get(name)?;
                let TypeBody::Sum(s) = &decl.body else {
                    return None;
                };
                let v = s.variants.iter().find(|v| v.name.name == variant)?;
                let f = v.payload.get(idx)?;
                // #593: substitute the instantiation's type arguments into the
                // payload field type — a bare type parameter (`Loaded(value: T)`)
                // resolves to its concrete argument, exactly as the checker's
                // `variants_of` does. Plain resolve for a non-generic sum (empty
                // `args`), so a nested positional binding recovers the real field
                // name instead of falling back to the generic `"value"`.
                bynk_check::checker::instantiate_field_ty(
                    decl,
                    args,
                    &f.type_ref,
                    &self.commons().types,
                    tys,
                )
            }
            _ => None,
        }
    }
}

/// Unchecked construction of a branded value in emitted TypeScript.
///
/// ADR 0182: an **opaque** type exposes a runtime `.unsafe(value)` constructor
/// (source-callable within its defining commons, and the target of its internal
/// uses), so opaque construction stays `T.unsafe(value)`. A **refined** or
/// **alias** type has **no** public `.unsafe`: exposing one let hand-written host
/// or adapter code bypass the refinement predicate, the credibility hole #545
/// closed. Its admitted / generated values are branded with an inline `as` cast
/// — byte-for-byte the old `.unsafe` body (`return value as T`) at the call site,
/// but not a callable API surface a consumer can reach.
pub(crate) fn unchecked_construct(name: &str, value: &str, is_opaque: bool) -> String {
    if is_opaque {
        format!("{name}.unsafe({value})")
    } else {
        format!("({value} as {name})")
    }
}

/// Unchecked construction inside GENERATED TEST scaffolding (`tests/*.test.ts`).
///
/// There a branded type is in scope only as an `any`-typed value binding
/// (`const {{ T }} = ns as any`) — never as a type — so the production
/// `(value as T)` form fails to resolve `T`. Opaque still constructs through its
/// `.unsafe` value method (kept, ADR 0182); a refined/alias value brands to `any`,
/// which is exactly the type the pre-0182 `T.unsafe(value)` already produced here
/// (`T` being `any`) and erases to the raw value at runtime — without
/// reintroducing a callable refined `.unsafe`.
pub(crate) fn unchecked_construct_test(name: &str, value: &str, is_opaque: bool) -> String {
    if is_opaque {
        format!("{name}.unsafe({value})")
    } else {
        // P7.2: deferred, not narrowed — checked, not skipped. This doc comment's
        // own explanation still holds: `name` genuinely isn't resolvable as a
        // type at this call site (only as an `any`-typed value binding), so
        // there is no real type to cast to here. Fixing this needs restructuring
        // what the test scaffold imports, not a same-line text change.
        format!("({value} as any)")
    }
}

fn ts_base(b: BaseType) -> &'static str {
    match b {
        BaseType::Int => "number",
        BaseType::String => "string",
        BaseType::Bool => "boolean",
        BaseType::Float => "number",
        BaseType::Duration | BaseType::Instant => "number",
        // v0.110 (ADR 0142): `Bytes` is the one base type that does NOT erase
        // to `number` — it lowers to an immutable octet sequence, `Uint8Array`.
        BaseType::Bytes => "globalThis.Uint8Array",
    }
}

pub(crate) fn ts_type_ref(r: &TypeRef) -> String {
    ts_type_ref_with(r, None)
}

/// [`TsType`]-returning sibling of `ts_type_ref`, qualifying named types that
/// live in `scope` with the namespace `ns` (`Order` → `Ns.Order`). Used by
/// `observation_call_record_types` (`project/tests_emit.rs`) for mock method
/// signatures that sit outside the destructuring that brings a namespace's
/// value-side names into local scope, so the types must be referenced fully
/// qualified. Qualification recurses through generic arguments; base/unit
/// types are unaffected.
///
/// Originally paired with a `String`-returning `ts_type_ref_qualified` (Decision
/// B, #1321) — `workers.rs`'s own type-position needs (Arc C slice 3) wanted
/// the structured `TsType` this function returns, added *alongside* rather
/// than replacing, since `ts_type_ref_qualified` itself was still needed by a
/// `String`-based caller. Arc C, slice 32 (`tests_emit.rs` slice B, #1399)
/// converted that one remaining caller (`observation_call_record_types`) to
/// call this function directly instead — `ts_type_ref_qualified` had no
/// other production caller left, so it was deleted rather than kept as dead
/// code (the same "close it, don't leave a substitute-around orphan" call
/// slice 6's `ts_string_literal` deletion already made); its own direct unit
/// tests below now pin this function's output through `bynk_ts::print_type`
/// instead.
///
/// Not a new structural walk — `ts_type_ref_to_ts_type` (below) already
/// builds a real `TsType` from every `TypeRef` variant; this function is
/// that same build, just with the qualifying closure threaded through.
pub(crate) fn ts_type_ref_qualified_ts_type(
    r: &TypeRef,
    scope: &HashSet<String>,
    ns: &str,
) -> TsType {
    ts_type_ref_to_ts_type(
        r,
        Some(&|name| scope.contains(name).then(|| ns.to_string())),
    )
}

/// [`TsType`]-returning sibling of `ts_type_ref`, like `ts_type_ref_qualified`
/// but with each in-scope name carrying its *own* namespace via `type_ns`
/// rather than one shared `ns` — needed when a signature mixes names owned
/// by the target unit with names reached only through a `uses`d commons
/// (e.g. a stub class implementing an adapter-sourced capability whose
/// return type lives in a commons the capability's own unit `uses`, never
/// in the target context itself — Locale capability track, slice 1, #844).
/// Qualifying such a name under the target's own namespace would reference
/// an export `emit_context_rebrands` never emits (it only rebrands names
/// the target's *own* lowered body references), so each name is qualified
/// under the namespace that actually exports it. Used by `emit_stub_class`
/// (`project/tests_emit.rs`) for its own method params/return-type.
///
/// Originally paired with a `String`-returning `ts_type_ref_qualified_multi`
/// — Arc C slice 33 (`tests_emit.rs` slice C, #1401) converted `emit_stub_
/// class`'s own two real call sites (params/return-type) to call this
/// function directly instead, the same cleanup Arc C slice 32/#1399 already
/// made for `ts_type_ref_qualified`'s own identical pairing — `ts_type_ref_
/// qualified_multi` had no other production caller left, so it was deleted
/// rather than kept as dead code, with its own direct unit test rerouted
/// through `bynk_ts::print_type` instead.
pub(crate) fn ts_type_ref_qualified_multi_ts_type(
    r: &TypeRef,
    type_ns: &HashMap<String, String>,
) -> TsType {
    ts_type_ref_to_ts_type(r, Some(&|name| type_ns.get(name).cloned()))
}

/// A name → owning-namespace lookup for `ts_type_ref_with`'s `qualify` arm.
type QualifyFn<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Shared renderer behind `ts_type_ref` (`qualify = None`) and the two
/// `ts_type_ref_qualified*` helpers above (`qualify = Some(name -> namespace)`).
/// With `None` it is output-identical to the historic `ts_type_ref`; the only
/// divergence is the `Named`/`App` arms, which qualify in-scope names when
/// `qualify` is set.
/// P7.9 (#1315, R7.2): renders `r` by building a real [`bynk_ts::TsType`]
/// and printing it through [`bynk_ts::print_type`], instead of `format!`-ing
/// the type text by hand — this function's own signature and every one of
/// its ~110 real callers (via `ts_type_ref`/`ts_type_ref_qualified*`) are
/// unchanged; only the internal construction moved. `TypeRef` has no shape
/// this crate's `TsType` (`Named`/`Array`/`Object`/`Fn`, all extended for
/// this slice's own grounded gaps — a `readonly` modifier, and `Fn` itself)
/// can't represent — confirmed by matching every `TypeRef` variant below.
fn ts_type_ref_with(r: &TypeRef, qualify: Option<QualifyFn<'_>>) -> String {
    bynk_ts::print_type(&ts_type_ref_to_ts_type(r, qualify))
}

/// Whether `ty` is a bare (no type arguments) `Named` type whose name is
/// exactly `target` — used by the two arms below that special-case a
/// `Promise`/function return of `void`, checking the *structure* of the
/// already-built `TsType` instead of the pre-P7.9 code's own `inner == "()"
/// || inner == "void"` text comparison. No arm in [`ts_type_ref_to_ts_type`]
/// ever builds a literal `"()"`-named type, so that half of the check is
/// dead in practice today, same as it was before this slice — inherited
/// unchanged rather than dropped, since removing "impossible" defensive
/// code isn't this slice's job.
fn is_bare_named(ty: &TsType, target: &str) -> bool {
    matches!(ty, TsType::Named { name, type_args } if type_args.is_empty() && name == target)
}

/// #1883, slice 1a: the TypeScript shape of a Bynk `Asset` —
/// `{ readonly url: string }`, structural, so a program that only *names* the
/// type (`fn link(a: Asset) -> String`) compiles with no runtime support.
/// Each declared asset is emitted as a constant of this shape (`emit_project`).
pub(crate) fn asset_ts_type() -> TsType {
    TsType::Object(vec![bynk_ts::TsTypeMember::readonly_prop(
        "url",
        TsType::named("string"),
    )])
}

fn ts_type_ref_to_ts_type(r: &TypeRef, qualify: Option<QualifyFn<'_>>) -> TsType {
    match r {
        TypeRef::Base(b, _) => TsType::named(ts_base(*b)),
        TypeRef::Named(id) => {
            let name = if let Some(f) = qualify
                && let Some(ns) = f(&id.name)
            {
                format!("{ns}.{}", id.name)
            } else {
                id.name.clone()
            };
            TsType::named(name)
        }
        TypeRef::Result(t, e, _) => TsType::named_with_args(
            "Result",
            vec![
                ts_type_ref_to_ts_type(t, qualify),
                ts_type_ref_to_ts_type(e, qualify),
            ],
        ),
        TypeRef::Option(t, _) => {
            TsType::named_with_args("Option", vec![ts_type_ref_to_ts_type(t, qualify)])
        }
        TypeRef::Effect(t, _) => {
            let inner = ts_type_ref_to_ts_type(t, qualify);
            if is_bare_named(&inner, "()") || is_bare_named(&inner, "void") {
                TsType::named_with_args("globalThis.Promise", vec![TsType::named("void")])
            } else {
                TsType::named_with_args("globalThis.Promise", vec![inner])
            }
        }
        TypeRef::HttpResult(t, _) => {
            TsType::named_with_args("HttpResult", vec![ts_type_ref_to_ts_type(t, qualify)])
        }
        // v0.20b: collections lower to immutable TS shapes.
        TypeRef::List(t, _) => TsType::readonly_array(ts_type_ref_to_ts_type(t, qualify)),
        // A `Query[T]`'s own real shape is `(() => readonly T[])` — an
        // *outer* paren pair around the whole function type (found by the
        // zero-diff check this slice's own "Done when" requires: the
        // general `TsType::Fn` rendering, correctly matching the real
        // parametered-function-type case, has no such wrap, but every
        // fixture's own Query annotation does — e.g.
        // `bynkc/tests/fixtures/positive/302_query_annotated_let/expected/
        // probe.ts:46`). `TsType` has no general parenthesisation wrapper
        // (adding one for this single real use would be inventing a new
        // variant beyond this slice's own authorised gap list — `readonly`
        // and `Fn`, nothing else); pre-rendering the `Fn` type immediately
        // and wrapping the *text* in parens, carried as an opaque `Named`
        // with no type arguments (which prints its `name` verbatim,
        // byte-for-byte, whatever it contains), reproduces the real shape
        // exactly without widening the algebra. Worth reconsidering if a
        // second real case ever needs the same wrap.
        TypeRef::Query(t, _) => {
            let fn_ty = TsType::Fn {
                params: vec![],
                ret: Box::new(TsType::readonly_array(ts_type_ref_to_ts_type(t, qualify))),
            };
            TsType::named(format!("({})", bynk_ts::print_type(&fn_ty)))
        }
        // v0.100: `Stream[T]` lowers to a host async iterable.
        TypeRef::Stream(t, _) => {
            TsType::named_with_args("AsyncIterable", vec![ts_type_ref_to_ts_type(t, qualify)])
        }
        // v0.102: a `Connection[F]` lowers to the runtime `Connection<F>`
        // interface (the concrete implementation arrives with the protocol).
        TypeRef::Connection(t, _) => {
            TsType::named_with_args("Connection", vec![ts_type_ref_to_ts_type(t, qualify)])
        }
        // v0.119: `History[Agent]` is a test-only generator with no emitted TS
        // type — it never reaches a signature/field position (the property runner
        // binds the driven history as an ordinary array). Rendered defensively.
        TypeRef::History(_, _) => TsType::named("never"),
        TypeRef::Map(k, v, _) => TsType::named_with_args(
            "ReadonlyMap",
            vec![
                ts_type_ref_to_ts_type(k, qualify),
                ts_type_ref_to_ts_type(v, qualify),
            ],
        ),
        TypeRef::QueueResult(_) => TsType::named("QueueResult"),
        TypeRef::Asset(_) => asset_ts_type(),
        TypeRef::ValidationError(_) => TsType::named("ValidationError"),
        TypeRef::JsonError(_) => TsType::named("JsonError"),
        TypeRef::Unit(_) => TsType::named("void"),
        // v0.157 (ADR 0183): `Name[Arg, …]` lowers to the erased TS generic
        // `Name<Arg, …>` — the generic record's interface is emitted with the
        // same type parameters (like a generic function's erased `<A, B>`).
        TypeRef::App { name, args, .. } => {
            // Review of #1315/#1316: pre-slice this arm interpolated `head`
            // unconditionally, so a hypothetical empty-args `App` rendered
            // `Head<>` — `TsType::named_with_args(head, vec![])` instead
            // renders bare `Head` (`TsType::Named`'s own printer only opens
            // `<...>` when `type_args` is non-empty), a real behaviour
            // change *if* this arm is ever reached with empty `args`. It
            // isn't today: `ty_to_type_ref` only ever builds `App` with a
            // non-empty argument list (this file, `ty_to_type_ref`'s own
            // `Ty::Named` arm), and the resolver arity-checks a
            // `Name[Arg, …]` application before either side ever sees one.
            // Asserted, not silently relied on, so a future caller that
            // *can* reach here with no args fails loudly instead of
            // silently changing bytes.
            debug_assert!(
                !args.is_empty(),
                "TypeRef::App is only ever built with a non-empty argument list"
            );
            let head = if let Some(f) = qualify
                && let Some(ns) = f(&name.name)
            {
                format!("{ns}.{}", name.name)
            } else {
                name.name.clone()
            };
            let rendered: Vec<TsType> = args
                .iter()
                .map(|a| ts_type_ref_to_ts_type(a, qualify))
                .collect();
            TsType::named_with_args(head, rendered)
        }
        // v0.20a: a function type lowers to a TS function type. Positional
        // parameter names (`a0`, `a1`, …) are the printer's own job now
        // (`bynk_ts::print_type`'s `TsType::Fn` rendering) — TS requires
        // names in function type syntax; an Effect return is already
        // Promise via recursion.
        TypeRef::Fn(params, ret, _) => {
            let params: Vec<TsType> = params
                .iter()
                .map(|p| ts_type_ref_to_ts_type(p, qualify))
                .collect();
            let ret = ts_type_ref_to_ts_type(ret, qualify);
            let ret = if is_bare_named(&ret, "()") {
                TsType::named("void")
            } else {
                ret
            };
            TsType::Fn {
                params,
                ret: Box::new(ret),
            }
        }
    }
}

/// P7.9 (#1315): pins `ts_type_ref`'s exact pre-slice text for a
/// representative `TypeRef` from every real shape category, the same
/// discipline this track's own history names for "prove the test would
/// actually catch it" (P7.2's `brand_assertion` gap, P7.3's escaping gap —
/// both invisible to the byte-golden fixture corpus alone). These pin the
/// literal strings `ts_type_ref_with` built by hand before this slice
/// rebuilt it to construct a real `bynk_ts::TsType` and print that instead
/// — a regression here is a regression in every type-position string the
/// compiler emits, not caught by any one fixture necessarily exercising
/// this exact shape.
#[cfg(test)]
mod ts_type_ref_tests {
    use super::*;

    fn sp() -> bynk_syntax::span::Span {
        bynk_syntax::span::Span::new(0, 0)
    }

    fn base(b: BaseType) -> TypeRef {
        TypeRef::Base(b, sp())
    }

    fn named(name: &str) -> TypeRef {
        TypeRef::Named(Ident {
            name: name.to_string(),
            span: sp(),
        })
    }

    #[test]
    fn bare_named_type() {
        assert_eq!(ts_type_ref(&named("Order")), "Order");
    }

    #[test]
    fn result_type() {
        let r = TypeRef::Result(
            Box::new(base(BaseType::Int)),
            Box::new(named("MyError")),
            sp(),
        );
        assert_eq!(ts_type_ref(&r), "Result<number, MyError>");
    }

    #[test]
    fn option_type() {
        let r = TypeRef::Option(Box::new(base(BaseType::String)), sp());
        assert_eq!(ts_type_ref(&r), "Option<string>");
    }

    #[test]
    fn effect_of_a_non_unit_type_becomes_promise() {
        let r = TypeRef::Effect(Box::new(base(BaseType::Int)), sp());
        assert_eq!(ts_type_ref(&r), "globalThis.Promise<number>");
    }

    #[test]
    fn effect_of_unit_becomes_promise_void() {
        let r = TypeRef::Effect(Box::new(TypeRef::Unit(sp())), sp());
        assert_eq!(ts_type_ref(&r), "globalThis.Promise<void>");
    }

    #[test]
    fn http_result_type() {
        let r = TypeRef::HttpResult(Box::new(named("Order")), sp());
        assert_eq!(ts_type_ref(&r), "HttpResult<Order>");
    }

    #[test]
    fn list_is_a_readonly_array() {
        let r = TypeRef::List(Box::new(named("Order")), sp());
        assert_eq!(ts_type_ref(&r), "readonly Order[]");
    }

    /// The one shape this slice's own gap analysis found: `Query`'s real
    /// text wraps the whole function type in an *extra* outer paren pair
    /// (`(() => readonly T[])`), distinct from a bare `Fn` type's own
    /// convention (no outer wrap) — caught only by this zero-diff check
    /// against `bynkc/tests/fixtures/positive/302_query_annotated_let/
    /// expected/probe.ts`, not by reasoning about the algebra alone.
    #[test]
    fn query_wraps_the_whole_function_type_in_parens() {
        let r = TypeRef::Query(Box::new(named("Order")), sp());
        assert_eq!(ts_type_ref(&r), "(() => readonly Order[])");
    }

    #[test]
    fn stream_is_an_async_iterable() {
        let r = TypeRef::Stream(Box::new(named("Order")), sp());
        assert_eq!(ts_type_ref(&r), "AsyncIterable<Order>");
    }

    #[test]
    fn connection_type() {
        let r = TypeRef::Connection(Box::new(named("Frame")), sp());
        assert_eq!(ts_type_ref(&r), "Connection<Frame>");
    }

    #[test]
    fn history_renders_defensively_as_never() {
        let r = TypeRef::History(Box::new(named("Agent")), sp());
        assert_eq!(ts_type_ref(&r), "never");
    }

    #[test]
    fn map_is_a_readonly_map() {
        let r = TypeRef::Map(
            Box::new(base(BaseType::String)),
            Box::new(named("Order")),
            sp(),
        );
        assert_eq!(ts_type_ref(&r), "ReadonlyMap<string, Order>");
    }

    #[test]
    fn the_four_bare_named_types() {
        assert_eq!(ts_type_ref(&TypeRef::QueueResult(sp())), "QueueResult");
        assert_eq!(
            ts_type_ref(&TypeRef::ValidationError(sp())),
            "ValidationError"
        );
        assert_eq!(ts_type_ref(&TypeRef::JsonError(sp())), "JsonError");
        assert_eq!(ts_type_ref(&TypeRef::Unit(sp())), "void");
    }

    #[test]
    fn app_is_a_generic_instantiation() {
        let r = TypeRef::App {
            name: Ident {
                name: "MyGeneric".to_string(),
                span: sp(),
            },
            args: vec![base(BaseType::Int), named("Order")],
            span: sp(),
        };
        assert_eq!(ts_type_ref(&r), "MyGeneric<number, Order>");
    }

    #[test]
    fn real_function_type_uses_positional_parameter_names() {
        let r = TypeRef::Fn(
            vec![base(BaseType::String), base(BaseType::Int)],
            Box::new(named("Order")),
            sp(),
        );
        assert_eq!(ts_type_ref(&r), "(a0: string, a1: number) => Order");
    }

    #[test]
    fn base_types() {
        assert_eq!(ts_type_ref(&base(BaseType::Int)), "number");
        assert_eq!(ts_type_ref(&base(BaseType::Float)), "number");
        assert_eq!(ts_type_ref(&base(BaseType::Duration)), "number");
        assert_eq!(ts_type_ref(&base(BaseType::Instant)), "number");
        assert_eq!(ts_type_ref(&base(BaseType::String)), "string");
        assert_eq!(ts_type_ref(&base(BaseType::Bool)), "boolean");
        assert_eq!(ts_type_ref(&base(BaseType::Bytes)), "globalThis.Uint8Array");
    }

    /// Coverage gap named in review of #1315/#1316: every test above goes
    /// through `ts_type_ref` (`qualify = None`) — the qualifying branches
    /// this slice's own refactor most directly restructured (`App`'s `head`
    /// now flows through `TsType::named_with_args` instead of being
    /// interpolated) were unpinned. Asserts both that the head qualifies
    /// *and* that qualification recurses into the type arguments.
    #[test]
    fn qualified_generic_instantiation_qualifies_head_and_recurses_into_args() {
        let r = TypeRef::App {
            name: Ident {
                name: "MyGeneric".to_string(),
                span: sp(),
            },
            args: vec![base(BaseType::Int), named("Order")],
            span: sp(),
        };
        let mut scope = HashSet::new();
        scope.insert("MyGeneric".to_string());
        scope.insert("Order".to_string());
        assert_eq!(
            bynk_ts::print_type(&ts_type_ref_qualified_ts_type(&r, &scope, "Ns")),
            "Ns.MyGeneric<number, Ns.Order>"
        );
    }

    /// Same shape as above, through `ts_type_ref_qualified_multi_ts_type` —
    /// no direct test existed for this function at all before this fix.
    #[test]
    fn qualified_multi_generic_instantiation_qualifies_head_and_recurses_into_args() {
        let r = TypeRef::App {
            name: Ident {
                name: "MyGeneric".to_string(),
                span: sp(),
            },
            args: vec![base(BaseType::Int), named("Order")],
            span: sp(),
        };
        let mut type_ns = HashMap::new();
        type_ns.insert("MyGeneric".to_string(), "A".to_string());
        type_ns.insert("Order".to_string(), "B".to_string());
        assert_eq!(
            bynk_ts::print_type(&ts_type_ref_qualified_multi_ts_type(&r, &type_ns)),
            "A.MyGeneric<number, B.Order>"
        );
    }
}

/// v0.20b: render a checker `Ty` as a TypeScript type. Used by the inline
/// kernel-method lowerings, whose IIFE parameters must be annotated
/// (`noImplicitAny`). Rigid type variables render as themselves — inside an
/// emitted generic function they are in scope as TS type parameters.
/// P7.9 (#1315, R7.2): renders `t` by building a real [`bynk_ts::TsType`]
/// and printing it through [`bynk_ts::print_type`], instead of `format!`-ing
/// the type text by hand — this function's own signature and every one of
/// its ~45 real callers are unchanged; only the internal construction moved.
/// `Ty::ActorSum` needed a new gap closed beyond the accepted proposal's own
/// `readonly`/`Fn` list — a real, grounded one (found during implementation
/// review, not speculative): a resolved multi-actor sum lowers to a genuine
/// type-position union no existing `TsType` variant could represent, closed
/// by adding `TsType::Union` (see its own doc).
fn ts_ty(t: TyId, tys: &Arc<Types>) -> String {
    bynk_ts::print_type(&ts_ty_to_ts_type(t, tys))
}

/// `elem_ts` (a [`ts_ty`] rendering) made safe to suffix with `[]`, for the
/// kernel lowerings that `format!` an array type around an element's text
/// rather than building a [`TsType::Array`]. `[]` binds tighter than a prefix
/// `readonly`, a function arrow, `|` and `&`, so `readonly readonly T[][]`
/// (TS1354) or `(a0: T) => T[]` would be misread; such an element is
/// parenthesised. The check is on text, so it is conservative — an operator
/// nested inside a generic argument parenthesises too, which is redundant but
/// still valid TypeScript. The printer's own `TsType::Array` arm applies the
/// exact structural rule.
fn ts_array_elem(elem_ts: &str) -> String {
    if elem_ts.starts_with("readonly ")
        || elem_ts.contains("=>")
        || elem_ts.contains('|')
        || elem_ts.contains('&')
    {
        format!("({elem_ts})")
    } else {
        elem_ts.to_string()
    }
}

/// #1927: how the module being emitted spells a type it names. A unit's own
/// value (a function, asset, service, agent or provider) can hide a
/// `uses`-imported type of its name. The unit's source may not write that
/// type, but the lowering still names the types of checked expressions (#1778:
/// a kernel's `(__x: Name) => …`, an `if` slot, `List.empty`), and in this
/// module `Name` is the local value: a `const`, or for an agent or provider a
/// `class`, whose instance type would capture the annotation. So while such a
/// module is emitted, each hidden type is spelled as an inline import type,
/// `import("<module>").Name`, which needs no import statement and names the
/// declaring module's type whatever this module declares. Agents are never
/// `Ty::Named`, so within the unit a hidden name always means the imported
/// type.
///
/// Scoped per module, not threaded: [`ts_ty`] has ~45 callers and no module
/// context. [`type_renames::enter`] sets the spellings for this thread until its
/// guard drops, and a module is lowered on one thread.
pub(crate) mod type_renames {
    use std::cell::RefCell;
    use std::collections::HashMap;

    thread_local! {
        static SPELLINGS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    }

    /// Restores the spellings in force before [`enter`] when it drops.
    pub(crate) struct Guard(HashMap<String, String>);

    impl Drop for Guard {
        fn drop(&mut self) {
            let previous = std::mem::take(&mut self.0);
            SPELLINGS.with(|s| *s.borrow_mut() = previous);
        }
    }

    /// Spell each type in `spellings` (name → TypeScript text) until the
    /// returned guard drops.
    #[must_use]
    pub(crate) fn enter(spellings: HashMap<String, String>) -> Guard {
        Guard(SPELLINGS.with(|s| std::mem::replace(&mut *s.borrow_mut(), spellings)))
    }

    /// How the current module spells the type `name`: its rename, if any.
    pub(crate) fn spelling(name: &str) -> String {
        SPELLINGS
            .with(|s| s.borrow().get(name).cloned())
            .unwrap_or_else(|| name.to_string())
    }
}

fn ts_ty_to_ts_type(t: TyId, tys: &Arc<Types>) -> TsType {
    match &*tys.get(t) {
        // bynk internal error (finding #28, R4.3): `Ty::Error` records a
        // resolution failure, which per R4.3 is always accompanied by a
        // pushed diagnostic — the check that produced it should have failed
        // the whole program and never reached emission. A loud failure here
        // beats silently emitting a type for a node the checker gave up on.
        Ty::Error => panic!(
            "bynk internal error (finding #28): emitter asked to render `Ty::Error` as a \
             TypeScript type — a checked program should never contain one"
        ),
        Ty::Base(b) => TsType::named(ts_base(*b)),
        // #1977: the compiler-known `MapEntry[K, V]` (ADR 0184) has no
        // `TypeDecl`, so no module declares or imports a `MapEntry` type to
        // name. It renders as its own shape, the `{ key, value }` record the
        // `.entries` lowering builds. (A user type of the same name, which the
        // checker prefers for field access, interns to the same `Ty` and so
        // renders inline too; no program declares one today.)
        Ty::Named {
            name,
            kind: NamedKind::Record,
            args,
        } if name == MAP_ENTRY && args.len() == 2 => TsType::Object(vec![
            bynk_ts::TsTypeMember::readonly_prop("key", ts_ty_to_ts_type(args[0], tys)),
            bynk_ts::TsTypeMember::readonly_prop("value", ts_ty_to_ts_type(args[1], tys)),
        ]),
        // v0.157 (ADR 0183): a generic record instantiation renders as the
        // erased TS generic `Name<Arg, …>`; a non-generic named type is bare.
        Ty::Named { name, args, .. } if args.is_empty() => {
            TsType::named(type_renames::spelling(name))
        }
        Ty::Named { name, args, .. } => TsType::named_with_args(
            type_renames::spelling(name),
            args.iter().map(|a| ts_ty_to_ts_type(*a, tys)).collect(),
        ),
        Ty::Result(t, e) => TsType::named_with_args(
            "Result",
            vec![ts_ty_to_ts_type(*t, tys), ts_ty_to_ts_type(*e, tys)],
        ),
        Ty::Option(t) => TsType::named_with_args("Option", vec![ts_ty_to_ts_type(*t, tys)]),
        Ty::Effect(t) => match &*tys.get(*t) {
            Ty::Unit => TsType::named_with_args("globalThis.Promise", vec![TsType::named("void")]),
            _ => TsType::named_with_args("globalThis.Promise", vec![ts_ty_to_ts_type(*t, tys)]),
        },
        Ty::HttpResult(t) => TsType::named_with_args("HttpResult", vec![ts_ty_to_ts_type(*t, tys)]),
        Ty::List(t) => TsType::readonly_array(ts_ty_to_ts_type(*t, tys)),
        // v0.91 (ADR 0119): a `Query[T]` lowers to a deferred producer of its
        // elements — a thunk run by the terminal. Same outer-paren shape as
        // `TypeRef::Query` (`ts_type_ref_to_ts_type`'s own comment explains
        // the opaque-`Named` wrap this reuses, unchanged reasoning).
        Ty::Query(t) => {
            let fn_ty = TsType::Fn {
                params: vec![],
                ret: Box::new(TsType::readonly_array(ts_ty_to_ts_type(*t, tys))),
            };
            TsType::named(format!("({})", bynk_ts::print_type(&fn_ty)))
        }
        // v0.100: a `Stream[T]` lowers to a host async iterable.
        Ty::Stream(t) => TsType::named_with_args("AsyncIterable", vec![ts_ty_to_ts_type(*t, tys)]),
        // v0.102: a `Connection[F]` lowers to the runtime `Connection<F>` interface.
        Ty::Connection(t) => TsType::named_with_args("Connection", vec![ts_ty_to_ts_type(*t, tys)]),
        Ty::Map(k, v) => TsType::named_with_args(
            "ReadonlyMap",
            vec![ts_ty_to_ts_type(*k, tys), ts_ty_to_ts_type(*v, tys)],
        ),
        Ty::QueueResult => TsType::named("QueueResult"),
        // #1934: produced only by a `system`-tier test-service address, so
        // only a test module meets them: the runtime's `HttpOutcome<T>`, and
        // the shape of its `Rejected` payload.
        Ty::HttpOutcome(t) => {
            TsType::named_with_args("HttpOutcome", vec![ts_ty_to_ts_type(*t, tys)])
        }
        Ty::BoundaryRejection => TsType::named("{ readonly tag: string; readonly detail: string }"),
        Ty::Asset => asset_ts_type(),
        Ty::ValidationError => TsType::named("ValidationError"),
        Ty::JsonError => TsType::named("JsonError"),
        Ty::Unit => TsType::named("void"),
        Ty::Fn { params, ret } => TsType::Fn {
            params: params.iter().map(|p| ts_ty_to_ts_type(*p, tys)).collect(),
            ret: Box::new(ts_ty_to_ts_type(*ret, tys)),
        },
        Ty::Var(n) => TsType::named(n.clone()),
        // The identity type the actor binding yields (`name.identity`).
        Ty::Actor(id) => ts_ty_to_ts_type(*id, tys),
        // v0.52: a resolved multi-actor sum lowers to a discriminated union
        // tagged by actor name; non-unit members carry their identity.
        // Each member's own real text is `, `-separated (`{ tag: "x",
        // identity: T }`) — `TsType::Object`'s own renderer is `; `-
        // separated (the ordinary TS object-*type* convention, correct for
        // every other real caller, e.g. `events_fanout.rs`'s own interface
        // members), so building each member as an `Object` would silently
        // change this one shape's separator and fail the zero-diff check
        // (caught exactly that way — a fixture-corpus-adjacent direct test
        // failure, not reasoning about the algebra). Each member is instead
        // an opaque `Named`-carries-verbatim-text node, the same convention
        // `ts_type_ref_to_ts_type`'s own `Query` arm already established for
        // a single real shape `TsType`'s general renderers don't reproduce
        // byte-for-byte — not a new pattern, the second real use of one.
        Ty::ActorSum(members) => TsType::union(
            members
                .iter()
                .map(|(name, id)| match &*tys.get(*id) {
                    Ty::Unit => TsType::named(format!("{{ tag: \"{name}\" }}")),
                    _ => TsType::named(format!(
                        "{{ tag: \"{name}\", identity: {} }}",
                        bynk_ts::print_type(&ts_ty_to_ts_type(*id, tys))
                    )),
                })
                .collect(),
        ),
    }
}

/// P7.9 (#1315): pins `ts_ty`'s exact pre-slice text for a representative
/// `Ty` from every real shape category — the same "prove the test would
/// actually catch it" discipline `ts_type_ref_tests` above already applies
/// to `ts_type_ref`, and doubly important here since `ts_ty` is where
/// `TsType::Union`/`ActorSum` (added in review of this same PR, beyond the
/// accepted proposal's own gap list) is the one real, exercised caller.
#[cfg(test)]
mod ts_ty_tests {
    use super::*;
    use bynk_check::checker::{NamedKind, Types};

    #[test]
    fn base_types() {
        let tys = Arc::new(Types::new());
        assert_eq!(ts_ty(tys.intern(Ty::Base(BaseType::Int)), &tys), "number");
    }

    #[test]
    fn named_type_bare_and_generic() {
        let tys = Arc::new(Types::new());
        let bare = tys.intern(Ty::Named {
            name: "Order".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        assert_eq!(ts_ty(bare, &tys), "Order");

        let arg = tys.intern(Ty::Base(BaseType::String));
        let generic = tys.intern(Ty::Named {
            name: "Paginated".to_string(),
            kind: NamedKind::Record,
            args: vec![arg],
        });
        assert_eq!(ts_ty(generic, &tys), "Paginated<string>");
    }

    #[test]
    fn map_entry_renders_its_own_shape() {
        // #1977: no module declares `MapEntry`, so it is never named.
        let tys = Arc::new(Types::new());
        let k = tys.intern(Ty::Base(BaseType::String));
        let v = tys.intern(Ty::Base(BaseType::Int));
        let entry = bynk_check::checker::map_entry_ty(k, v, &tys);
        assert_eq!(
            ts_ty(entry, &tys),
            "{ readonly key: string; readonly value: number }"
        );
        let list = tys.intern(Ty::List(entry));
        assert_eq!(
            ts_ty(list, &tys),
            "readonly { readonly key: string; readonly value: number }[]"
        );
    }

    #[test]
    fn result_option_effect_http_result() {
        let tys = Arc::new(Types::new());
        let int = tys.intern(Ty::Base(BaseType::Int));
        let err = tys.intern(Ty::Named {
            name: "MyError".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        let result = tys.intern(Ty::Result(int, err));
        assert_eq!(ts_ty(result, &tys), "Result<number, MyError>");

        let option = tys.intern(Ty::Option(int));
        assert_eq!(ts_ty(option, &tys), "Option<number>");

        let effect = tys.intern(Ty::Effect(int));
        assert_eq!(ts_ty(effect, &tys), "globalThis.Promise<number>");

        let unit = tys.intern(Ty::Unit);
        let effect_unit = tys.intern(Ty::Effect(unit));
        assert_eq!(ts_ty(effect_unit, &tys), "globalThis.Promise<void>");

        let http_result = tys.intern(Ty::HttpResult(int));
        assert_eq!(ts_ty(http_result, &tys), "HttpResult<number>");
    }

    #[test]
    fn list_map_stream_connection() {
        let tys = Arc::new(Types::new());
        let order = tys.intern(Ty::Named {
            name: "Order".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        assert_eq!(ts_ty(tys.intern(Ty::List(order)), &tys), "readonly Order[]");
        // A nested list's inner readonly array is parenthesised (TS1354).
        let orders = tys.intern(Ty::List(order));
        assert_eq!(
            ts_ty(tys.intern(Ty::List(orders)), &tys),
            "readonly (readonly Order[])[]"
        );
        assert_eq!(ts_array_elem(&ts_ty(orders, &tys)), "(readonly Order[])");
        assert_eq!(ts_array_elem(&ts_ty(order, &tys)), "Order");

        let s = tys.intern(Ty::Base(BaseType::String));
        assert_eq!(
            ts_ty(tys.intern(Ty::Map(s, order)), &tys),
            "ReadonlyMap<string, Order>"
        );
        assert_eq!(
            ts_ty(tys.intern(Ty::Stream(order)), &tys),
            "AsyncIterable<Order>"
        );
        assert_eq!(
            ts_ty(tys.intern(Ty::Connection(order)), &tys),
            "Connection<Order>"
        );
    }

    /// The same shared shape as `TypeRef::Query` — pins that `ts_ty`'s own
    /// `Query` arm reproduces the identical outer-paren wrap after
    /// conversion, not just `ts_type_ref`'s.
    #[test]
    fn query_wraps_the_whole_function_type_in_parens() {
        let tys = Arc::new(Types::new());
        let order = tys.intern(Ty::Named {
            name: "Order".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        assert_eq!(
            ts_ty(tys.intern(Ty::Query(order)), &tys),
            "(() => readonly Order[])"
        );
    }

    #[test]
    fn bare_types_and_fn() {
        let tys = Arc::new(Types::new());
        assert_eq!(ts_ty(tys.intern(Ty::QueueResult), &tys), "QueueResult");
        assert_eq!(
            ts_ty(tys.intern(Ty::ValidationError), &tys),
            "ValidationError"
        );
        assert_eq!(ts_ty(tys.intern(Ty::JsonError), &tys), "JsonError");
        assert_eq!(ts_ty(tys.intern(Ty::Unit), &tys), "void");

        let int = tys.intern(Ty::Base(BaseType::Int));
        let s = tys.intern(Ty::Base(BaseType::String));
        let f = tys.intern(Ty::Fn {
            params: vec![int, s],
            ret: int,
        });
        assert_eq!(ts_ty(f, &tys), "(a0: number, a1: string) => number");
    }

    /// The real gap found during implementation review: a resolved
    /// multi-actor sum lowers to a genuine type-position union, tagged by
    /// actor name, non-unit members carrying their identity — the shape
    /// that motivated adding `TsType::Union` beyond the accepted proposal's
    /// own `readonly`/`Fn` gap list.
    #[test]
    fn actor_sum_is_a_tagged_union() {
        let tys = Arc::new(Types::new());
        let unit = tys.intern(Ty::Unit);
        let identity = tys.intern(Ty::Named {
            name: "AdminIdentity".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        let sum = tys.intern(Ty::ActorSum(vec![
            ("Guest".to_string(), unit),
            ("Admin".to_string(), identity),
        ]));
        assert_eq!(
            ts_ty(sum, &tys),
            "{ tag: \"Guest\" } | { tag: \"Admin\", identity: AdminIdentity }"
        );
    }

    /// Coverage gap named in review of #1315/#1316: `Ty::Var` (a type
    /// variable's own bare name) and `Ty::Actor` (delegates to its
    /// identity type) were unpinned.
    #[test]
    fn var_and_actor() {
        let tys = Arc::new(Types::new());
        assert_eq!(ts_ty(tys.intern(Ty::Var("T".to_string())), &tys), "T");

        let identity = tys.intern(Ty::Named {
            name: "AdminIdentity".to_string(),
            kind: NamedKind::Record,
            args: vec![],
        });
        let actor = tys.intern(Ty::Actor(identity));
        assert_eq!(ts_ty(actor, &tys), "AdminIdentity");
    }
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
/// position (params, locals, function names, import names). Bynk identifiers
/// that are illegal as TS binding names — the JS reserved words plus the
/// strict-mode/module sets (emitted modules are always strict ESM) — and
/// names the emitter itself introduces alongside user bindings (`deps`) are
/// renamed into the generated-name namespace (`__id_<name>`), which the
/// parser keeps free of user identifiers. Property/field names never pass
/// through here: reserved words are legal there, and record field names are
/// wire format.
pub(crate) fn ts_ident(name: &str) -> String {
    const RESERVED: &[&str] = &[
        // ES reserved words.
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "debugger",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "function",
        "if",
        "import",
        "in",
        "instanceof",
        "new",
        "null",
        "return",
        "super",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "typeof",
        "var",
        "void",
        "while",
        "with",
        // Strict-mode reserved (emitted modules are always strict).
        "implements",
        "interface",
        "let",
        "package",
        "private",
        "protected",
        "public",
        "static",
        "yield",
        // Module-code reserved.
        "await",
        // Illegal binding targets in strict mode.
        "arguments",
        "eval",
        // #1653: the emitted code reaches host globals through `globalThis`
        // (`globalThis.JSON`), so no user binding may take that name.
        "globalThis",
        // Generated identifiers a user binding may sit next to: handler
        // signatures append a `deps` parameter, so a user param named `deps`
        // would otherwise duplicate it.
        "deps",
    ];
    if RESERVED.contains(&name) {
        format!("__id_{name}")
    } else {
        name.to_string()
    }
}

/// Delegates to `bynk_check::wire_default::escape_ts_literal` — the two
/// splice into generated TypeScript from opposite sides (real emission here,
/// event-field wire defaults there), so a correction to the escaping rules
/// must land once, not drift between two copies.
pub(crate) fn escape_ts_string(s: &str) -> String {
    bynk_check::wire_default::escape_ts_literal(s)
}

/// #661 (Decision D)/#70 review: the one `PredKind` → runtime-check mapping,
/// shared by the owner-side check (`emit::emit_pred_check`, over a `value`
/// binding) and the boundary-side inline check
/// (`serialisation::emit_inline_pred_check`, over a `json` binding) — the
/// two used to hand-roll this mapping independently, pinned identical only by
/// a comment, so amending one (e.g. the `Matches` regex's `^(?:…)$` anchoring)
/// could silently drift from the other. `receiver` is the bound name the
/// generated condition reads (`value` or `json`); the returned message is the
/// same either side of the boundary by construction.
///
/// #1471: `cond` is a real [`bynk_ts::TsExpr`], not opaque text — the
/// blocker (`bynk_ts::TsBinaryOp` had no `>=`/`<=`) is resolved by that
/// enum's own new `GreaterThanEq`/`LessThanEq` variants. Both real callers
/// wrap the returned expression in their own `Unary::Not`/`Paren` unchanged;
/// only this function's own arms changed, from `format!`ing condition text
/// to building the equivalent node tree.
///
/// Review of #1336: both real callers (`emit::emit_pred_check`,
/// `serialisation::emit_inline_pred_check`) splice the returned `message`
/// straight into a TypeScript string literal **unescaped** — safe today only
/// because every arm's own message is either static English text or already
/// `escape_ts_string`-escaped (the `Matches` arm's own pattern). A future arm
/// returning raw, unescaped text (a predicate carrying a user-supplied string
/// operand, say) would emit a malformed or injectable literal at both call
/// sites, and nothing in the existing fixture corpus would catch it. This
/// invariant must match at every arm added here, not just the ones that exist
/// today — return plain text or already-`escape_ts_string`-escaped text only.
/// `msg` stays exactly this opaque `String`, unchanged by #1471: only the
/// `cond` side of the pair became a real node (see the `Matches` arm below
/// for why the message keeps its own already-escaped copy of the pattern
/// separate from the condition's raw, unescaped one).
pub(crate) fn pred_condition_and_message(
    pred: &PredKind,
    receiver: &str,
) -> (bynk_ts::TsExpr, String) {
    use bynk_ts::{TsBinaryOp, TsExpr, TsLit};

    let recv = || TsExpr::Ident(receiver.to_string());
    let recv_length = || TsExpr::Member {
        object: Box::new(recv()),
        property: "length".to_string(),
    };
    let num = |n: String| TsExpr::Lit(TsLit::Num(n));
    let cmp = |op, left, right| TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    };

    match pred {
        PredKind::NonNegative => (
            cmp(TsBinaryOp::GreaterThanEq, recv(), num("0".to_string())),
            "must be non-negative".to_string(),
        ),
        PredKind::Positive => (
            cmp(TsBinaryOp::GreaterThan, recv(), num("0".to_string())),
            "must be positive".to_string(),
        ),
        PredKind::InRange(a, b) => {
            let (a, b) = (a.value, b.value);
            (
                cmp(
                    TsBinaryOp::And,
                    cmp(TsBinaryOp::GreaterThanEq, recv(), num(a.to_string())),
                    cmp(TsBinaryOp::LessThanEq, recv(), num(b.to_string())),
                ),
                format!("must be in range [{a}, {b}]"),
            )
        }
        PredKind::InRangeF(a, b) => {
            let (a, b) = (&a.lexeme, &b.lexeme);
            (
                cmp(
                    TsBinaryOp::And,
                    cmp(TsBinaryOp::GreaterThanEq, recv(), num(a.clone())),
                    cmp(TsBinaryOp::LessThanEq, recv(), num(b.clone())),
                ),
                format!("must be in range [{a}, {b}]"),
            )
        }
        PredKind::NonEmpty => (
            cmp(TsBinaryOp::GreaterThan, recv_length(), num("0".to_string())),
            "must be non-empty".to_string(),
        ),
        PredKind::MinLength(n) => (
            cmp(TsBinaryOp::GreaterThanEq, recv_length(), num(n.to_string())),
            format!("length must be at least {n}"),
        ),
        PredKind::MaxLength(n) => (
            cmp(TsBinaryOp::LessThanEq, recv_length(), num(n.to_string())),
            format!("length must be at most {n}"),
        ),
        PredKind::Length(n) => (
            cmp(TsBinaryOp::StrictEq, recv_length(), num(n.to_string())),
            format!("length must be exactly {n}"),
        ),
        PredKind::Matches(pat) => {
            // The condition's own `RegExp` source uses `pat` raw, not
            // `escaped` — `TsLit::Str`'s printer already applies the exact
            // same escaping as `escape_ts_string`
            // (`bynk_check::wire_default::escape_ts_literal`, kept
            // deliberately identical — see `bynk-ts/src/printer.rs`'s own
            // `render_lit`), so escaping `pat` here too would double-escape
            // it, same bug class `msg`'s own doc above already argues
            // against. `escaped` is still needed for `msg`, which is opaque
            // text spliced with no further escaping.
            let escaped = escape_ts_string(pat);
            let pattern = cmp(
                TsBinaryOp::Add,
                cmp(
                    TsBinaryOp::Add,
                    TsExpr::Lit(TsLit::Str("^(?:".to_string())),
                    TsExpr::Lit(TsLit::Str(pat.clone())),
                ),
                TsExpr::Lit(TsLit::Str(")$".to_string())),
            );
            let regexp = TsExpr::New {
                callee: Box::new(TsExpr::Ident("globalThis.RegExp".to_string())),
                args: vec![pattern],
            };
            let test_call = TsExpr::Call {
                callee: Box::new(TsExpr::Member {
                    object: Box::new(regexp),
                    property: "test".to_string(),
                }),
                args: vec![recv()],
            };
            (test_call, format!("must match /{escaped}/"))
        }
    }
}

#[allow(dead_code)]
fn _unused_hashmap(_h: HashMap<String, ()>) {}

#[cfg(test)]
mod runtime_tests {
    use super::*;

    #[test]
    fn runtime_emits_all_required_exports() {
        let s = emit_runtime_module();
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
    fn coverage_tsconfig_enables_source_maps() {
        // #854: the coverage remap consumes tsc's `.js.map`s, so the variant must
        // set `sourceMap` — a guard against a silent string-replace miss if the
        // base config's `outDir` line is ever reworded. The default stays map-free
        // so a normal `bynkc test` / deployment build ships no `.js.map`s.
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
            crate::project::worker_dir_name("commerce.payment"),
            "commerce-payment"
        );
        assert_eq!(crate::project::worker_dir_name("a.b.c"), "a-b-c");
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

    /// The import line is the first `import { … } from "./runtime.js"` in the
    /// emitted module.
    fn runtime_import_line(ts: &str) -> &str {
        ts.lines()
            .find(|l| l.starts_with("import {") && l.contains("runtime.js"))
            .unwrap_or("")
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
        TypeRef::JsonError(bynk_syntax::span::Span::new(0, 0))
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
        assert!(!type_ref_mentions(&t, TypeRefMarker::JsonError));
    }

    #[test]
    fn a_non_matching_wrapper_still_recurses_into_its_inner_type() {
        // `HttpResult[Connection[String]]` does not match the `HttpResult`
        // marker itself when the marker is `Connection` — it must still find
        // the `Connection` nested one level in.
        let t = http_result(connection(base(BaseType::String)));
        assert!(type_ref_mentions(&t, TypeRefMarker::Connection));
        assert!(type_ref_mentions(&t, TypeRefMarker::HttpResult));
        assert!(!type_ref_mentions(&t, TypeRefMarker::JsonError));
    }

    #[test]
    fn json_error_has_no_inner_type_to_recurse_into() {
        let t = json_error();
        assert!(type_ref_mentions(&t, TypeRefMarker::JsonError));
        assert!(!type_ref_mentions(&t, TypeRefMarker::HttpResult));
        assert!(!type_ref_mentions(&t, TypeRefMarker::Connection));
    }

    #[test]
    fn a_plain_base_type_matches_no_marker() {
        let t = base(BaseType::Int);
        assert!(!type_ref_mentions(&t, TypeRefMarker::JsonError));
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
            crate::project::BuildTarget::Bundle,
        );
        let app = out.text("d/app.ts");
        assert!(
            app.lines()
                .any(|l| l.starts_with("import") && l.contains("lim") && l.contains("./lim.js")),
            "{app}"
        );
    }
}
