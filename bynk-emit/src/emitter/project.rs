//! The project-mode module builder (2.5b, #4090): `emit_project` and the helpers the
//! driver, `compose/` and the emitters read (`call_handler_needs_exec`,
//! `walk_unit_table_bodies`, `agent_cross_context_targets`, the agent registry and
//! factory names). `emitter` re-exports the public ones.

use std::path::Path;

use super::boundary::emit_boundary_helpers;
use super::ctx::UnitFacts;
use super::ts_types::asset_ts_type;
use super::{InternalFault, emit, ts_ident};
use super::{
    emit_agent, emit_capability, emit_consts, emit_consumed_type_reexports, emit_context_rebrands,
    emit_cross_context_namespace_imports, emit_derived_capability, emit_free_fn,
    emit_json_codec_helpers, emit_make_surface, emit_messages_bundle, emit_project_imports,
    emit_provider, emit_service, emit_type, external_references_of, extra_import_stmts,
    type_shape_for, used_external_names, write_commons_doc, write_header,
};
use crate::emit_model::EmitProjectCtx;
use bynk_check::checker::CheckedProgram;
use bynk_check::project_model::BuildTarget;
use bynk_check::symbols::UnitTable;
use bynk_model::walk_block_exprs;
use bynk_model::{lower_capability_ops_ir, lower_protocol_ir, lower_service_handler_signature_ir};
use bynk_project::UnitKind;
use bynk_syntax::ast::{CommonsItem, Expr, ExprKind, FnName, MessagesDecl};

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
    // #3927: a template is a function of its fields instead (`templates::emit_embed`).
    for item in &commons.commons.items {
        if let CommonsItem::Embed(e) = item {
            body.push(super::templates::emit_embed(e, commons, ctx));
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

/// The module-level state-registry constant name for an agent class.
pub(super) fn agent_registry_name(agent: &str) -> String {
    format!("__{agent}Registry")
}

/// The exported agent-construction factory name for an agent class.
pub fn agent_factory_name(agent: &str) -> String {
    format!("__make{agent}")
}
