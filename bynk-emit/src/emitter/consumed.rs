//! Boundary helpers for the contexts a module consumes (2.5b, #4090): the helpers
//! `emit_consumed_context_helpers` emits for a consumed context's types, and the
//! renames that hide a type the consumed context does not export
//! (`unexported_type_renames`, `rename_in_decl`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use super::boundary::unbranded_commons_qual;
use super::serialisation;
use super::{qualified_to_ns, ty_to_type_ref};
use crate::emit_model::EmitProjectCtx;
use bynk_check::checker::{CheckedProgram, Ty, TypedCommons};
use bynk_model::walk_block_exprs;
use bynk_project::UnitKind;
use bynk_syntax::ast::{CommonsItem, Expr, TypeBody, TypeDecl};

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
pub(super) fn emit_consumed_context_helpers(
    program: &CheckedProgram,
    ctx: &EmitProjectCtx,
    own_roots: &HashMap<String, Vec<bynk_syntax::ast::TypeRef>>,
    emitted_names: &mut HashSet<String>,
    emitted_insts: &mut HashSet<String>,
) -> (Vec<bynk_ts::TsStmt>, Vec<String>, Vec<String>, Vec<String>) {
    use super::serialisation::{
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
/// caller imports, `boundary::commons_codec_imports`) keeps its name: it is the
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
