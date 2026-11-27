//! The module's own boundary codecs (2.5b, #4090): which types cross the module's
//! boundary (`collect_json_codec_roots`, `agent_call_boundary_roots`), the codecs and
//! helpers it emits for them (`emit_json_codec_helpers`, `emit_boundary_helpers`) and
//! how a type is named across files (`ty_to_type_ref`, `codec_import_spec`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use super::consumed::emit_consumed_context_helpers;
use super::{codec, serialisation};
use super::{commons_alias, cross_commons_import_specifier_for_path};
use crate::emit_model::EmitProjectCtx;
use bynk_check::checker::{CheckedProgram, Ty, TyId, TypedCommons};
use bynk_check::kernel_op::{FormStatic, JsonStatic, StaticOp, YamlStatic};
use bynk_check::project_model::BuildTarget;
use bynk_model::walk_block_exprs;
use bynk_project::UnitKind;
use bynk_syntax::ast::{AgentDecl, CommonsItem, Expr, ExprKind, Ident, ServiceDecl, TypeRef};

/// A checker `Ty` rendered back to a `TypeRef` for the codec machinery; the one
/// conversion lives in `bynk_check::ty_ref` (#3879), which the queue check shares.
pub use bynk_check::ty_ref::ty_to_type_ref;

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
                // #4105: a typed Analytics read decodes its rows as a `List`.
                Some(bynk_check::checker::Callee::AnalyticsSelect { row, .. }) => {
                    push_ground(commons.tys().intern(Ty::List(*row)), &mut roots);
                }
                // #4099: `Ai.generateValue[T]` decodes the reply as `T`.
                Some(bynk_check::checker::Callee::AiGenerateValue { value, .. }) => {
                    push_ground(*value, &mut roots);
                }
                // #4101: a typed Vectorize read decodes each match's metadata.
                Some(bynk_check::checker::Callee::VectorValueRead { meta, .. }) => {
                    push_ground(*meta, &mut roots);
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
                | StaticOp::Forwarded(_)
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
pub(super) fn emit_json_codec_helpers(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
    skip_names: &HashSet<String>,
    skip_insts: &HashSet<String>,
) -> Vec<bynk_ts::TsStmt> {
    use super::serialisation::{
        collect_codec_closure, emit_generic_helpers, emit_helpers_for_owner,
    };
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

/// #4549: the `out` frame type of each `from websocket` service. It is written by its codec (on `workers`, by the
/// socket's `send`; on `bundle`, by the `TestConnection` a case opens), so it is a boundary type too: the `in` type
/// already is, as `on message`'s parameter. In service-name order.
fn ws_out_roots(commons: &TypedCommons) -> Vec<TypeRef> {
    let mut services: Vec<&ServiceDecl> = commons
        .commons
        .items
        .iter()
        .filter_map(CommonsItem::as_service)
        .collect();
    services.sort_by(|a, b| a.name.name.cmp(&b.name.name));
    services
        .into_iter()
        .filter_map(|s| match &s.protocol {
            bynk_syntax::ast::ServiceProtocol::WebSocket { out_type, .. } => Some(out_type.clone()),
            bynk_syntax::ast::ServiceProtocol::Call
            | bynk_syntax::ast::ServiceProtocol::Http
            | bynk_syntax::ast::ServiceProtocol::Cron
            | bynk_syntax::ast::ServiceProtocol::Queue { .. }
            | bynk_syntax::ast::ServiceProtocol::Events { .. }
            | bynk_syntax::ast::ServiceProtocol::Email { .. } => None,
        })
        .collect()
}

/// #4632: the events this unit emits through `Events.emit`, as types, in name order. The checker records the
/// event on each call's callee (#4113).
fn emitted_event_roots(commons: &TypedCommons) -> Vec<TypeRef> {
    let names: std::collections::BTreeSet<&String> = commons
        .callees
        .values()
        .filter_map(|c| {
            if let bynk_check::checker::Callee::Capability {
                event: Some(name),
                first_party: Some(bynk_check::firstparty::FirstPartyCap::Events),
                ..
            } = c
            {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    names
        .into_iter()
        .map(|n| {
            TypeRef::Named(Ident {
                name: n.clone(),
                span: Default::default(),
            })
        })
        .collect()
}

/// The type positions beyond the services' own that cross a wire in this unit: on `workers`, every agent
/// handler's parameters and return type (#1678) and each event the unit emits (#4632); on both targets, a
/// websocket service's `out` frame (#4549). On `bundle` an agent call and an event dispatch are in-process,
/// and need no codec.
fn wire_boundary_roots(
    commons: &TypedCommons,
    agents: &HashMap<String, AgentDecl>,
    workers: bool,
    inspect_agents: bool,
) -> Vec<TypeRef> {
    let mut roots = if workers {
        agent_call_boundary_roots(agents, inspect_agents)
    } else {
        Vec::new()
    };
    roots.extend(ws_out_roots(commons));
    if workers {
        roots.extend(emitted_event_roots(commons));
    }
    roots
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

pub(super) fn emit_boundary_helpers(
    program: &CheckedProgram,
    ctx: &EmitProjectCtx,
) -> (Vec<bynk_ts::TsStmt>, HashSet<String>, HashSet<String>) {
    use super::serialisation::{
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
        let agent_call_roots = wire_boundary_roots(commons, &agents, workers, ctx.inspect_agents);
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
                    .entry(commons_decl_path_of_key(ctx, commons_name, n))
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
            let parts: Vec<String> = names.iter().flat_map(|n| codec_names(n)).collect();
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

/// #4403: [`commons_decl_path`] for a type key, which may be a qualified spelling of a name.
fn commons_decl_path_of_key(ctx: &EmitProjectCtx, commons_name: &str, key: &str) -> PathBuf {
    commons_decl_path(
        ctx,
        commons_name,
        bynk_check::emitted_names::declared_name_of_key(key),
    )
}

/// #4403: the names of the two codec helpers of the type `key` in this module.
fn codec_names(key: &str) -> [String; 2] {
    let key = bynk_check::emitted_names::codec_key(key);
    [format!("__serialise_{key}"), format!("__deserialise_{key}")]
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
            commons_decl_path(
                ctx,
                commons_name,
                bynk_check::emitted_names::declared_name_of_key(&n),
            )
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
pub(super) fn unbranded_commons_qual(
    commons: &TypedCommons,
    ctx: &EmitProjectCtx,
) -> HashMap<String, String> {
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
        .map(|n| {
            // #4403: a name a `uses` clause qualifies (`lib.x.Status`, `X.Status`) is read
            // through the commons' namespace import, which is unbranded as it is; it has no
            // `__Commons` alias.
            let spelled = if n.contains('.') {
                bynk_check::emitted_names::use_qualified_name(n).unwrap_or_else(|| n.clone())
            } else {
                commons_alias(n)
            };
            (n.clone(), spelled)
        })
        .collect()
}

/// #2887: a codec helper of type `n` (`prefix` is `__serialise_` or
/// `__deserialise_`) as an import specifier: for a type this unit renames, the
/// helper the module exports for the real type, bound under the alias's name.
fn codec_import_spec(ctx: &EmitProjectCtx, prefix: &str, n: &str) -> String {
    // #4403: a name a `uses` clause qualifies (`lib.x.Status`) is one dotted key; its
    // helper is the declared type's, bound under an underscore name.
    if n.contains('.') {
        let declared = bynk_check::emitted_names::declared_name_of_key(n);
        let bound = bynk_check::emitted_names::codec_key(n);
        return format!("{prefix}{declared} as {prefix}{bound}");
    }
    match ctx.consumed_type_aliases.get(n) {
        Some(original) => format!("{prefix}{original} as {prefix}{n}"),
        None => format!("{prefix}{n}"),
    }
}
