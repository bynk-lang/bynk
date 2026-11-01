use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::builtin_names::methods::{OF, UNSAFE};
use crate::checker::{self, CapabilityInfo, CapabilityOpInfo, Ty, TyId, TypedExpr, Types};
use crate::hints::HintSink;
use crate::index::{RefSink, SymbolKind};
use crate::locals::LocalsSink;
use crate::requirements::RequirementSink;
use crate::resolver::{self, ResolvedCommons};
use crate::symbols::{ConsumedType, UnitTable, record_provides_clause_ref, resolve_given_cap_ref};
use bynk_project::detect_provider_dependency_cycles;
use bynk_syntax::ast::*;
use bynk_syntax::error::{Applicability, CompileError};
use bynk_syntax::span::Span;

/// #926: build a checker-facing [`CapabilityOpInfo`] from a capability op's
/// AST, with the op's own type parameters (if any) resolved as [`Ty::Var`]
/// rather than ground types — a call site substitutes a concrete `Ty` for
/// each before checking. Shared by every site that reconstructs
/// `CapabilityInfo` from a `CapabilityDecl` (local capabilities, test/property
/// bodies targeting a context) so the vars-in-scope treatment can't drift.
pub fn build_capability_op_info(
    op: &CapabilityOp,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
) -> CapabilityOpInfo {
    let vars: HashSet<String> = op.type_params.iter().map(|p| p.name.name.clone()).collect();
    CapabilityOpInfo {
        name: op.name.name.clone(),
        type_params: op.type_params.iter().map(|p| p.name.name.clone()).collect(),
        params: op
            .params
            .iter()
            .map(|p| checker::resolve_type_ref_in(&p.type_ref, types, &vars, tys))
            .map(|t| t.unwrap_or_else(|| tys.intern(Ty::Unit)))
            .collect(),
        param_names: op.params.iter().map(|p| p.name.name.clone()).collect(),
        return_ty: checker::resolve_type_ref_in(&op.return_type, types, &vars, tys)
            .unwrap_or_else(|| tys.intern(Ty::Unit)),
    }
}

/// Enforce v0.4 construction rules: types owned by a consumed context can be
/// referenced (held, passed, read for transparent exports) but cannot be
/// constructed. This catches `OtherType { ... }`, `OtherType.of(...)`,
/// `OtherType.unsafe(...)`, and `OtherType.Variant(...)` expressions where
/// `OtherType` is from a consumed context.
pub fn check_context_constraints(
    typed: &checker::TypedCommons,
    consumed_types: &HashMap<String, ConsumedType>,
    local_type_names: &HashSet<String>,
    tys: &Arc<Types>,
) -> Vec<CompileError> {
    let mut errors = Vec::new();
    for item in &typed.commons.items {
        if let CommonsItem::Fn(f) = item {
            walk_block_for_constraints(
                &f.body,
                typed,
                consumed_types,
                local_type_names,
                &mut errors,
                tys,
            );
            // #1700: a contract predicate is an expression like any other.
            for c in f.requires.iter().chain(&f.ensures) {
                walk_expr_for_constraints(
                    &c.predicate,
                    typed,
                    consumed_types,
                    local_type_names,
                    &mut errors,
                    tys,
                );
            }
        }
    }
    errors
}

/// #1700: the same cross-context constraints as [`check_context_constraints`],
/// over the bodies it cannot reach — service and agent handlers, agent
/// invariants and transitions, and provider operations. These are typed by [`check_context_declarations`], after
/// [`check_context_constraints`] runs, so this pass runs after that one: the
/// opaque-`match` rule reads the discriminant's checked type.
pub fn check_handler_constraints(
    typed: &checker::TypedCommons,
    consumed_types: &HashMap<String, ConsumedType>,
    local_type_names: &HashSet<String>,
    tys: &Arc<Types>,
) -> Vec<CompileError> {
    let mut errors = Vec::new();
    for item in &typed.commons.items {
        let (bodies, predicates): (Vec<&Block>, Vec<&Expr>) = match item {
            CommonsItem::Service(s) => (s.handlers.iter().map(|h| &h.body).collect(), Vec::new()),
            // An agent's invariants and transitions are typed alongside its
            // handlers, and are expressions that can name a consumed type.
            CommonsItem::Agent(a) => (
                a.handlers.iter().map(|h| &h.body).collect(),
                a.invariants
                    .iter()
                    .map(|i| &i.predicate)
                    .chain(a.transitions.iter().map(|t| &t.predicate))
                    .collect(),
            ),
            CommonsItem::Provider(p) => (p.ops.iter().map(|op| &op.body).collect(), Vec::new()),
            _ => continue,
        };
        for body in bodies {
            walk_block_for_constraints(
                body,
                typed,
                consumed_types,
                local_type_names,
                &mut errors,
                tys,
            );
        }
        for e in predicates {
            walk_expr_for_constraints(e, typed, consumed_types, local_type_names, &mut errors, tys);
        }
    }
    errors
}

fn walk_block_for_constraints(
    block: &Block,
    typed: &checker::TypedCommons,
    consumed: &HashMap<String, ConsumedType>,
    local: &HashSet<String>,
    errors: &mut Vec<CompileError>,
    tys: &Arc<Types>,
) {
    let mut exprs = Vec::new();
    for stmt in &block.statements {
        statement_exprs(stmt, &mut exprs);
    }
    exprs.push(&block.tail);
    for e in exprs {
        walk_expr_for_constraints(e, typed, consumed, local, errors, tys);
    }
}

/// Recurse an expression for the cross-context construction/inspection
/// constraints, checking each node's own shape then descending through
/// `ast::expr_children` — the exhaustive total child iterator — rather than a
/// hand-matched recursion. A `_ => {}` below only opts a variant out of *this
/// function's own* business-rule check; the recursion beneath it is
/// unconditional and can't be silently skipped by a future `ExprKind` variant
/// the way the equivalent hand-rolled match could.
///
/// `local` threads through unread — pre-existing (`check_context_constraints`'s
/// `local_type_names` is part of the broader `ResolvedCommons`/local-type-name
/// handling the review flags separately at #57), not introduced by this pass.
/// Collapsing the old mutual block/expr recursion into one self-recursive
/// function made clippy's `only_used_in_recursion` newly able to see it.
#[allow(clippy::only_used_in_recursion)]
fn walk_expr_for_constraints(
    e: &Expr,
    typed: &checker::TypedCommons,
    consumed: &HashMap<String, ConsumedType>,
    local: &HashSet<String>,
    errors: &mut Vec<CompileError>,
    tys: &Arc<Types>,
) {
    match &e.kind {
        // A transparent export shares its structure with consumers, including
        // field-level construction (type-system §6.5); only an opaque one's
        // record form is the owner's alone. #1700: this used to reject any
        // consumed type, which free functions rarely hit but handlers do —
        // building `bynk`'s `Request`, or an adapter's boundary record, is how
        // those capabilities are called.
        ExprKind::RecordConstruction { type_name, .. } => {
            if let Some(ct) = consumed.get(&type_name.name)
                && ct.visibility == Visibility::Opaque
            {
                errors.push(
                    CompileError::new(
                        "bynk.context.external_construction",
                        type_name.span,
                        format!(
                            "cannot construct `{}` here — it is owned by context `{}`",
                            type_name.name, ct.owning_context,
                        ),
                    )
                    .with_note(
                        "values of an externally-owned type can only be created inside the owning context",
                    ),
                );
            }
        }
        // `T.method(...)` written as MethodCall with receiver Ident(T).
        ExprKind::MethodCall {
            receiver, method, ..
        } => {
            if let ExprKind::Ident(id) = &receiver.kind
                && let Some(ct) = consumed.get(&id.name)
            {
                let is_construct = method.name == OF
                    || method.name == UNSAFE
                    || matches!(
                        typed.types.get(&id.name).map(|d| &d.body),
                        Some(TypeBody::Sum(s)) if s.variants.iter().any(|v| v.name.name == method.name),
                    );
                if is_construct {
                    errors.push(
                        CompileError::new(
                            "bynk.context.external_construction",
                            id.span.merge(method.span),
                            format!(
                                "cannot construct `{}.{}` here — `{}` is owned by context `{}`",
                                id.name, method.name, id.name, ct.owning_context,
                            ),
                        )
                        .with_note(
                            "values of an externally-owned type can only be created inside the owning context",
                        ),
                    );
                }
            }
        }
        // `T.X` on an opaquely-exported sum's type name names a variant, and
        // a field read on a value of an opaquely-exported type reads its
        // fields: both inspect the shape the export hides (#1948).
        ExprKind::FieldAccess { receiver, field } => {
            if let ExprKind::Ident(id) = &receiver.kind
                && let Some(ct) = consumed.get(&id.name)
                && ct.visibility == Visibility::Opaque
                && typed
                    .types
                    .get(&id.name)
                    .map(|d| matches!(d.body, TypeBody::Sum(_)))
                    .unwrap_or(false)
            {
                errors.push(
                    CompileError::new(
                        "bynk.context.opaque_inspection",
                        id.span.merge(field.span),
                        format!(
                            "cannot inspect opaquely-exported type `{}` from outside context `{}`",
                            id.name, ct.owning_context,
                        ),
                    )
                    .with_note(
                        "opaque exports hide the type's shape; the owning context did not expose variants or fields",
                    ),
                );
            } else if let Some((name, ct)) = opaque_export_of(receiver, typed, consumed) {
                errors.push(
                    CompileError::new(
                        "bynk.context.opaque_inspection",
                        field.span,
                        format!(
                            "cannot read field `{}` of opaquely-exported type `{name}` from outside context `{}`",
                            field.name, ct.owning_context,
                        ),
                    )
                    .with_note(
                        "opaque exports hide the type's shape; a consumer can hold and pass the value, and call its methods, but not read its fields",
                    ),
                );
            }
        }
        // If the discriminant is typed as an opaquely-exported consumed
        // type, the match is forbidden because we can't reveal the variants.
        ExprKind::Match { discriminant, arms } => {
            let mut discriminant_reported = false;
            // #2712: a `let`/lambda-parameter record pattern is a `match` on a
            // hidden binder the author never wrote, so its opaque type is
            // reported where they wrote it — at the pattern, below.
            let hidden_record = matches!(&discriminant.kind, ExprKind::Ident(i) if bynk_syntax::ast::is_generated_binding(&i.name))
                && matches!(
                    arms.first().map(|a| &a.pattern),
                    Some(Pattern::Record { .. })
                );
            if !hidden_record && let Some(ty) = typed.expr_ty(discriminant.id).as_deref() {
                let display = ty.display(tys);
                if let Some(ct) = consumed.get(&display)
                    && ct.visibility == Visibility::Opaque
                {
                    discriminant_reported = true;
                    errors.push(
                        CompileError::new(
                            "bynk.context.opaque_inspection",
                            discriminant.span,
                            format!(
                                "cannot `match` on opaquely-exported type `{}` from outside context `{}`",
                                display, ct.owning_context,
                            ),
                        )
                        .with_note(
                            "opaque exports hide the type's shape; the owning context did not expose variants",
                        ),
                    );
                }
            }
            // #2641: a record pattern reads fields exactly as `.field` does.
            // The discriminant report above already covers a `match` on the
            // opaque value itself.
            for arm in arms {
                opaque_record_patterns(&arm.pattern, consumed, !discriminant_reported, errors);
            }
        }
        ExprKind::Is { pattern, .. } => opaque_record_patterns(pattern, consumed, true, errors),
        _ => {}
    }
    for child in expr_children(e) {
        walk_expr_for_constraints(child, typed, consumed, local, errors, tys);
    }
}

/// #2641: every record pattern in `pattern` that names an opaquely-exported
/// consumed type reads the fields the export hides, as a field read does.
/// `top` is false when the pattern's own scrutinee was already reported.
fn opaque_record_patterns(
    pattern: &Pattern,
    consumed: &HashMap<String, ConsumedType>,
    top: bool,
    errors: &mut Vec<CompileError>,
) {
    match pattern {
        Pattern::Record {
            type_name, fields, ..
        } => {
            if top
                && let Some(ct) = consumed.get(&type_name.name)
                && ct.visibility == Visibility::Opaque
            {
                errors.push(
                    CompileError::new(
                        "bynk.context.opaque_inspection",
                        pattern.span(),
                        format!(
                            "cannot match fields of opaquely-exported type `{}` from outside context `{}`",
                            type_name.name, ct.owning_context,
                        ),
                    )
                    .with_note(
                        "opaque exports hide the type's shape; a consumer can hold and pass the value, and call its methods, but not read its fields",
                    ),
                );
            }
            for (_, sub) in fields {
                if let Some(sub) = sub {
                    opaque_record_patterns(sub, consumed, true, errors);
                }
            }
        }
        Pattern::List { elems, .. } => {
            for e in elems {
                opaque_record_patterns(e, consumed, true, errors);
            }
        }
        Pattern::Variant { bindings, .. } => {
            for b in bindings {
                opaque_record_patterns(b.pattern(), consumed, true, errors);
            }
        }
        Pattern::Or(alts, _) => {
            for a in alts {
                opaque_record_patterns(a, consumed, top, errors);
            }
        }
        Pattern::Refined { inner, .. } => opaque_record_patterns(inner, consumed, top, errors),
        Pattern::Wildcard(_)
        | Pattern::Binding(_)
        | Pattern::Literal { .. }
        | Pattern::Range(_) => {}
    }
}

/// #1948: the opaquely-exported consumed type `e` is a value of, if any,
/// with its export's metadata. Reads the type `e` was checked at, so a
/// value bound from a call or a parameter is caught however it is spelled.
fn opaque_export_of<'c>(
    e: &Expr,
    typed: &checker::TypedCommons,
    consumed: &'c HashMap<String, ConsumedType>,
) -> Option<(String, &'c ConsumedType)> {
    let ty = typed.expr_ty(e.id)?;
    let Ty::Named { name, .. } = &*ty else {
        return None;
    };
    let ct = consumed.get(name)?;
    (ct.visibility == Visibility::Opaque).then(|| (name.clone(), ct))
}

/// Check capability/provider/service/agent declaration bodies for a context (or
/// adapter) unit. Mutates `typed` to extend the expr_types map with bindings
/// observed in the new bodies.
///
/// The parent builds the shared state read by every per-kind validator — a
/// `resolved` commons snapshot and the `capability_info_map` (local capability
/// signatures, extended with the cross-context flattened caps) — then runs the
/// per-declaration-kind validators in a fixed order. The order is load-bearing:
/// multi-error fixtures assert the diagnostic sequence
/// (capabilities → providers → services → agents).
#[allow(clippy::too_many_arguments)]
pub fn check_context_declarations(
    typed: &mut checker::TypedCommons,
    table: &UnitTable,
    cross_context: &resolver::CrossContextInfo,
    is_context: bool,
    uses_commons_type_names: &HashSet<String>,
    // Events slice 3a (#972): this unit's own local + direct-`uses` types —
    // deliberately narrower than `typed.types` (local + uses + *consumes*).
    // A field default is validated against this table because it's the same
    // one a **subscriber** regenerating this event's codec cross-context
    // will see (`emit_consumed_context_helpers`'s `combined_types_for`,
    // #973) — a default reachable only through this unit's own `consumes`
    // would pass here-with-the-wider-table and then silently fail to
    // construct in a subscriber's module, with no diagnostic at emit time.
    subscriber_visible_types: &HashMap<String, Arc<TypeDecl>>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    requirements: &mut RequirementSink,
    tys: &Arc<Types>,
) -> Vec<CompileError> {
    let mut errors = Vec::new();
    let no_vars: HashSet<String> = HashSet::new();

    // Build a resolved-commons snapshot for the per-handler checker.
    // We synthesise a ResolvedCommons by reusing typed.types / typed.fns /
    // typed.methods; the resolver wouldn't add anything new. `ResolvedCommons::new`
    // derives `local_type_names`/`event_type_names` from `table` — the
    // *pre-merge* local table — rather than `typed.types` (already
    // local+uses+consumes merged); see its doc comment for why that
    // distinction matters (owner-only emission, spine #936).
    let resolved = ResolvedCommons::new(
        typed.commons.clone(),
        typed.types.clone(),
        &table.types,
        typed.fns.clone(),
        typed.methods.clone(),
        table.agents.clone(),
        &table.events,
        cross_context.clone(),
        HashMap::new(),
        is_context,
        uses_commons_type_names.clone(),
    )
    // #1883, slice 1a: a handler body sees the unit's assets too.
    .with_assets(typed.assets.clone())
    .with_embeds(typed.embeds.clone())
    .with_consts(typed.consts.clone())
    .with_shadowed_imports(typed.shadowed_imports.clone())
    .with_extensible_enums(typed.extensible_enums.clone())
    .with_stability(typed.stability.clone());

    // v0.25: capability operation signatures reference types.
    check_capability_decls(table, &typed.types, &no_vars, refs);
    // S4a (#1966): a derived capability's base (`capability Sessions = Kv`)
    // is a reference to the base, in the unit that declares it, recorded as a
    // bare `given Kv` is.
    for item in &typed.commons.items {
        if let CommonsItem::Capability(c) = item
            && let Some(d) = &c.derived
        {
            crate::symbols::record_capability_clause_ref(&d.base, cross_context, refs);
        }
    }

    // Capability info from the table.
    let mut capability_info_map: HashMap<String, CapabilityInfo> = table
        .capabilities
        .iter()
        .map(|(name, decl)| {
            let ops = decl
                .ops
                .iter()
                .map(|op| build_capability_op_info(op, &typed.types, tys))
                .collect();
            (
                name.clone(),
                CapabilityInfo {
                    name: name.clone(),
                    ops,
                    derived_from: decl.derived.as_ref().and_then(|d| {
                        cross_context
                            .flattened_caps
                            .get(&d.base.name)
                            .map(|unit| (unit.clone(), d.base.name.clone()))
                    }),
                },
            )
        })
        .collect();
    // v0.17: flattened capabilities (`consumes U { Cap }`) enter the local map
    // under their bare names, resolved from the consumed unit's exported
    // capability so bare `given Cap` / `Cap.op(…)` type-check as if local.
    for (cap, unit) in &cross_context.flattened_caps {
        let Some(xcap) = cross_context
            .consumed_capabilities
            .get(unit)
            .and_then(|m| m.get(cap))
        else {
            continue;
        };
        let ops = xcap
            .ops
            .iter()
            .map(|op| {
                let vars: HashSet<String> = op.type_params.iter().cloned().collect();
                CapabilityOpInfo {
                    name: op.name.clone(),
                    type_params: op.type_params.clone(),
                    params: op
                        .params
                        .iter()
                        .map(|(_, tr)| {
                            checker::resolve_type_ref_in(tr, &typed.types, &vars, tys)
                                .unwrap_or_else(|| tys.intern(Ty::Unit))
                        })
                        .collect(),
                    param_names: op.params.iter().map(|(n, _)| n.clone()).collect(),
                    return_ty: checker::resolve_type_ref_in(
                        &op.return_type,
                        &typed.types,
                        &vars,
                        tys,
                    )
                    .unwrap_or_else(|| tys.intern(Ty::Unit)),
                }
            })
            .collect();
        capability_info_map.insert(
            cap.clone(),
            CapabilityInfo {
                name: cap.clone(),
                ops,
                derived_from: None,
            },
        );
    }

    check_provider_decls(
        typed,
        table,
        cross_context,
        &resolved,
        &capability_info_map,
        refs,
        hints,
        locals,
        requirements,
        &mut errors,
        tys,
    );
    check_service_decls(
        typed,
        table,
        cross_context,
        &resolved,
        &capability_info_map,
        refs,
        hints,
        locals,
        requirements,
        &mut errors,
        tys,
    );
    check_agent_decls(
        typed,
        table,
        cross_context,
        is_context,
        uses_commons_type_names,
        &capability_info_map,
        &no_vars,
        refs,
        hints,
        locals,
        requirements,
        &mut errors,
        tys,
    );
    // After the agent bodies, so their own agent calls are classified too.
    check_provider_agent_calls(table, &typed.callees, &mut errors);
    // #2469: every body's requirements and agent calls are now classified, so
    // the capability paths can be drawn from the same facts.
    let paths = crate::capability_paths::CapabilityGraph::build(
        table,
        &typed.callees,
        requirements.current(),
        typed.commons.span.file,
    );
    requirements.record_paths(paths);

    check_event_field_defaults(
        table,
        &resolved,
        subscriber_visible_types,
        &mut typed.expr_types,
        &mut typed.callees,
        refs,
        hints,
        locals,
        &mut errors,
        tys,
    );

    check_event_annotations(table, &mut errors);

    // State-migrations S3 (#2248): `replaces` / `removed agent` on their own
    // terms; what needs the schema lock is the reconcile's.
    check_class_declarations(table, &mut errors);

    errors
}

/// State-migrations S3 (#2248, Decision B): the source-only rules for agent
/// class renames and removals. The old name of a `replaces` or `removed agent`
/// must not still be declared as an agent, and each old name has one fate — a
/// single rename, or a removal.
fn check_class_declarations(table: &UnitTable, errors: &mut Vec<CompileError>) {
    let mut fates: HashMap<&str, Span> = HashMap::new();
    let mut agents: Vec<&AgentDecl> = table.agents.values().collect();
    agents.sort_by_key(|a| a.span.start);
    for a in agents {
        let Some(old) = &a.replaces else { continue };
        if table.agents.contains_key(&old.name) {
            errors.push(
                CompileError::new(
                    "bynk.agent.replaces_live",
                    old.span,
                    format!(
                        "agent `{}` replaces `{}`, but `{}` is still declared",
                        a.name.name, old.name, old.name
                    ),
                )
                .with_note("a rename moves the old class's data to the new one; remove the old declaration"),
            );
        }
        if let Some(prev) = fates.insert(&old.name, old.span) {
            errors.push(
                CompileError::new(
                    "bynk.agent.class_conflict",
                    old.span,
                    format!("agent class `{}` is renamed more than once", old.name),
                )
                .with_label(prev, "first renamed here"),
            );
        }
    }
    for r in &table.removed_agents {
        if table.agents.contains_key(&r.name.name) {
            errors.push(
                CompileError::new(
                    "bynk.agent.removed_live",
                    r.name.span,
                    format!(
                        "`removed agent {}` names a class that is still declared",
                        r.name.name
                    ),
                )
                .with_note(
                    "removing a class deletes its stored data; remove the declaration first",
                ),
            );
        }
        if let Some(prev) = fates.insert(&r.name.name, r.name.span) {
            errors.push(
                CompileError::new(
                    "bynk.agent.class_conflict",
                    r.name.span,
                    format!(
                        "agent class `{}` is both renamed and removed, or removed twice",
                        r.name.name
                    ),
                )
                .with_label(prev, "its other fate is declared here"),
            );
        }
    }
}

/// Events slice 3a (#972): validate every `event`'s field default (`field: T
/// = expr`), if it has one. Two gates, both required before emission ever
/// sees it:
///
/// 1. **Static/pure/typed** — `checker::check_event_field_default`, the same
///    empty-pure-scope discipline agent `store` field defaults already have
///    (`bynk.agents.bad_state_initialiser`'s sibling), pushing
///    `bynk.types.bad_field_default` on failure.
/// 2. **Constructible** — `crate::wire_default::lower_field_default_wire`
///    against `subscriber_visible_types`, the *narrower* table a subscriber
///    regenerating this event's codec cross-context will actually see. This
///    is what keeps emission's own `.ok()` fallback (`emit_record`)
///    unreachable in practice: anything this same function can't build is
///    rejected here, with a diagnostic, before it ever reaches emission.
///
/// Only gate 2 runs when gate 1 already found a problem — a value that
/// isn't even a valid static value of the right type has nothing useful to
/// say about wire-constructibility, and would just be a confusing second
/// error for the same field.
#[allow(clippy::too_many_arguments)]
fn check_event_field_defaults(
    table: &UnitTable,
    resolved: &ResolvedCommons,
    subscriber_visible_types: &HashMap<String, Arc<TypeDecl>>,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    callees: &mut HashMap<ExprId, checker::Callee>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    errors: &mut Vec<CompileError>,
    tys: &Arc<Types>,
) {
    for event in table.events.values() {
        for field in &event.body.fields {
            let Some(init) = &field.init else {
                continue;
            };
            let before = errors.len();
            checker::check_event_field_default(
                init,
                &field.type_ref,
                "event field default",
                resolved,
                tys,
                expr_types,
                callees,
                errors,
                refs,
                hints,
                locals,
            );
            if errors.len() > before {
                continue;
            }
            if let Err(reason) = crate::wire_default::lower_field_default_wire(
                init,
                &field.type_ref,
                subscriber_visible_types,
            ) {
                errors.push(
                    CompileError::new(
                        "bynk.types.bad_field_default",
                        init.span,
                        format!(
                            "event field `{}`'s default cannot be represented on the wire: {reason}",
                            field.name.name
                        ),
                    )
                    .with_note(
                        "a default is spliced into the same codec a real wire value passes \
                         through, so it must be buildable with no reference to any type's \
                         generated value namespace — only literals, sum-variant tags, and record \
                         literals qualify",
                    ),
                );
            }
        }
    }
}

/// Events slice 3b (#978): validate every event's `@`-annotations against
/// the closed one-name registry. `@schema` is the only legal name; its sole
/// argument must be a positive `Int` literal, positional (not labelled), and
/// it may appear at most once per event. `EventDecl::schema_version` reads
/// the same annotations permissively (falling back to `1` on anything that
/// doesn't fit) — this is what keeps that fallback unreachable for anything
/// but an already-reported error.
fn check_event_annotations(table: &UnitTable, errors: &mut Vec<CompileError>) {
    for event in table.events.values() {
        let mut schema_count = 0usize;
        for ann in &event.annotations {
            if ann.name.name != "schema" {
                errors.push(
                    CompileError::new(
                        "bynk.event.unknown_annotation",
                        ann.name.span,
                        format!(
                            "unknown event annotation `@{}` — expected `@schema`",
                            ann.name.name
                        ),
                    )
                    .with_note("event annotations are a closed set"),
                );
                continue;
            }
            schema_count += 1;
            if schema_count > 1 {
                errors.push(
                    CompileError::new(
                        "bynk.event.bad_schema_version",
                        ann.span,
                        "`@schema` may appear at most once on an event",
                    )
                    .with_note("the event's schema version is a single value, not a set"),
                );
                continue;
            }
            match ann.args.as_slice() {
                [arg] if arg.label.is_none() => {
                    if !matches!(&arg.value.kind, ExprKind::IntLit { value, .. } if *value > 0) {
                        errors.push(CompileError::new(
                            "bynk.event.bad_schema_version",
                            arg.span,
                            "`@schema`'s argument must be a positive `Int` literal",
                        ));
                    }
                }
                [arg] => {
                    errors.push(CompileError::new(
                        "bynk.event.bad_schema_version",
                        arg.span,
                        "`@schema` takes one positional argument, not a labelled one",
                    ));
                }
                [] => {
                    errors.push(
                        CompileError::new(
                            "bynk.event.bad_schema_version",
                            ann.span,
                            "`@schema` requires one argument — the schema version",
                        )
                        .with_note("write `@schema(2)`, for example"),
                    );
                }
                _ => {
                    errors.push(CompileError::new(
                        "bynk.event.bad_schema_version",
                        ann.span,
                        "`@schema` takes exactly one argument",
                    ));
                }
            }
        }
    }
}

/// v0.25: capability operation signatures reference types; record them under
/// the capability as owner (the table is unit-level — the owner re-attributes
/// spans to the declaring file at assembly).
fn check_capability_decls(
    table: &UnitTable,
    types: &HashMap<String, Arc<TypeDecl>>,
    no_vars: &HashSet<String>,
    refs: &mut RefSink,
) {
    for (name, decl) in &table.capabilities {
        refs.set_owner(name);
        for op in &decl.ops {
            // #926: an op's own type parameters shadow a same-named real type
            // (mirroring every other `skip`-set use here) — a bare `T` should
            // never index-reference an unrelated declared type `T`.
            let vars: HashSet<String> = if op.type_params.is_empty() {
                no_vars.clone()
            } else {
                op.type_params.iter().map(|p| p.name.name.clone()).collect()
            };
            for p in &op.params {
                checker::record_type_refs(&p.type_ref, types, &vars, refs);
            }
            checker::record_type_refs(&op.return_type, types, &vars, refs);
        }
    }
    refs.clear_owner();
}

/// Check provider bodies. v0.12: a provider may declare `given` and use
/// those capabilities in its bodies (provider composition). Bodies are
/// effectful if the operation returns Effect[T]; no `self`. Also detects
/// provider dependency cycles over capabilities.
#[allow(clippy::too_many_arguments)]
fn check_provider_decls(
    typed: &mut checker::TypedCommons,
    table: &UnitTable,
    cross_context: &resolver::CrossContextInfo,
    resolved: &ResolvedCommons,
    capability_info_map: &HashMap<String, CapabilityInfo>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    requirements: &mut RequirementSink,
    errors: &mut Vec<CompileError>,
    tys: &Arc<Types>,
) {
    for provider in table.providers.values() {
        refs.set_owner(&provider.provider_name.name);
        // v0.25: `provides Cap = …` references the capability.
        // v0.35 (ADR 0068): and records a capability→provider implementation edge.
        if table.capabilities.contains_key(&provider.capability.name)
            || cross_context
                .flattened_caps
                .contains_key(&provider.capability.name)
        {
            record_provides_clause_ref(&provider.capability, cross_context, refs);
        }
        // Build the provider's capability scope from its `given`, validating
        // each name is a declared capability.
        let mut provider_caps: HashMap<String, CapabilityInfo> = HashMap::new();
        for cap_ref in &provider.given {
            if let Some(info) =
                resolve_given_cap_ref(cap_ref, capability_info_map, cross_context, errors, refs)
            {
                provider_caps.insert(cap_ref.key().to_string(), info);
            }
        }
        for op in &provider.ops {
            // The provider's `given` keys are in scope (so cross-context
            // capability calls resolve), but unused-`given` is not reported
            // per-op: a capability may be used in one op but not another.
            // No `given_anchor`: the clause lives on the `provides` line,
            // not at the op's return type, so an absent clause is not
            // synthesised here (v0.26).
            checker::check_handler_body(
                resolved,
                checker::HandlerBodyCheck {
                    capabilities: provider_caps.clone(),
                    declared_capabilities: capability_info_map.clone(),
                    ..checker::HandlerBodyCheck::new(
                        &op.body,
                        &op.return_type,
                        &op.params,
                        &provider.given,
                    )
                },
                checker::CheckSinks {
                    tys,
                    expr_types: &mut typed.expr_types,
                    errors,
                    refs,
                    hints,
                    locals,
                    requirements,
                    callees: &mut typed.callees,
                },
            );
        }
    }

    // v0.12: providers form a dependency graph over capabilities (a provider's
    // `given` are the capabilities its provided capability depends on). Reject
    // a cycle — the composition root cannot instantiate one in dependency
    // order. Self-provision (`provides X = … given X`) is the trivial cycle.
    detect_provider_dependency_cycles(&table.providers, errors);
}

/// The agent handlers `body` calls, as `(agent, handler)` — read from the
/// checker's `Callee::Agent` classification, so an inline `Agent(key).m()` and
/// a let-bound `a.m()` count alike.
pub(crate) fn agent_calls_in(
    body: &Block,
    callees: &HashMap<ExprId, checker::Callee>,
) -> Vec<(String, String, Span)> {
    fn walk(
        e: &Expr,
        callees: &HashMap<ExprId, checker::Callee>,
        out: &mut Vec<(String, String, Span)>,
    ) {
        if let ExprKind::MethodCall { method, .. } = &e.kind
            && let Some(checker::Callee::Agent { agent, handler }) = callees.get(&e.id)
        {
            out.push((agent.clone(), handler.clone(), method.span));
        }
        for c in expr_children(e) {
            walk(c, callees, out);
        }
    }
    let mut top = Vec::new();
    for s in &body.statements {
        statement_exprs(s, &mut top);
    }
    top.push(&body.tail);
    let mut out = Vec::new();
    for e in top {
        walk(e, callees, &mut out);
    }
    out
}

/// A capability an agent call needs: the `given` entry that asks for it, and
/// the agent handler whose `given` it is.
struct AgentNeed<'t> {
    cap: &'t CapRef,
    agent: String,
    handler: String,
}

/// Every capability calling `agent.handler` needs: the handler's own `given`,
/// and — since a handler's deps are widened to cover the agent handlers it
/// calls (#527) — theirs, transitively. The first handler to ask for a key
/// is the one recorded.
fn agent_handler_needs<'t>(
    table: &'t UnitTable,
    callees: &HashMap<ExprId, checker::Callee>,
    agent: &str,
    handler: &str,
    seen: &mut HashSet<(String, String)>,
    out: &mut Vec<AgentNeed<'t>>,
) {
    if !seen.insert((agent.to_string(), handler.to_string())) {
        return;
    }
    let Some(h) = table.agents.get(agent).and_then(|a| {
        a.handlers
            .iter()
            .find(|h| h.method_name.as_ref().is_some_and(|n| n.name == handler))
    }) else {
        return;
    };
    for c in &h.given {
        if !out.iter().any(|n| n.cap.key() == c.key()) {
            out.push(AgentNeed {
                cap: c,
                agent: agent.to_string(),
                handler: handler.to_string(),
            });
        }
    }
    for (a, m, _) in agent_calls_in(&h.body, callees) {
        agent_handler_needs(table, callees, &a, &m, seen, out);
    }
}

/// A provider op that calls an agent handler hands it the provider's own deps,
/// and a provider is constructed with exactly its declared `given` — unlike a
/// service or agent handler, whose deps are widened to cover the agent
/// handlers it calls (#527). So every capability the agent handler needs must
/// be in the provider's `given`; otherwise the emitted call fails `tsc`.
fn check_provider_agent_calls(
    table: &UnitTable,
    callees: &HashMap<ExprId, checker::Callee>,
    errors: &mut Vec<CompileError>,
) {
    for provider in table.providers.values() {
        let entries: Vec<(String, Span)> = provider
            .given
            .iter()
            .map(|c| (c.key().to_string(), c.span))
            .collect();
        for op in &provider.ops {
            for (agent, handler, span) in agent_calls_in(&op.body, callees) {
                let mut needs = Vec::new();
                agent_handler_needs(
                    table,
                    callees,
                    &agent,
                    &handler,
                    &mut HashSet::new(),
                    &mut needs,
                );
                for need in needs {
                    let key = need.cap.key();
                    if entries.iter().any(|(k, _)| k == key) {
                        continue;
                    }
                    // As the agent handler's `given` spells it: a cross-context
                    // entry keeps its prefix.
                    let written = match need.cap.prefix() {
                        Some(prefix) => format!("{prefix}.{key}"),
                        None => key.to_string(),
                    };
                    let mut err = CompileError::new(
                        "bynk.provider.agent_call_undeclared_capability",
                        span,
                        format!(
                            "agent handler `{agent}.{handler}` needs `{written}`, but provider `{}` does not list it in `given`",
                            provider.provider_name.name
                        ),
                    );
                    if (need.agent.as_str(), need.handler.as_str())
                        != (agent.as_str(), handler.as_str())
                    {
                        err = err.with_note(format!(
                            "`{agent}.{handler}` calls `{}.{}`, which is `given {written}`",
                            need.agent, need.handler
                        ));
                    }
                    err = err.with_note(format!(
                        "a provider hands an agent handler only the capabilities in its own `given`; add `{written}` to the provider's `given` clause"
                    ));
                    // The clause lives on the `provides` line, so the fix is
                    // offered only when it already has entries (v0.26). Not for
                    // a cross-context entry: its prefix may be an alias that only
                    // the agent's own file declares.
                    if let Some((_, last)) = entries.last()
                        && !need.cap.is_cross_context()
                    {
                        err = err.with_suggestion(
                            format!("add `{written}` to the `given` clause"),
                            vec![(Span::new(last.end, last.end), format!(", {written}"))],
                            Applicability::MachineApplicable,
                        );
                    }
                    errors.push(err);
                }
            }
        }
    }
}

/// Check service handlers across all services in this context: HTTP/cron/queue
/// handler shape and per-kind duplicate detection (route/schedule/consumer),
/// then each handler's `given` clause and body. The duplicate-detection passes
/// run before the body pass so the `bynk.<kind>.duplicate_*` diagnostics
/// precede the body diagnostics in multi-error fixtures.
/// v0.44: a service is one protocol adapter — every handler's form must match
/// the `from <protocol>` header. A `from`-less service (`Call`) admits only
/// `on call`; mismatches are `bynk.service.{missing_from,mixed_protocols}`.
/// `visible_types` is the merged type map (this unit's declarations plus its
/// `uses`/`consumes` targets, `typed.types`); `table.types` holds only this
/// unit's own, so a check that fails closed on an unknown name must use the
/// former, as the HTTP path-param rule does.
fn check_service_protocols(
    table: &UnitTable,
    visible_types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
    tys: &Arc<Types>,
) {
    // v0.104 (slice 3b, D5): at v1 the Workers upgrade routes by the `Upgrade:
    // websocket` header alone (no path/query discriminator), so a context may hold
    // at most one `from websocket` service. Report every WS service past the first
    // (name-sorted for a deterministic diagnostic).
    let mut ws_services: Vec<&ServiceDecl> = table
        .services
        .values()
        .filter(|s| matches!(s.protocol, ServiceProtocol::WebSocket { .. }))
        .collect();
    ws_services.sort_by(|a, b| a.name.name.cmp(&b.name.name));
    for extra in ws_services.iter().skip(1) {
        errors.push(
            CompileError::new(
                "bynk.service.websocket_multiple",
                extra.name.span,
                format!(
                    "this context holds more than one `from websocket` service (`{}`) — at v1 the upgrade routes by the `Upgrade: websocket` header alone, so a context may host only one",
                    extra.name.name
                ),
            )
            .with_note("split the WebSocket services into separate contexts; per-path routing of multiple WebSocket endpoints is a named follow-on"),
        );
    }
    for service in table.services.values() {
        // v0.103: a `from websocket` service holds exactly one `on open` handler
        // (the edge upgrade); inbound frames are the agent's typed messages, not
        // service handlers.
        if matches!(service.protocol, ServiceProtocol::WebSocket { .. }) {
            let opens: Vec<&Handler> = service
                .handlers
                .iter()
                .filter(|h| matches!(h.kind, HandlerKind::Open))
                .collect();
            if opens.is_empty() {
                errors.push(
                    CompileError::new(
                        "bynk.service.websocket_open_arity",
                        service.name.span,
                        format!(
                            "the `from websocket` service `{}` has no `on open` handler — it needs exactly one (the edge upgrade)",
                            service.name.name
                        ),
                    )
                    .with_note("a `from websocket` service holds exactly one `on open`, and optionally one `on message` (inbound) and one `on close`"),
                );
            } else if opens.len() > 1 {
                errors.push(CompileError::new(
                    "bynk.service.websocket_open_arity",
                    opens[1].span,
                    format!(
                        "the `from websocket` service `{}` has more than one `on open` handler — it needs exactly one",
                        service.name.name
                    ),
                ));
            }
            // #1657 (runtime-semantics track §3.4): an `on open` parameter
            // arrives as a query-string value (`url.searchParams.get`), which the
            // upgrade passes on unparsed. A `room: Int` was a string cast `as
            // number`, so `room + 1` gave `"51"` and `?room=05` reached a
            // different agent than `Room(5)`. Like an HTTP path parameter
            // (`bynk.http.path_param_not_stringy`), it must be constructible from
            // `String`. `on message`/`on close` route values are a prefix of
            // these, so they inherit the rule.
            for open in &opens {
                for p in &open.params {
                    // The merged map: an opaque `String` imported through `uses`
                    // is constructible too (review of #1693).
                    if !is_string_constructible(&p.type_ref, visible_types) {
                        errors.push(
                            CompileError::new(
                                "bynk.service.websocket_param_not_stringy",
                                p.type_ref.span(),
                                format!(
                                    "the `on open` parameter `{}` must have a type constructible from `String` (got `{}`)",
                                    p.name.name,
                                    ts_type_ref_display(&p.type_ref),
                                ),
                            )
                            .with_note(
                                "it arrives as a query-string value; use `String`, a refined `String`, or an opaque type whose base is `String`, and parse an `Int` with `Int.parse` in the body",
                            ),
                        );
                    }
                }
            }
            // v0.106 (slice 3b-iii): the inbound `on message` and `on close` are
            // optional but at most one each; an `on message` carries the decoded
            // inbound frame as the single param typed as the service's `in` type.
            let ServiceProtocol::WebSocket { in_type, .. } = &service.protocol else {
                unreachable!("guarded by the enclosing match");
            };
            // Resolved-`Ty` equality, not surface-syntax comparison — the
            // param/route matching below must not silently treat two
            // differently-spelled-but-equal types as a mismatch, nor two
            // distinct types `type_refs_match`'s `_ => false` fallback
            // couldn't classify (List/Map/Query/…) as matching.
            let resolve_ty = |t: &TypeRef| {
                checker::resolve_type_ref_in(t, &table.types, &HashSet::new(), tys)
                    .unwrap_or(tys.intern(Ty::Unit))
            };
            let messages: Vec<&Handler> = service
                .handlers
                .iter()
                .filter(|h| matches!(h.kind, HandlerKind::Message))
                .collect();
            let closes: Vec<&Handler> = service
                .handlers
                .iter()
                .filter(|h| matches!(h.kind, HandlerKind::Close))
                .collect();
            if messages.len() > 1 {
                errors.push(CompileError::new(
                    "bynk.service.websocket_open_arity",
                    messages[1].span,
                    format!(
                        "the `from websocket` service `{}` has more than one `on message` handler — it needs at most one",
                        service.name.name
                    ),
                ));
            }
            if closes.len() > 1 {
                errors.push(CompileError::new(
                    "bynk.service.websocket_open_arity",
                    closes[1].span,
                    format!(
                        "the `from websocket` service `{}` has more than one `on close` handler — it needs at most one",
                        service.name.name
                    ),
                ));
            }
            for message in &messages {
                let frame_params = message
                    .params
                    .iter()
                    .filter(|p| resolve_ty(&p.type_ref) == resolve_ty(in_type))
                    .count();
                if frame_params != 1 {
                    errors.push(
                        CompileError::new(
                            "bynk.ws.message_frame_param",
                            message.span,
                            format!(
                                "a WebSocket `on message` handler must have exactly one parameter of the service's inbound frame type `{}` (the decoded frame), but found {frame_params}",
                                ts_type_ref_display(in_type)
                            ),
                        )
                        .with_note(
                            "declare the frame as a parameter, e.g. `on message by user: Actor (frame: ClientFrame)`; any other parameters are route values recovered from the connection",
                        ),
                    );
                }
            }
            // v0.106 (slice 3b-iii): an `on message`/`on close` recovers its
            // non-frame (route) parameters **positionally** from the socket
            // attachment the `on open` accept wrote — so they must be a
            // type-compatible prefix of the `on open` parameters. A mismatch would
            // silently `as`-cast one route value to another's type at the dispatch.
            if let [open] = opens.as_slice() {
                let op = &open.params;
                let route_mismatch = |p: &Param, errors: &mut Vec<CompileError>| {
                    errors.push(
                        CompileError::new(
                            "bynk.ws.route_param_mismatch",
                            p.span,
                            format!(
                                "the route parameter `{}: {}` does not match the `on open` parameter at this position — `on message`/`on close` route values are recovered positionally from the connection, so they must be a type-compatible prefix of the `on open` parameters",
                                p.name.name,
                                ts_type_ref_display(&p.type_ref)
                            ),
                        )
                        .with_note(
                            "give the inbound/close handler the same leading parameters (name aside) as `on open`, in the same order",
                        ),
                    );
                };
                if let [message] = messages.as_slice() {
                    let mut idx = 0usize;
                    for p in &message.params {
                        if resolve_ty(&p.type_ref) == resolve_ty(in_type) {
                            continue; // the decoded frame, not a route value
                        }
                        if op
                            .get(idx)
                            .is_none_or(|o| resolve_ty(&p.type_ref) != resolve_ty(&o.type_ref))
                        {
                            route_mismatch(p, errors);
                        }
                        idx += 1;
                    }
                }
                if let [close] = closes.as_slice() {
                    for (i, p) in close.params.iter().enumerate() {
                        if op
                            .get(i)
                            .is_none_or(|o| resolve_ty(&p.type_ref) != resolve_ty(&o.type_ref))
                        {
                            route_mismatch(p, errors);
                        }
                    }
                }
            }
            // v0.104 (D2): on Workers the upgrade is routed to the Durable Object
            // that hosts the connection — the agent the `on open` transfers it to.
            // That target must be statically resolvable: exactly one top-level
            // transfer (`Agent(key).method(…, connection)`).
            let local_agents: std::collections::HashSet<String> =
                table.agents.keys().cloned().collect();
            for open in &opens {
                // v0.104 (slice 3b): an `on open` cannot `given` capabilities — on
                // Workers it runs inside the connection-hosting Durable Object, which
                // has no composition root to supply them (the capabilities belong on
                // the agent handler the connection transfers to).
                if !open.given.is_empty() {
                    errors.push(
                        CompileError::new(
                            "bynk.ws.open_given_unsupported",
                            open.span,
                            "a WebSocket `on open` handler cannot declare `given` capabilities — on Workers it runs inside the connection-hosting Durable Object, which has no composition root to supply them",
                        )
                        .with_note(
                            "move capability use into the agent handler the connection transfers to (it carries its own `given`)",
                        ),
                    );
                }
                use crate::websocket::{WsOpenShape, analyse_open_shape};
                match analyse_open_shape(&open.body, &local_agents) {
                    WsOpenShape::One(_) => {}
                    WsOpenShape::None => errors.push(
                        CompileError::new(
                            "bynk.ws.open_transfer_shape",
                            open.span,
                            "a WebSocket `on open` handler must transfer its `connection` into exactly one agent — e.g. `Room(roomId).join(…, connection)` — so the upgrade can be routed to the hosting Durable Object",
                        )
                        .with_note(
                            "transfer the connection to an agent unconditionally (not inside an `if`/`match`); a key derivable from a handler parameter routes the upgrade",
                        ),
                    ),
                    WsOpenShape::Multiple => errors.push(CompileError::new(
                        "bynk.ws.open_transfer_shape",
                        open.span,
                        "a WebSocket `on open` handler transfers its `connection` into more than one agent — the upgrade has no single Durable Object to route to",
                    )),
                }
            }
        }
        // #1781: a `from Events` service has exactly one `on event` handler.
        // Two were emitted as one object with a duplicate `event` key, which
        // `tsc --strict` rejects and which ran only the last in the bundle.
        if matches!(service.protocol, ServiceProtocol::Events { .. }) {
            let mut events = service
                .handlers
                .iter()
                .filter(|h| h.kind == HandlerKind::Event);
            if let (Some(first), Some(second)) = (events.next(), events.next()) {
                errors.push(
                    CompileError::new(
                        "bynk.event.duplicate_handler",
                        second.span,
                        format!(
                            "the `from Events` service `{}` has more than one `on event` handler — it needs exactly one",
                            service.name.name
                        ),
                    )
                    .with_label(first.span, "the service's `on event` handler")
                    .with_note(
                        "to react to one event in two ways, declare two services: each subscriber is delivered to independently",
                    ),
                );
            }
        }
        for handler in &service.handlers {
            let matches_protocol = matches!(
                (&service.protocol, &handler.kind),
                (ServiceProtocol::Call, HandlerKind::Call)
                    | (ServiceProtocol::Http, HandlerKind::Http { .. })
                    | (ServiceProtocol::Http, HandlerKind::Rejected)
                    | (ServiceProtocol::Cron, HandlerKind::Cron { .. })
                    | (ServiceProtocol::Queue { .. }, HandlerKind::Message)
                    // v0.103/v0.106: a `from websocket` admits `on open` (the
                    // upgrade), and the inbound/close lifecycle `on message`/`on
                    // close` (slice 3b-iii).
                    | (
                        ServiceProtocol::WebSocket { .. },
                        HandlerKind::Open | HandlerKind::Message | HandlerKind::Close
                    )
                    // Events track, slice 0 (spine #936): `from Events(E)`
                    // admits exactly `on event(e: E)`.
                    | (ServiceProtocol::Events { .. }, HandlerKind::Event)
            );
            if matches_protocol {
                // Events track, slice 1 (spine #936): a latent slice-0 gap —
                // nothing previously checked that `on event(e: E)`'s declared
                // parameter type agrees with the header's `from Events(E)`.
                // Harmless while no code depended on it; load-bearing now
                // that a subscription pattern (checked against the header's
                // `E`) assumes the body sees `e` at that same type. Runs
                // whether or not a pattern is present.
                if let ServiceProtocol::Events { event_type, .. } = &service.protocol
                    && handler.kind == HandlerKind::Event
                {
                    // #1781: emission is fire-and-forget, so a subscriber's
                    // result has nowhere to go; it returns `Effect[()]`. A
                    // non-`Effect` return is `bynk.service.return_not_effect`'s.
                    if let TypeRef::Effect(inner, _) = &handler.return_type
                        && !matches!(inner.as_ref(), TypeRef::Unit(_))
                    {
                        errors.push(
                            CompileError::new(
                                "bynk.event.return_not_effect_unit",
                                handler.return_type.span(),
                                format!(
                                    "an `on event` handler must return `Effect[()]`, but got `{}`",
                                    ts_type_ref_display(&handler.return_type)
                                ),
                            )
                            .with_note(
                                "emission is fire-and-forget: nothing receives a subscriber's result",
                            ),
                        );
                    }
                    if let Some(param) = handler.params.first() {
                        let header_name = type_ref_named(event_type);
                        let param_name = type_ref_named(&param.type_ref);
                        if header_name.is_none() || header_name != param_name {
                            errors.push(
                                CompileError::new(
                                    "bynk.event.handler_param_type_mismatch",
                                    param.type_ref.span(),
                                    format!(
                                        "this handler's parameter type does not match the header's event type `{}`",
                                        type_ref_to_display(event_type)
                                    ),
                                )
                                .with_note(
                                    "an `on event(e: E)` handler's parameter must be the same event type its `from Events(E)` header names",
                                ),
                            );
                        }
                    }
                    // Events track, slice 2 (spine #936): the arity/type
                    // check for the optional `env: EventEnvelope` second
                    // parameter — a latent gap independent of whether this
                    // slice's envelope machinery is ever used. Before this,
                    // `on event(e: E, extra: Whatever)` parsed and passed
                    // every existing check (the type-mismatch check above
                    // only ever inspected `params.first()`), then failed as
                    // a raw `tsc` argument-count error at the generated
                    // call site rather than a bynk diagnostic. A malformed
                    // *first* parameter is caught above already
                    // (`handler_param_type_mismatch` fires when position 0
                    // isn't the header's event type, including when it's
                    // `EventEnvelope` written in the wrong slot) — this
                    // check only adds the arity bound and the second
                    // parameter's required type.
                    match handler.params.len() {
                        0 => errors.push(
                            CompileError::new(
                                "bynk.event.bad_params",
                                handler.span,
                                "`on event` handlers take at least one parameter (the event payload)",
                            )
                            .with_note("add the payload parameter — e.g. `on event(e: E)`"),
                        ),
                        1 => {}
                        2 => {
                            let env_param = &handler.params[1];
                            if type_ref_named(&env_param.type_ref) != Some("EventEnvelope") {
                                errors.push(
                                    CompileError::new(
                                        "bynk.event.bad_params",
                                        env_param.type_ref.span(),
                                        "an `on event` handler's second parameter must be `EventEnvelope`",
                                    )
                                    .with_note(
                                        "the payload comes first; `EventEnvelope` carries runtime metadata about the emission (eventId, publisherId, emittedAt, schemaVersion)",
                                    ),
                                );
                            }
                        }
                        n => errors.push(CompileError::new(
                            "bynk.event.bad_params",
                            handler.params[2].span,
                            format!(
                                "`on event` handlers take at most two parameters (the event payload and, optionally, `EventEnvelope`), got {n}"
                            ),
                        )),
                    }
                }
                continue;
            }
            match &service.protocol {
                ServiceProtocol::Call => {
                    let suggested = match &handler.kind {
                        HandlerKind::Http { .. } | HandlerKind::Rejected => "from http",
                        HandlerKind::Cron { .. } => "from cron",
                        HandlerKind::Message => "from queue(\"…\")",
                        HandlerKind::Open | HandlerKind::Close => "from websocket(in: …, out: …)",
                        HandlerKind::Event => "from Events(EventType)",
                        HandlerKind::Call => continue,
                    };
                    errors.push(
                        CompileError::new(
                            "bynk.service.missing_from",
                            handler.span,
                            format!(
                                "this handler needs a protocol on the service header — add `{suggested}` to `service {}`",
                                service.name.name,
                            ),
                        )
                        .with_note("a service with no `from` clause admits only `on call` handlers"),
                    );
                }
                wire => {
                    errors.push(
                        CompileError::new(
                            "bynk.service.mixed_protocols",
                            handler.span,
                            format!(
                                "a `{}` service admits only its own handler form; this handler does not match",
                                protocol_label(wire),
                            ),
                        )
                        .with_note(
                            "a service is one protocol adapter — split differing handlers into separate services",
                        ),
                    );
                }
            }
        }
    }
}

fn protocol_label(p: &ServiceProtocol) -> &'static str {
    match p {
        ServiceProtocol::Call => "call",
        ServiceProtocol::Http => "from http",
        ServiceProtocol::Cron => "from cron",
        ServiceProtocol::Queue { .. } => "from queue",
        ServiceProtocol::WebSocket { .. } => "from websocket",
        ServiceProtocol::Events { .. } => "from Events",
    }
}

/// The bare name of a `TypeRef::Named` reference, or `None` for anything
/// else — an event type is always a plain named record, so this is enough
/// to compare a `from Events(E)` header against an `on event(e: T)`
/// handler's declared parameter type (Events track slice 1, spine #936).
fn type_ref_named(t: &TypeRef) -> Option<&str> {
    match t {
        TypeRef::Named(id) => Some(id.name.as_str()),
        _ => None,
    }
}

/// Render a type-ref in the same form the user wrote it, for diagnostics.
///
/// P4.1 (#1115): moved here from `bynk-emit/src/project/tests_emit.rs` — a
/// pure `TypeRef` renderer with no emission dependency, shared by this
/// module's own checks (`check_by_clause_contracts`, `check_service_decls`,
/// …) and by `bynk-emit`'s `tests_emit`/`project.rs`, which now call
/// `bynk_check::context_checks::ts_type_ref_display` instead of a local copy.
pub fn ts_type_ref_display(r: &TypeRef) -> String {
    match r {
        TypeRef::Base(b, _) => b.name().to_string(),
        TypeRef::Named(id) => crate::symbols::display_type_name(&id.name).to_string(),
        TypeRef::Result(t, e, _) => format!(
            "Result[{}, {}]",
            ts_type_ref_display(t),
            ts_type_ref_display(e)
        ),
        TypeRef::Option(t, _) => format!("Option[{}]", ts_type_ref_display(t)),
        TypeRef::Effect(t, _) => format!("Effect[{}]", ts_type_ref_display(t)),
        TypeRef::HttpResult(t, _) => format!("HttpResult[{}]", ts_type_ref_display(t)),
        TypeRef::QueueResult(_) => "QueueResult".to_string(),
        TypeRef::Asset(_) => "Asset".to_string(),
        TypeRef::RequestHeaders(_) => "RequestHeaders".to_string(),
        TypeRef::TokenClaims(_) => "TokenClaims".to_string(),
        TypeRef::BoundaryRejection(_) => "BoundaryRejection".to_string(),
        TypeRef::List(t, _) => format!("List[{}]", ts_type_ref_display(t)),
        TypeRef::Set(t, _) => format!("Set[{}]", ts_type_ref_display(t)),
        TypeRef::Query(t, _) => format!("Query[{}]", ts_type_ref_display(t)),
        TypeRef::Stream(t, _) => format!("Stream[{}]", ts_type_ref_display(t)),
        TypeRef::Connection(t, _) => format!("Connection[{}]", ts_type_ref_display(t)),
        TypeRef::History(t, _) => format!("History[{}]", ts_type_ref_display(t)),
        TypeRef::Map(k, v, _) => format!(
            "Map[{}, {}]",
            ts_type_ref_display(k),
            ts_type_ref_display(v)
        ),
        TypeRef::ValidationError(_) => "ValidationError".to_string(),
        TypeRef::JsonError(_) => "JsonError".to_string(),
        TypeRef::Unit(_) => "()".to_string(),
        // v0.157 (ADR 0183): render a generic-type application as written.
        TypeRef::App { name, args, .. } => format!(
            "{}[{}]",
            name.name,
            args.iter()
                .map(ts_type_ref_display)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        TypeRef::Fn(params, ret, _) => {
            let lhs = match params.len() {
                0 => "()".to_string(),
                1 if !matches!(params[0], TypeRef::Fn(..)) => ts_type_ref_display(&params[0]),
                _ => format!(
                    "({})",
                    params
                        .iter()
                        .map(ts_type_ref_display)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            format!("{lhs} -> {}", ts_type_ref_display(ret))
        }
    }
}

/// A short bynk-source-level rendering of a type reference for a diagnostic
/// message — not the TS-facing `ts_type_ref` family, which renders the
/// erased/emitted shape rather than what the author wrote.
pub fn type_ref_to_display(t: &TypeRef) -> String {
    match t {
        TypeRef::Named(id) => crate::symbols::display_type_name(&id.name).to_string(),
        TypeRef::Base(b, _) => b.name().to_string(),
        other => format!("{other:?}"),
    }
}

/// v0.45: actor-contract well-formedness and the handler `by`-clause checks.
///
/// Two passes: (1) each `actor` declaration is well-formed — the refinement
/// form's predicate is restricted to the closed actor-claim catalogue over a
/// `Bearer` base, the scheme is admitted, and a declared identity is a
/// context-ownable (sealed) type; (2) each service handler either
/// names an admissible actor on `by` or inherits the protocol default — and
/// HTTP requires an explicit `by`.
/// Validate one `by` clause's actor contracts against a protocol (v0.155,
/// factored so both a handler's own clause and a service-level default are
/// checked by the same logic). `params` is the enclosing handler's parameters —
/// `Some` for a real handler, `None` for a service-level default validated in
/// isolation (a default has no handler body, so the two body-shaped checks —
/// binder/parameter collision and `Signature`-requires-`body` — are skipped for
/// it and re-run per handler when the default is inherited).
fn check_by_clause_contracts(
    by: &bynk_syntax::ast::ByClause,
    params: Option<&[bynk_syntax::ast::Param]>,
    protocol: &ServiceProtocol,
    table: &UnitTable,
    refs: &mut RefSink,
    errors: &mut Vec<CompileError>,
) {
    use crate::actors::{self, Scheme};

    // A named binder introduces a new binding; it must not collide with a handler
    // parameter of the same name (which it would otherwise silently shadow in the
    // body scope). The binder-less form captures nothing, so it can't collide.
    // Only meaningful for a real handler (a default is validated without params).
    if let (Some(params), Some(binder)) = (params, &by.binder)
        && params.iter().any(|p| p.name.name == binder.name)
    {
        errors.push(
            CompileError::new(
                "bynk.actor.binder_shadows_param",
                binder.span,
                format!(
                    "the actor binder `{}` collides with a handler parameter of the same name",
                    binder.name,
                ),
            )
            .with_note("rename the `by` binder or the parameter"),
        );
    }
    // v0.52: a multi-actor sum (`by who: A | B`) must bind the resolved actor —
    // the body learns *which* peer verified by matching on the binder.
    if by.is_sum() && by.binder.is_none() {
        errors.push(
            CompileError::new(
                "bynk.actor.sum_requires_binder",
                by.span,
                "a multi-actor `by` clause must bind the resolved actor",
            )
            .with_note("write `by who: A | B (…)` and `match who { … }` in the body"),
        );
    }
    // Resolve each member to its contract: a local declaration *or* a prelude
    // actor. A local declaration that exists but is malformed (its scheme already
    // errored at the decl) does NOT fall through to a prelude actor of the same
    // name — only an unresolved name is. `members` keeps the resolved peers in
    // declared order for the reachability check below.
    let mut members: Vec<(&bynk_syntax::ast::Ident, actors::Contract)> = Vec::new();
    for actor_ref in &by.actors {
        let local = table.actors.get(&actor_ref.name);
        // A refinement actor (`actor A = B where …`) is never a peer: every `A`
        // is a `B`, so the arm is dead (Q3/Q4).
        if by.is_sum() && local.is_some_and(|a| a.refinement.is_some()) {
            errors.push(
                CompileError::new(
                    "bynk.actor.refinement_in_sum",
                    actor_ref.span,
                    format!(
                        "the refinement actor `{}` cannot be a peer in a multi-actor sum",
                        actor_ref.name
                    ),
                )
                .with_note(
                    "a refinement narrows a base actor — match it inside the \
                     resolved arm, not as a sum member",
                ),
            );
            continue;
        }
        let contract = if let Some(a) = local {
            refs.record(actor_ref.span, SymbolKind::Actor, &actor_ref.name);
            // v0.53: a refinement actor's contract is its base's scheme
            // (refinement elimination — an `Admin` is-a `User`); the invariant
            // rides the seam, not the scheme. A malformed refinement already
            // errored at its decl (pass 1).
            let scheme_actor = match &a.refinement {
                Some(r) => table.actors.get(&r.base.name),
                None => Some(a),
            };
            scheme_actor
                .and_then(|sa| sa.auth.as_ref())
                .and_then(|au| Scheme::from_name(&au.name))
                .filter(|s| s.admitted())
                .map(|scheme| actors::Contract {
                    scheme,
                    identity: actors::Identity::Unit,
                })
        } else {
            actors::prelude_actor(&actor_ref.name)
        };
        let Some(contract) = contract else {
            if local.is_none() {
                errors.push(
                    CompileError::new(
                        "bynk.actor.unknown_actor",
                        actor_ref.span,
                        format!("unknown actor `{}`", actor_ref.name),
                    )
                    .with_note(
                        "name a declared `actor` or a prelude actor \
                         (`Visitor`, `Scheduler`, `Producer`, `Caller`)",
                    ),
                );
            }
            continue;
        };
        if !actors::scheme_admissible(protocol, contract.scheme) {
            errors.push(
                CompileError::new(
                    "bynk.actor.scheme_not_admissible",
                    by.span,
                    format!(
                        "a `{}` actor is not admissible on a `{}` handler",
                        contract.scheme.as_str(),
                        protocol_label(protocol),
                    ),
                )
                .with_note(match protocol {
                    // I3 (#1971): a foreign caller calls a service, and only
                    // by `call`.
                    _ if contract.scheme == Scheme::Foreign => {
                        "a `Foreign` actor is admitted only on an `on call` handler"
                    }
                    ServiceProtocol::Http => {
                        "public HTTP routes take an anonymous actor — write `by v: Visitor`"
                    }
                    _ => "internal protocols (call/cron/queue) take an `Internal` actor",
                }),
            );
        }
        // v0.54: the `Caller` prelude actor yields a `CallerId` (the calling
        // context's name), a cross-context `on call` concept — it is admissible
        // only on the `Call` protocol, even though its `Internal` scheme is
        // otherwise valid on cron/queue (those take `Scheduler`/`Producer`).
        let is_caller = !table.actors.contains_key(&actor_ref.name)
            && actors::prelude_actor(&actor_ref.name).map(|c| c.identity)
                == Some(actors::Identity::CallerId);
        if is_caller && !matches!(protocol, ServiceProtocol::Call) {
            errors.push(
                CompileError::new(
                    "bynk.actor.scheme_not_admissible",
                    by.span,
                    format!(
                        "the `Caller` actor is not admissible on a `{}` handler",
                        protocol_label(protocol),
                    ),
                )
                .with_note(
                    "`Caller` carries the calling context's identity — it is only \
                     admissible on `on call`; cron takes `Scheduler`, queue takes `Producer`",
                ),
            );
        }
        // v0.151: `Oidc` is single-actor only this slice — a multi-actor sum owns
        // the whole boundary and reads the body once, a shape the OIDC seam (JWKS
        // fetch + async key import) does not yet fit. Reject it as a peer.
        if by.is_sum() && contract.scheme == actors::Scheme::Oidc {
            errors.push(
                CompileError::new(
                    "bynk.actor.oidc_not_in_sum",
                    actor_ref.span,
                    format!(
                        "the `Oidc` actor `{}` cannot be a peer in a multi-actor sum",
                        actor_ref.name
                    ),
                )
                .with_note(
                    "OIDC is single-actor this slice — give the route a single \
                     `by user: <OidcActor>` clause",
                ),
            );
        }
        members.push((actor_ref, contract));
    }
    // v0.51: a Signature member verifies an HMAC over the body, so the handler
    // MUST take a `body` parameter (single or sum). Skipped for a service-level
    // default (no handler body); re-checked per handler when inherited.
    if let Some(params) = params
        && members
            .iter()
            .any(|(_, c)| c.scheme == actors::Scheme::Signature)
        && !params.iter().any(|p| p.name.name == "body")
    {
        errors.push(
            CompileError::new(
                "bynk.actor.signature_requires_body",
                by.span,
                "a `Signature` handler must take a `body` parameter (the signature is over the body)",
            )
            .with_note("add a `(body: T)` parameter to the handler"),
        );
    }
    // v0.52: sum reachability — a decidable, scheme-level check. No two peers
    // share a scheme (the second is unreachable); a `None` catch-all (`Visitor`)
    // accepts everyone, so it must come last. The compiler does not reason about
    // predicate-level disjointness — that is what keeps this decidable (Q4).
    if by.is_sum() {
        let mut seen: Vec<actors::Scheme> = Vec::new();
        let mut seen_catch_all = false;
        for (actor_ref, contract) in &members {
            if seen_catch_all {
                errors.push(
                    CompileError::new(
                        "bynk.actor.unreachable_sum_arm",
                        actor_ref.span,
                        format!(
                            "actor `{}` is unreachable — an earlier `None` peer accepts every caller",
                            actor_ref.name
                        ),
                    )
                    .with_note("a catch-all (`None`, e.g. `Visitor`) peer must come last"),
                );
                continue;
            }
            if contract.scheme == actors::Scheme::None {
                seen_catch_all = true;
            } else if contract.scheme == actors::Scheme::Foreign {
                // I3 (#1971): foreign peers are told apart by name, each
                // verified under its own secret, so several can share a sum.
                // That every foreign secret is the context's only use of its
                // name is checked once per context (`check_actor_contracts`).
            } else if seen.contains(&contract.scheme) {
                errors.push(
                    CompileError::new(
                        "bynk.actor.duplicate_sum_scheme",
                        actor_ref.span,
                        format!(
                            "actor `{}` repeats the `{}` scheme of an earlier peer",
                            actor_ref.name,
                            contract.scheme.as_str()
                        ),
                    )
                    .with_note(
                        "peers in a sum are distinguished by scheme — two same-scheme \
                         peers can't both be reached",
                    ),
                );
            } else {
                seen.push(contract.scheme);
            }
        }
    }
}

fn check_actor_contracts(
    table: &UnitTable,
    resolved: &ResolvedCommons,
    refs: &mut RefSink,
    errors: &mut Vec<CompileError>,
) {
    use crate::actors::{self, Scheme};

    // Pass 1 — actor declaration well-formedness.
    for actor in table.actors.values() {
        refs.set_owner(&actor.name.name);
        // v0.53: a refinement actor (`actor Admin = User where <pred>`) carries
        // an authorisation invariant. Its base MUST be a declared `Bearer` actor
        // (only Bearer carries claims to authorise against), and its `where`
        // predicate MUST be in the closed claim-predicate set.
        if let Some(r) = &actor.refinement {
            let base = table.actors.get(&r.base.name);
            let base_is_bearer = base.is_some_and(|b| {
                b.refinement.is_none()
                    && b.auth.as_ref().and_then(|a| Scheme::from_name(&a.name))
                        == Some(Scheme::Bearer)
            });
            if base_is_bearer {
                refs.record(r.base.span, SymbolKind::Actor, &r.base.name);
            } else {
                errors.push(
                    CompileError::new(
                        "bynk.actor.refinement_base_unsupported",
                        r.base.span,
                        format!(
                            "the base actor `{}` of refinement `{}` must be a declared `Bearer` actor",
                            r.base.name, actor.name.name,
                        ),
                    )
                    .with_note(
                        "authorisation invariants test JWT claims, which only a `Bearer` actor \
                         carries — refine a `Bearer` actor, not `None`/`Internal`/`Signature`",
                    ),
                );
            }
            if let Err(span) = actors::parse_claim_predicate(&r.predicate) {
                errors.push(
                    CompileError::new(
                        "bynk.actor.refinement_predicate_unsupported",
                        span,
                        "a refinement predicate must be `hasClaim(\"…\")` or `claimEquals(\"…\", \"…\")`, composed with `&&`, `||`, `!`",
                    )
                    .with_note(
                        "claims are untyped JSON, so the predicate vocabulary is a closed set this \
                         slice; a general typed-claims surface is a later slice",
                    ),
                );
            }
            continue;
        }
        let Some(auth) = &actor.auth else {
            continue;
        };
        match Scheme::from_name(&auth.name) {
            None => errors.push(
                CompileError::new(
                    "bynk.actor.unknown_scheme",
                    auth.span,
                    format!("unknown authentication scheme `{}`", auth.name),
                )
                .with_note(
                    "the authentication schemes are `None`, `Internal`, `Bearer`, `Signature`, `Oidc`, and `Foreign`",
                ),
            ),
            // I3 (#1971): a foreign caller names the secret its calls verify
            // under; it carries no identity beyond the actor itself, and its
            // name must never read as a Bynk context's, which is what keeps
            // it apart from every `Caller` on the wire.
            Some(Scheme::Foreign) => {
                match actor.scheme_arg("secret") {
                    None => errors.push(
                        CompileError::new(
                            "bynk.actor.foreign_missing_secret",
                            auth.span,
                            "a `Foreign` actor must name the secret its calls are signed with",
                        )
                        .with_note("write `auth = Foreign(secret = \"<ENV_NAME>\")`"),
                    ),
                    Some(arg) if arg.value.as_str().is_none() => errors.push(
                        CompileError::new(
                            "bynk.actor.foreign_secret_not_string",
                            arg.span,
                            "a `Foreign` actor's secret must name an env binding, as a string",
                        )
                        .with_note("write `auth = Foreign(secret = \"<ENV_NAME>\")`"),
                    ),
                    Some(arg) => {
                        if arg.value.as_str().is_some_and(|v| {
                            v.starts_with(crate::wire_format::EDGE_FROM_PREFIX)
                                || v.starts_with(crate::wire_format::EDGE_TO_PREFIX)
                        }) {
                            errors.push(
                                CompileError::new(
                                    "bynk.actor.foreign_reserved_secret",
                                    arg.span,
                                    "a `Foreign` actor's secret may not use a `BYNK_CALL_` edge key's name",
                                )
                                .with_note(
                                    "those names are the keys `bynk deploy` mints between contexts — \
                                     pick another, such as `LEGACY_CALL_KEY`",
                                ),
                            );
                        }
                    }
                }
                if crate::wire_format::is_edge_name(&actor.name.name) {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.foreign_name_is_context_name",
                            actor.name.span,
                            format!(
                                "foreign actor `{}` is named like a context",
                                actor.name.name
                            ),
                        )
                        .with_note(
                            "a foreign caller sends its actor's name as its caller, so it must \
                             not read as a context's name: start it with a capital letter",
                        ),
                    );
                }
                // I4 (#1970): a foreign caller signs under the key its Worker
                // holds as `BYNK_CALL_TO_<CONTEXT>`, named from this context's
                // name, so the name must be one a key can be derived from (the
                // domain `bynk deploy` refuses outside of, at a deploy rather
                // than here, for Bynk-to-Bynk edges).
                let context = resolved.commons.name.joined();
                if !crate::wire_format::is_edge_name(&context) {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.foreign_context_name_unkeyable",
                            auth.span,
                            format!(
                                "context `{context}` cannot admit foreign caller `{}`: no call key can be named from the context's name",
                                actor.name.name
                            ),
                        )
                        .with_note(
                            "an edge key is named from the context's name, which must be lowercase \
                             letters and digits in dot-separated segments, each starting with a \
                             letter, with single underscores between runs (no `__`, no trailing `_`)",
                        ),
                    );
                }
                if let Some(id) = &actor.identity {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.foreign_identity_unsupported",
                            id.span(),
                            "a `Foreign` actor does not support a declared `identity`",
                        )
                        .with_note(
                            "the actor is the identity: a handler tells foreign callers apart by \
                             `match`ing its `by` sum",
                        ),
                    );
                }
            }
            // v0.47: a Bearer actor must name its signing secret and yield a
            // string-constructible identity (minted from the JWT `sub` claim).
            Some(Scheme::Bearer) => {
                if actor.scheme_arg("secret").is_none() {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.bearer_missing_secret",
                            auth.span,
                            "a `Bearer` actor must name its signing secret",
                        )
                        .with_note(
                            "write `auth = Bearer(secret = \"<ENV_NAME>\")` — the env var the \
                             `Secrets` capability resolves to the JWT signing key",
                        ),
                    );
                }
                match &actor.identity {
                    None => errors.push(
                        CompileError::new(
                            "bynk.actor.bearer_identity_not_string_constructible",
                            auth.span,
                            "a `Bearer` actor must declare a string-constructible `identity`",
                        )
                        .with_note(
                            "the verified identity is minted from the token's `sub` claim — \
                             declare `identity = T` where `T` is a refined or opaque `String`",
                        ),
                    ),
                    Some(id) if !is_string_constructible(id, &resolved.types) => errors.push(
                        CompileError::new(
                            "bynk.actor.bearer_identity_not_string_constructible",
                            id.span(),
                            "a `Bearer` actor's identity must be string-constructible",
                        )
                        .with_note(
                            "the identity is minted from the token's `sub` claim (a string) — \
                             use a refined or opaque `String` type",
                        ),
                    ),
                    Some(_) => {}
                }
            }
            // v0.51: a Signature actor must name its secret and signature header;
            // a `tolerance` requires a `timestamp`; identity is `()` (a declared
            // identity is not yet supported).
            Some(Scheme::Signature) => {
                if actor.scheme_arg("secret").is_none() {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.signature_missing_secret",
                            auth.span,
                            "a `Signature` actor must name its signing secret",
                        )
                        .with_note(
                            "write `auth = Signature(secret = \"<ENV_NAME>\", header = \"<Header>\")`",
                        ),
                    );
                }
                if actor.scheme_arg("header").is_none() {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.signature_missing_header",
                            auth.span,
                            "a `Signature` actor must name the signature header",
                        )
                        .with_note(
                            "write `header = \"<Header-Name>\"` — the request header carrying the HMAC",
                        ),
                    );
                }
                if let Some(tol) = actor.scheme_arg("tolerance")
                    && actor.scheme_arg("timestamp").is_none()
                {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.signature_tolerance_without_timestamp",
                            tol.span,
                            "`tolerance` requires a `timestamp` header to check against",
                        )
                        .with_note("add `timestamp = \"<Header>\"`, or drop `tolerance`"),
                    );
                }
                if let Some(id) = &actor.identity {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.signature_identity_unsupported",
                            id.span(),
                            "a `Signature` actor does not yet support a declared `identity`",
                        )
                        .with_note(
                            "a signature attests authenticity, not a principal — the event is the \
                             body param; use `by Webhook ()`",
                        ),
                    );
                }
            }
            // v0.151: an `Oidc` actor names its provider's public trust
            // parameters — `issuer` (checked against `iss`), `audience` (checked
            // against `aud`), and the `jwks` endpoint URL — and yields a
            // string-constructible identity minted from the verified `sub`
            // claim. It names **no secret**: the trust root is the provider's
            // published public key set, not a shared signing key.
            Some(Scheme::Oidc) => {
                if actor.scheme_arg("issuer").is_none() {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.oidc_missing_issuer",
                            auth.span,
                            "an `Oidc` actor must name its `issuer`",
                        )
                        .with_note(
                            "write `auth = Oidc(issuer = \"https://issuer.example\", audience = \"<aud>\", jwks = \"<jwks-url>\")` — \
                             the `iss` the verified token must carry",
                        ),
                    );
                }
                if actor.scheme_arg("audience").is_none() {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.oidc_missing_audience",
                            auth.span,
                            "an `Oidc` actor must name its `audience`",
                        )
                        .with_note(
                            "add `audience = \"<aud>\"` — the `aud` claim the token must be issued for (this API)",
                        ),
                    );
                }
                if actor.scheme_arg("jwks").is_none() {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.oidc_missing_jwks",
                            auth.span,
                            "an `Oidc` actor must name its `jwks` endpoint",
                        )
                        .with_note(
                            "add `jwks = \"https://issuer.example/.well-known/jwks.json\"` — the public key set the verifier fetches",
                        ),
                    );
                }
                // #2380 (L5): `trustFrom` names an environment variable whose JSON
                // overrides the declared trust roots at run time (tests, staging).
                if let Some(arg) = actor.scheme_arg("trustFrom") {
                    match arg.value.as_str() {
                        None => errors.push(
                            CompileError::new(
                                "bynk.actor.oidc_trust_from_not_string",
                                arg.span,
                                "`trustFrom` names an environment variable, as a string",
                            )
                            .with_note(
                                "write `trustFrom = \"MY_OIDC_TRUST\"`; when it is set, its JSON `{\"issuer\", \"jwks\", \"audience\"?}` replaces the declared trust roots",
                            ),
                        ),
                        Some(name) => {
                            let clashes = table.actors.values().any(|other| {
                                other
                                    .scheme_arg("secret")
                                    .and_then(|a| a.value.as_str())
                                    == Some(name)
                            });
                            if clashes {
                                errors.push(
                                    CompileError::new(
                                        "bynk.actor.oidc_trust_from_is_secret",
                                        arg.span,
                                        format!(
                                            "`trustFrom` names `{name}`, which an actor uses as its `secret`"
                                        ),
                                    )
                                    .with_note(
                                        "give the trust override its own environment variable: a secret is a key, not trust roots",
                                    ),
                                );
                            }
                        }
                    }
                }
                match &actor.identity {
                    None => errors.push(
                        CompileError::new(
                            "bynk.actor.oidc_identity_not_string_constructible",
                            auth.span,
                            "an `Oidc` actor must declare a string-constructible `identity`",
                        )
                        .with_note(
                            "the verified identity is minted from the token's `sub` claim — \
                             declare `identity = T` where `T` is a refined or opaque `String`",
                        ),
                    ),
                    Some(id) if !is_string_constructible(id, &resolved.types) => errors.push(
                        CompileError::new(
                            "bynk.actor.oidc_identity_not_string_constructible",
                            id.span(),
                            "an `Oidc` actor's identity must be string-constructible",
                        )
                        .with_note(
                            "the identity is minted from the token's `sub` claim (a string) — \
                             use a refined or opaque `String` type",
                        ),
                    ),
                    Some(_) => {}
                }
            }
            Some(_) => {}
        }
        // A declared identity must be a context-ownable (sealed) type — either
        // declared directly in this context, or a `uses`-imported commons type
        // this context's own emission rebrands (`uses_commons_type_names`,
        // `emit_context_rebrands`'s exact predicate) — either way, unforgeable
        // from outside the context. A `consumes`-surfaced cross-context type is
        // neither: it is not rebranded, so it stays excluded.
        //
        // Events track, slice 0 (spine #936) narrowed `local_type_names` itself
        // to "declared directly here" only (owner-only emission and `.raw`/
        // `.unsafe()` need exactly that, excluding `uses`-rebrands too) — this
        // check predates that narrowing and needs the broader "context-owned"
        // union back, so it reads `uses_commons_type_names` alongside it
        // instead of relying on the now-narrower `local_type_names` alone.
        // (Signature and Foreign handle their own identity rule above.)
        if !matches!(
            Scheme::from_name(actor.auth.as_ref().map(|a| a.name.as_str()).unwrap_or("")),
            Some(Scheme::Signature | Scheme::Foreign)
        ) && let Some(id) = &actor.identity
        {
            let ownable = matches!(id, TypeRef::Named(n) if
                resolved.is_local_type(&n.name) || resolved.is_uses_commons_type(&n.name));
            if !ownable {
                errors.push(
                    CompileError::new(
                        "bynk.actor.identity_not_sealed",
                        id.span(),
                        "an actor identity must be a context-ownable value type",
                    )
                    .with_note(
                        "declare the identity as a type in this context so it is sealed — \
                         minted only inside the context and unforgeable downstream",
                    ),
                );
            }
        }
    }

    // I3 (#1971): a foreign caller's secret is the context's only use of that
    // name. The Worker's table of foreign callers maps every one of them to
    // its secret, and a caller is checked under the secret its claimed name
    // selects, so two foreign actors on one secret could each pass for the
    // other wherever either is admitted; and `bynk deploy` mints a foreign
    // secret, which would overwrite another actor's. In declaration order, so
    // the later one is reported.
    let mut by_position: Vec<&ActorDecl> = table.actors.values().collect();
    by_position.sort_by_key(|a| a.name.span.start);
    let mut secret_owner: HashMap<&str, (&str, bool)> = HashMap::new();
    for actor in by_position {
        let Some(arg) = actor.scheme_arg("secret") else {
            continue;
        };
        let Some(secret) = arg.value.as_str() else {
            continue;
        };
        let foreign = actor
            .auth
            .as_ref()
            .is_some_and(|au| Scheme::from_name(&au.name) == Some(Scheme::Foreign));
        match secret_owner.get(secret) {
            Some((other, other_foreign)) if foreign || *other_foreign => {
                errors.push(
                    CompileError::new(
                        "bynk.actor.foreign_duplicate_secret",
                        arg.span,
                        format!(
                            "actor `{}` uses the secret `{secret}` already named by actor `{other}`, and one of them is a foreign caller",
                            actor.name.name
                        ),
                    )
                    .with_note(
                        "a foreign caller is told apart by the secret its calls verify under, and \
                         `bynk deploy` mints it — give it a name no other actor uses",
                    ),
                );
            }
            Some(_) => {}
            None => {
                secret_owner.insert(secret, (&actor.name.name, foreign));
            }
        }
    }

    // Pass 2 — handler `by`-clause contracts.
    for service in table.services.values() {
        refs.set_owner(&service.name.name);
        for handler in &service.handlers {
            match &handler.by_clause {
                Some(by) => {
                    check_by_clause_contracts(
                        by,
                        Some(&handler.params),
                        &service.protocol,
                        table,
                        refs,
                        errors,
                    );
                }
                // #2341 (decision C): `on rejected` has no actor; its own
                // checks (`check_rejected_handlers`) refuse a `by`.
                _ if handler.kind == HandlerKind::Rejected => {}
                None => {
                    // No `by`: edge protocols (HTTP, WebSocket) have no safe
                    // default actor; the internal protocols inherit one.
                    if actors::default_actor(&service.protocol).is_none() {
                        // v0.103 (D-A): a WebSocket upgrade authenticates at the
                        // edge before the connection is accepted — `on open` must
                        // name its actor, no anonymous upgrade.
                        let (msg, note) = match &service.protocol {
                            ServiceProtocol::WebSocket { .. } => (
                                "a WebSocket `on open` handler must declare its actor with a `by` clause",
                                "the upgrade authenticates at the edge before accepting the connection — name the actor (`by user: Participant`), there is no anonymous upgrade",
                            ),
                            _ => (
                                "an HTTP handler must declare its actor with a `by` clause",
                                "HTTP has no safe default actor — a public route writes `by v: Visitor`; an authenticated route names its actor",
                            ),
                        };
                        errors.push(
                            CompileError::new("bynk.actor.missing_by_on_http", handler.span, msg)
                                .with_note(note),
                        );
                    }
                }
            }
        }
        // v0.155: a service-level `by` default is validated *indirectly* — the
        // normalization pass injects it into the handlers that omit their own
        // clause, and the loop above checks those copies. So when the default is
        // inherited by **no** handler (every handler overrides it, or the service
        // has no handlers), it is injected into nothing and would go unchecked —
        // a typo'd/unknown default actor could pass silently, then surface later
        // at the header the moment an override is removed. Validate it directly
        // here in exactly that case, against the header span, so the diagnostic
        // is neither missed nor duplicated with the inherited-handler path.
        if let Some(default_by) = &service.default_by {
            let inherited = service.handlers.iter().any(|h| {
                h.by_clause
                    .as_ref()
                    .is_some_and(|b| b.span == default_by.span)
            });
            if !inherited {
                check_by_clause_contracts(default_by, None, &service.protocol, table, refs, errors);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_service_decls(
    typed: &mut checker::TypedCommons,
    table: &UnitTable,
    cross_context: &resolver::CrossContextInfo,
    resolved: &ResolvedCommons,
    capability_info_map: &HashMap<String, CapabilityInfo>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    requirements: &mut RequirementSink,
    errors: &mut Vec<CompileError>,
    tys: &Arc<Types>,
) {
    // v0.44: a service is one protocol adapter — every handler's form must
    // match the service's `from <protocol>` header.
    check_service_protocols(table, &typed.types, errors, tys);

    // v0.45: actor-contract well-formedness and the handler `by`-clause checks.
    check_actor_contracts(table, resolved, refs, errors);

    // #2341 (decision C): the shape of each service's `on rejected`.
    check_rejected_handlers(table, errors);

    // v0.9: validate HTTP handler shape and check for duplicate routes
    // across all services in this context.
    let mut route_first_span: HashMap<(HttpMethod, String), Span> = HashMap::new();
    for service in table.services.values() {
        for handler in &service.handlers {
            let HandlerKind::Http { method, path } = &handler.kind else {
                continue;
            };
            validate_http_handler(handler, *method, path, &typed.types, errors);
            let key = (*method, path.clone());
            if let Some(prev) = route_first_span.get(&key).copied() {
                errors.push(
                    CompileError::new(
                        "bynk.http.duplicate_route",
                        handler.span,
                        format!(
                            "duplicate HTTP route: another handler already declares `{} {}`",
                            method.as_str(),
                            path,
                        ),
                    )
                    .with_label(prev, "previously declared here"),
                );
            } else {
                route_first_span.insert(key, handler.span);
            }
        }
    }

    // v0.140 (ADR 0163): validate handler-position annotations (`@cache`) across
    // every handler — services and agents — so a misplaced annotation is caught
    // wherever it is written, not only on well-formed HTTP routes.
    for service in table.services.values() {
        let service_cap = service.limits.as_ref().and_then(|p| p.max_body());
        for handler in &service.handlers {
            validate_handler_annotations(handler, service_cap, errors);
        }
    }
    for agent in table.agents.values() {
        for handler in &agent.handlers {
            validate_handler_annotations(handler, None, errors);
        }
    }

    // v0.131 (ADR 0159): validate each service's `cors { }` policy.
    for service in table.services.values() {
        if let Some(policy) = &service.cors {
            validate_cors_policy(service, policy, errors);
        }
    }

    // v0.141 (ADR 0164): validate each service's `security { }` policy. (Absence
    // is legal and still stamps the safe defaults — only a *declared* block is
    // validated here.)
    for service in table.services.values() {
        if let Some(policy) = &service.security {
            validate_security_policy(service, policy, errors);
        }
    }

    // v0.142 (ADR 0165): validate each service's `limits { }` policy. (Absence is
    // legal — a service with no cap is unchanged; only a *declared* block is
    // validated here.)
    for service in table.services.values() {
        if let Some(policy) = &service.limits {
            validate_limits_policy(service, policy, errors);
        }
    }

    // v0.10a: validate `on cron` handler shape and check for duplicate
    // schedules across all services in this context (the generated
    // `scheduled` dispatcher routes on `event.cron`, so duplicates are
    // ambiguous).
    let mut schedule_first_span: HashMap<String, Span> = HashMap::new();
    for service in table.services.values() {
        for handler in &service.handlers {
            let HandlerKind::Cron { expr } = &handler.kind else {
                continue;
            };
            validate_cron_handler(handler, expr, errors);
            if let Some(prev) = schedule_first_span.get(expr).copied() {
                errors.push(
                    CompileError::new(
                        "bynk.cron.duplicate_schedule",
                        handler.span,
                        format!(
                            "duplicate cron schedule: another handler already declares `{expr}`",
                        ),
                    )
                    .with_label(prev, "previously declared here"),
                );
            } else {
                schedule_first_span.insert(expr.clone(), handler.span);
            }
        }
    }

    // v0.10b: validate `on queue` handler shape and check for duplicate
    // consumers across all services in this context (the generated `queue`
    // dispatcher routes on `batch.queue`, so two consumers of the same queue
    // are ambiguous).
    let mut consumer_first_span: HashMap<String, Span> = HashMap::new();
    for service in table.services.values() {
        let ServiceProtocol::Queue { name } = &service.protocol else {
            continue;
        };
        for handler in &service.handlers {
            if !matches!(handler.kind, HandlerKind::Message) {
                continue;
            }
            validate_queue_handler(handler, name, errors);
            if let Some(prev) = consumer_first_span.get(name).copied() {
                errors.push(
                    CompileError::new(
                        "bynk.queue.duplicate_consumer",
                        handler.span,
                        format!(
                            "duplicate queue consumer: another handler already consumes `{name}`",
                        ),
                    )
                    .with_label(prev, "previously declared here"),
                );
            } else {
                consumer_first_span.insert(name.clone(), handler.span);
            }
        }
    }

    // Check service handlers.
    for service in table.services.values() {
        refs.set_owner(&service.name.name);
        for handler in &service.handlers {
            // The given clause must reference only declared (local) or
            // exported (cross-context) capabilities.
            let mut handler_caps: HashMap<String, CapabilityInfo> = HashMap::new();
            for cap_ref in &handler.given {
                if let Some(info) =
                    resolve_given_cap_ref(cap_ref, capability_info_map, cross_context, errors, refs)
                {
                    handler_caps.insert(cap_ref.key().to_string(), info);
                }
            }
            // The handler return type must be Effect[T].
            if !matches!(handler.return_type, TypeRef::Effect(_, _)) {
                errors.push(CompileError::new(
                    "bynk.service.return_not_effect",
                    handler.return_type.span(),
                    format!(
                        "service handler must return `Effect[T]`, but got `{}`",
                        ts_type_ref_display(&handler.return_type)
                    ),
                ));
            }
            // v0.45: the `by`-bound actor identity, in scope for the body.
            let actor_binding =
                handler_actor_binding(handler, &service.protocol, table, resolved, tys);
            // #1170: persist it, keyed by this handler's own span — the
            // "no arena identity" substitute `TypedCommons::actor_bindings`'s
            // own doc comment names — so a post-`certify` consumer
            // (`bynk-emit::ir::lower`) can read it back once one exists.
            if let Some((binder, ty)) = &actor_binding {
                typed
                    .actor_bindings
                    .insert(handler.span, (binder.clone(), *ty));
            }
            // v0.103 (real-time track slice 3): an `on open` handler receives a
            // fresh owned `Connection[out]` named `connection`. Inject it as a
            // synthetic first parameter so the body type-checks against it and
            // the linearity pass seeds it as an owned held binding the handler
            // must dispose (transfer to an agent).
            // v0.103/v0.106: a `from websocket` lifecycle handler receives the
            // `connection` as a synthetic first param — the fresh owned socket for
            // `on open` (which must be disposed/transferred), or the **borrowed**
            // firing socket for `on message`/`on close` (used non-consumingly, never
            // disposed by the handler). The body type-checks against it either way;
            // the linearity pass treats the borrowed cases via `borrowed_held`.
            let is_ws_lifecycle = matches!(
                (&handler.kind, &service.protocol),
                (
                    HandlerKind::Open | HandlerKind::Message | HandlerKind::Close,
                    ServiceProtocol::WebSocket { .. }
                )
            );
            let params_for_check: Vec<Param> = match (&handler.kind, &service.protocol) {
                (
                    HandlerKind::Open | HandlerKind::Message | HandlerKind::Close,
                    ServiceProtocol::WebSocket { out_type, .. },
                ) => {
                    let mut ps = vec![open_connection_param(out_type, handler.span)];
                    ps.extend(handler.params.iter().cloned());
                    ps
                }
                _ => handler.params.clone(),
            };
            // The firing `connection` of `on message`/`on close` is borrowed, not
            // owned — no disposal obligation (contrast `on open`, owned).
            let borrowed_held: std::collections::HashSet<String> = if is_ws_lifecycle
                && matches!(handler.kind, HandlerKind::Message | HandlerKind::Close)
            {
                std::iter::once("connection".to_string()).collect()
            } else {
                std::collections::HashSet::new()
            };
            checker::check_handler_body(
                resolved,
                checker::HandlerBodyCheck {
                    capabilities: handler_caps,
                    declared_capabilities: capability_info_map.clone(),
                    given_anchor: Some(handler.return_type.span()),
                    report_unused: true,
                    actor_binding,
                    borrowed_held,
                    oidc_binder: crate::actors::oidc_seam_for(handler, &table.actors)
                        .and_then(|seam| seam.binder),
                    // #2683: a WebSocket handler runs in the connection-hosting
                    // Durable Object, whose `deps` carry no execution context.
                    service_handler: !is_ws_lifecycle,
                    ..checker::HandlerBodyCheck::new(
                        &handler.body,
                        &handler.return_type,
                        &params_for_check,
                        &handler.given,
                    )
                },
                checker::CheckSinks {
                    tys,
                    expr_types: &mut typed.expr_types,
                    errors,
                    refs,
                    hints,
                    locals,
                    requirements,
                    callees: &mut typed.callees,
                },
            );
        }
        // v0.155: like the `by` default (see check_actor_contracts), a service-
        // level `given` default is validated only through the handlers that
        // inherit it — the normalization pass injects it into handlers that
        // declare no `given` of their own. When it is inherited by no handler
        // (every handler declares its own `given`), resolve the default's
        // capabilities directly here so an unknown/typo'd default capability is
        // still reported, at the header. (A service always has ≥1 handler, so the
        // zero-handler case cannot arise; only full shadowing.)
        if let Some(first) = service.default_given.first() {
            let inherited = service
                .handlers
                .iter()
                .any(|h| h.given.first().is_some_and(|g| g.span == first.span));
            if !inherited {
                for cap_ref in &service.default_given {
                    let _ = resolve_given_cap_ref(
                        cap_ref,
                        capability_info_map,
                        cross_context,
                        errors,
                        refs,
                    );
                }
            }
        }
    }
}

/// v0.103: the synthetic `connection: Connection[out]` parameter an `on open`
/// handler receives — a fresh, owned held binding the framework supplies and the
/// handler must dispose (§2.9.4).
fn open_connection_param(out_type: &TypeRef, span: Span) -> Param {
    Param {
        name: Ident {
            name: "connection".to_string(),
            span,
        },
        type_ref: TypeRef::Connection(Box::new(out_type.clone()), span),
        span,
    }
}

/// v0.45: the actor binding a service handler exposes to its body, if it has a
/// `by <binder>: <Actor>` clause. Returns `(binder, identity_ty)`. Default-actor
/// handlers (no `by`) carry no named binding. The identity type is the actor's
/// declared `identity = T` (a context-ownable type), or the scheme default:
/// `()` for trivial actors, the calling-context id (`String`) for the prelude
/// `Caller` (Q7).
fn handler_actor_binding(
    handler: &Handler,
    _protocol: &ServiceProtocol,
    table: &UnitTable,
    resolved: &ResolvedCommons,
    tys: &Arc<Types>,
) -> Option<(String, checker::TyId)> {
    let by = handler.by_clause.as_ref()?;
    // No binder (binder-less `by <Actor>`) ⇒ no identity binding in scope.
    let binder = by.binder.as_ref()?;
    // A binder that collides with a parameter is diagnosed
    // (`bynk.actor.binder_shadows_param`); suppress the binding so the body
    // scope keeps the real parameter rather than the clobbering actor binding.
    if handler.params.iter().any(|p| p.name.name == binder.name) {
        return None;
    }
    // v0.52: a sum (`by who: A | B`) binds an `ActorSum` the body matches; a
    // single actor binds an `Actor` exposing `.identity`.
    let binder_ty = if by.is_sum() {
        tys.intern(checker::Ty::ActorSum(
            by.actors
                .iter()
                .map(|a| {
                    (
                        a.name.clone(),
                        actor_identity_ty(&a.name, table, resolved, tys),
                    )
                })
                .collect(),
        ))
    } else {
        tys.intern(checker::Ty::Actor(actor_identity_ty(
            &by.primary().name,
            table,
            resolved,
            tys,
        )))
    };
    Some((binder.name.clone(), binder_ty))
}

/// The identity `Ty` a named actor yields (a local declaration or a prelude
/// actor).
fn actor_identity_ty(
    actor_name: &str,
    table: &UnitTable,
    resolved: &ResolvedCommons,
    tys: &Arc<Types>,
) -> checker::TyId {
    actor_identity_ty_guarded(actor_name, table, resolved, &mut Vec::new(), tys)
}

/// Inner worker carrying a `seen` chain so a malformed **refinement cycle**
/// (`actor A = A`, or `A = B` / `B = A`) terminates with the unit identity
/// instead of overflowing the stack. A valid refinement's base is a direct
/// `Bearer` actor (the checker rejects refinement chains/cycles with
/// `refinement_base_unsupported`), so this guard only ever fires on input that
/// is already a compile error — it keeps the checker from crashing before that
/// diagnostic is reported.
fn actor_identity_ty_guarded<'a>(
    actor_name: &'a str,
    table: &'a UnitTable,
    resolved: &ResolvedCommons,
    seen: &mut Vec<&'a str>,
    tys: &Arc<Types>,
) -> checker::TyId {
    use crate::actors::{Identity, prelude_actor};
    if let Some(local) = table.actors.get(actor_name) {
        // v0.53: a refinement actor (`actor Admin = User where …`) yields its
        // base's identity — refinement elimination, an `Admin` is-a `User`.
        if let Some(r) = &local.refinement {
            if seen.contains(&actor_name) {
                return tys.intern(checker::Ty::Unit);
            }
            seen.push(actor_name);
            // Resolve against the declaration's own key so the cycle guard sees
            // the same name on a self-reference.
            if let Some((key, _)) = table.actors.get_key_value(&r.base.name) {
                return actor_identity_ty_guarded(key.as_str(), table, resolved, seen, tys);
            }
            return tys.intern(checker::Ty::Unit);
        }
        return match &local.identity {
            Some(id) => checker::resolve_type_ref(id, &resolved.types, tys)
                .unwrap_or_else(|| tys.intern(checker::Ty::Unit)),
            None => tys.intern(checker::Ty::Unit),
        };
    }
    match prelude_actor(actor_name).map(|c| c.identity) {
        Some(Identity::CallerId) => {
            tys.intern(checker::Ty::Base(bynk_syntax::ast::BaseType::String))
        }
        _ => tys.intern(checker::Ty::Unit),
    }
}

/// The closed storage-kind catalogue (design notes §10). `Cell` and `Map` are
/// functional; the rest (`Set`/`Log`/`Queue`/`Cache`) parse and validate as known
/// kinds but are gated (`bynk.store.kind_unsupported`).
const STORAGE_KINDS: &[&str] = &["Cell", "Map", "Set", "Log", "Queue", "Cache"];

/// The closed storage-annotation registry (ADR 0111 D2/D3): each `@name` with the
/// storage kind(s) it attaches to and the slice that makes it functional. v0.85
/// (slice 3a) lands the grammar + registry; every annotation is gated
/// (`bynk.store.annotation_unsupported`) until its slice — so `functional` is
/// `false` for all of them here, flipped per-name as later slices land.
struct AnnotationSpec {
    name: &'static str,
    kinds: &'static [&'static str],
    slice: &'static str,
    functional: bool,
}

const ANNOTATIONS: &[AnnotationSpec] = &[
    AnnotationSpec {
        name: "ttl",
        kinds: &["Cache"],
        slice: "the Cache slice",
        functional: true,
    },
    AnnotationSpec {
        name: "retain",
        kinds: &["Log"],
        slice: "the Log slice",
        functional: true,
    },
    AnnotationSpec {
        name: "indexed",
        kinds: &["Map"],
        slice: "the query-algebra track",
        functional: true,
    },
    AnnotationSpec {
        name: "bounded",
        kinds: &["Queue", "Log"],
        slice: "the Queue/Log slices",
        functional: false,
    },
];

/// Validate a `store` field's annotations against the closed registry (ADR 0111):
/// an unknown name is `bynk.store.unknown_annotation`; a known name on the wrong
/// kind is `bynk.store.annotation_kind_mismatch`; a known name on the right kind
/// whose slice has not landed is `bynk.store.annotation_unsupported`. `head` is
/// the (already known-valid) storage kind of the field.
fn validate_store_annotations(
    f: &StoreField,
    head: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    for ann in &f.annotations {
        let name = ann.name.name.as_str();
        let Some(spec) = ANNOTATIONS.iter().find(|s| s.name == name) else {
            errors.push(
                CompileError::new(
                    "bynk.store.unknown_annotation",
                    ann.name.span,
                    format!(
                        "unknown storage annotation `@{name}` — expected one of {}",
                        ANNOTATIONS
                            .iter()
                            .map(|s| format!("@{}", s.name))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
                .with_note("storage annotations are a closed set (ADR 0111)"),
            );
            continue;
        };
        if !spec.kinds.contains(&head) {
            errors.push(CompileError::new(
                "bynk.store.annotation_kind_mismatch",
                ann.span,
                format!(
                    "`@{name}` applies to {}, not `{head}`",
                    spec.kinds
                        .iter()
                        .map(|k| format!("`{k}`"))
                        .collect::<Vec<_>>()
                        .join("/")
                ),
            ));
            continue;
        }
        if !spec.functional {
            errors.push(
                CompileError::new(
                    "bynk.store.annotation_unsupported",
                    ann.span,
                    format!(
                        "`@{name}` is not yet supported — it lands with {}",
                        spec.slice
                    ),
                )
                .with_note(
                    "the annotation grammar is in place; its meaning arrives with its slice",
                ),
            );
            continue;
        }
        // v0.93 (ADR 0118): `@indexed(by: k, …)` — each `by:` names a
        // **value-keyable field of the map's value type** to maintain a secondary
        // index on. Validate the keys here, now the kind/value type are known.
        if name == "indexed" {
            validate_indexed_keys(f, types, ann, errors);
        }
    }
}

/// v0.93 (ADR 0118): each `@indexed(by: k)` key must label a `by:` argument that
/// names a **value-keyable field** of the map's value type (a `Record`). A
/// non-`by:` argument, a key that is not a field, or a non-keyable field type is
/// a diagnostic.
fn validate_indexed_keys(
    f: &StoreField,
    types: &HashMap<String, Arc<TypeDecl>>,
    ann: &Annotation,
    errors: &mut Vec<CompileError>,
) {
    // The map's value type is the second kind argument (`Map[K, V]`).
    let value_fields: Option<&[RecordField]> = f
        .kind
        .args
        .get(1)
        .and_then(|v| match v {
            TypeRef::Named(id) => types.get(&id.name),
            _ => None,
        })
        .and_then(|decl| match &decl.body {
            TypeBody::Record(r) => Some(r.fields.as_slice()),
            _ => None,
        });
    for arg in &ann.args {
        // Only `by:` labels are admitted on `@indexed`.
        let Some(label) = &arg.label else {
            errors.push(CompileError::new(
                "bynk.index.bad_argument",
                arg.span,
                "`@indexed` arguments are `by: <field>` labels naming a field to index on",
            ));
            continue;
        };
        if label.name != "by" {
            errors.push(CompileError::new(
                "bynk.index.bad_argument",
                arg.span,
                format!("`@indexed` takes `by:` arguments, not `{}:`", label.name),
            ));
            continue;
        }
        let ExprKind::Ident(key) = &arg.value.kind else {
            errors.push(CompileError::new(
                "bynk.index.bad_argument",
                arg.value.span,
                "`@indexed(by: …)` names a field of the map's value type",
            ));
            continue;
        };
        // The value type must be a record whose field `key` exists and is keyable.
        match value_fields.and_then(|fs| fs.iter().find(|rf| rf.name.name == key.name)) {
            None => {
                errors.push(CompileError::new(
                    "bynk.index.unknown_key",
                    arg.value.span,
                    format!(
                        "`@indexed(by: {0})` — the map's value type has no field `{0}`",
                        key.name
                    ),
                ));
            }
            Some(field) if !type_ref_is_keyable(&field.type_ref, types) => {
                errors.push(
                    CompileError::new(
                        "bynk.index.unkeyable_key",
                        arg.value.span,
                        format!(
                            "`@indexed(by: {0})` — field `{0}` is not value-keyable; an index key must be `Int`, `String`, or a refined/opaque type over them",
                            key.name
                        ),
                    ),
                );
            }
            Some(_) => {}
        }
    }
}

/// #1680 (runtime-semantics track S13): a store `Set`'s element and a store
/// `Map`'s or `Cache`'s key must be value-keyable, the rule value `Map` keys,
/// `distinct`/`groupBy` keys and `@indexed` fields already follow. A store
/// collection is a plain object keyed by its key's string form
/// (`rec[item] = true`), so a record key failed `tsc` (TS2538), and without
/// `tsc` every record would have keyed as `"[object Object]"`. A name that
/// doesn't resolve is reported by the resolver, so it is skipped here.
fn check_store_keyable(
    t: &TypeRef,
    what: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    if let TypeRef::Named(id) = t
        && !types.contains_key(&id.name)
    {
        return;
    }
    if !type_ref_is_keyable(t, types) {
        errors.push(
            CompileError::new(
                "bynk.store.unkeyable_key",
                t.span(),
                format!(
                    "{what} must be value-keyable — `String`, `Int`, or a refined/opaque type over them"
                ),
            )
            .with_note(
                "only `String` and `Int` keys (or a refined or opaque type over them) are supported, matching value `Map` keys and `@indexed` fields; key a record by one of its id fields, and an enum by its name as a `String`",
            ),
        );
    }
}

/// Whether a `TypeRef` is value-keyable (the Map-key / index-key rule, ADR 0110
/// D5): `Int`/`String`, including a refined/opaque named type over them.
fn type_ref_is_keyable(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    match t {
        TypeRef::Base(BaseType::Int | BaseType::String, _) => true,
        TypeRef::Named(id) => matches!(
            types.get(&id.name).map(|d| &d.body),
            Some(TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. })
                if matches!(base, BaseType::Int | BaseType::String)
        ),
        _ => false,
    }
}

/// v0.93 (ADR 0118 D4): index-hygiene **warnings** (non-failing, via ADR 0117).
/// Cross-references the agent's `@indexed(by: …)` declarations against the
/// equality `filter`s in its handlers:
///   - `bynk.index.missing` — an equality `filter` on a non-indexed keyable field
///     (the lookup scans; an index would route it);
///   - `bynk.index.unused` — a declared index no equality `filter` routes through
///     (it costs maintenance on every write).
///
/// These are perf hints, never compile gates (§11). The selectivity/ambiguity
/// tie-break (D5) and compound-predicate routing are a named follow-on, so a
/// single-equality predicate (the only shape routed today) is never ambiguous.
fn validate_index_hygiene(
    agent: &AgentDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    let mut store_maps: HashSet<String> = HashSet::new();
    // map → declared (field, span-of-the-`by:`-argument)
    let mut declared: HashMap<String, Vec<(String, Span)>> = HashMap::new();
    // map → the value type's record fields (for the keyability check)
    let mut value_fields: HashMap<String, Vec<RecordField>> = HashMap::new();
    for f in &agent.store_fields {
        if f.kind.head.name != "Map" || f.kind.args.len() != 2 {
            continue;
        }
        store_maps.insert(f.name.name.clone());
        if let Some(TypeBody::Record(r)) = f
            .kind
            .args
            .get(1)
            .and_then(|v| match v {
                TypeRef::Named(id) => types.get(&id.name),
                _ => None,
            })
            .map(|d| &d.body)
        {
            value_fields.insert(f.name.name.clone(), r.fields.clone());
        }
        for an in f.annotations.iter().filter(|a| a.name.name == "indexed") {
            for arg in &an.args {
                if arg.label.as_ref().map(|l| l.name.as_str()) == Some("by")
                    && let ExprKind::Ident(k) = &arg.value.kind
                {
                    declared
                        .entry(f.name.name.clone())
                        .or_default()
                        .push((k.name.clone(), arg.value.span));
                }
            }
        }
    }
    if store_maps.is_empty() {
        return;
    }
    // Walk every handler body for equality filters in the routable position
    // (`<map>.filter((r) => r.f == …)`), recording the (map, field) pairs hit and
    // warning about a missing index the first time a field is filtered on.
    let mut used: HashSet<(String, String)> = HashSet::new();
    let mut missing_seen: HashSet<(String, String)> = HashSet::new();
    for h in &agent.handlers {
        walk_block_for_index_filters(&h.body, &store_maps, &mut |map, field, span| {
            used.insert((map.to_string(), field.to_string()));
            let is_declared = declared
                .get(map)
                .is_some_and(|v| v.iter().any(|(f, _)| f == field));
            if is_declared {
                return;
            }
            let keyable = value_fields.get(map).is_some_and(|fs| {
                fs.iter()
                    .any(|rf| rf.name.name == field && type_ref_is_keyable(&rf.type_ref, types))
            });
            if keyable && missing_seen.insert((map.to_string(), field.to_string())) {
                errors.push(
                    CompileError::new(
                        "bynk.index.missing",
                        span,
                        format!(
                            "a query filters `{map}` by equality on `{field}`, which is not indexed — add `@indexed(by: {field})` to route this lookup through an index instead of a scan"
                        ),
                    )
                    .with_note("a perf hint, not an error — the scan still compiles and runs"),
                );
            }
        });
    }
    // A declared index no equality filter routes through is dead maintenance.
    for (map, fields) in &declared {
        for (field, span) in fields {
            if !used.contains(&(map.clone(), field.clone())) {
                errors.push(
                    CompileError::new(
                        "bynk.index.unused",
                        *span,
                        format!(
                            "`@indexed(by: {field})` on `{map}` is never used — no query filters `{map}` by equality on `{field}`, yet the index is maintained on every write"
                        ),
                    )
                    .with_note("remove it, or add a query that filters by equality on this field"),
                );
            }
        }
    }
}

/// `<map>.filter((r) => r.<field> == …)` with `map` a store map → `(map, field)`.
/// The routable equality-filter shape (the only one [`route_indexed_filter`]
/// lowers); deeper-in-a-chain filters cannot route, so they are not hygiene-relevant.
fn routable_eq_filter<'a>(
    store_maps: &HashSet<String>,
    e: &'a Expr,
) -> Option<(&'a str, &'a str, Span)> {
    let ExprKind::MethodCall {
        receiver,
        method,
        args,
        ..
    } = &e.kind
    else {
        return None;
    };
    if method.name != "filter" {
        return None;
    }
    let ExprKind::Ident(map) = &receiver.kind else {
        return None;
    };
    if !store_maps.contains(&map.name) {
        return None;
    }
    let [arg] = args.as_slice() else {
        return None;
    };
    let ExprKind::Lambda(lam) = &arg.kind else {
        return None;
    };
    let [param] = lam.params.as_slice() else {
        return None;
    };
    let pname = param.name.name.as_str();
    let ExprKind::BinOp(BinOp::Eq, lhs, rhs) = &lam.body.kind else {
        return None;
    };
    let field_of = |x: &'a Expr| -> Option<&'a str> {
        if let ExprKind::FieldAccess { receiver, field } = &x.kind
            && let ExprKind::Ident(r) = &receiver.kind
            && r.name == pname
        {
            Some(field.name.as_str())
        } else {
            None
        }
    };
    let field = field_of(lhs).or_else(|| field_of(rhs))?;
    Some((map.name.as_str(), field, e.span))
}

/// Recurse a block, invoking `cb(map, field, span)` for each routable equality
/// filter found anywhere in it.
fn walk_block_for_index_filters(
    block: &Block,
    store_maps: &HashSet<String>,
    cb: &mut dyn FnMut(&str, &str, Span),
) {
    let mut exprs = Vec::new();
    for stmt in &block.statements {
        statement_exprs(stmt, &mut exprs);
    }
    exprs.push(&block.tail);
    for e in exprs {
        walk_expr_for_index_filters(e, store_maps, cb);
    }
}

/// Recurse an expression, invoking `cb` for each routable equality filter.
/// Descends through `ast::expr_children` — the exhaustive total child
/// iterator — rather than a hand-matched recursion, so a future `ExprKind`
/// variant can't be silently skipped the way the old `_ => {}` here could.
fn walk_expr_for_index_filters(
    e: &Expr,
    store_maps: &HashSet<String>,
    cb: &mut dyn FnMut(&str, &str, Span),
) {
    if let Some((map, field, span)) = routable_eq_filter(store_maps, e) {
        cb(map, field, span);
    }
    for child in expr_children(e) {
        walk_expr_for_index_filters(child, store_maps, cb);
    }
}

/// v0.81/v0.82 (storage track): validate an agent's `store`-field kinds and build
/// the per-kind scopes — `Cell` fields (name → element type; bare reads + `:=`)
/// and `Map` fields (name → (key, value) types; effectful entry ops, ADR 0110).
/// Unknown heads, bad arity, and not-yet-supported kinds are diagnosed.
#[allow(clippy::type_complexity)]
fn store_field_scopes(
    agent: &AgentDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
    no_vars: &HashSet<String>,
    refs: &mut RefSink,
    errors: &mut Vec<CompileError>,
    tys: &Arc<Types>,
) -> (
    HashMap<String, TyId>,
    HashMap<String, (TyId, TyId)>,
    HashMap<String, TyId>,
    HashMap<String, (TyId, TyId, i64)>,
    HashMap<String, TyId>,
) {
    let mut cells: HashMap<String, TyId> = HashMap::new();
    let mut maps: HashMap<String, (TyId, TyId)> = HashMap::new();
    let mut sets: HashMap<String, TyId> = HashMap::new();
    let mut caches: HashMap<String, (TyId, TyId, i64)> = HashMap::new();
    let mut logs: HashMap<String, TyId> = HashMap::new();
    let arity_err = |f: &StoreField, kind: &str, want: usize, errors: &mut Vec<CompileError>| {
        errors.push(CompileError::new(
            "bynk.store.kind_arity",
            f.kind.span,
            format!(
                "`{kind}` takes exactly {want} type argument(s), found {}",
                f.kind.args.len()
            ),
        ));
    };
    for f in &agent.store_fields {
        let head = f.kind.head.name.as_str();
        if !STORAGE_KINDS.contains(&head) {
            errors.push(
                CompileError::new(
                    "bynk.store.unknown_kind",
                    f.kind.head.span,
                    format!(
                        "unknown storage kind `{head}` — expected one of {}",
                        STORAGE_KINDS.join(", ")
                    ),
                )
                .with_note("a `store` field's type is a storage kind, not an ordinary type"),
            );
            continue;
        }
        // v0.85 (ADR 0111): validate any `@…` annotations now the kind is known.
        validate_store_annotations(f, head, types, errors);
        match head {
            "Cell" => {
                if f.kind.args.len() != 1 {
                    arity_err(f, "Cell", 1, errors);
                    continue;
                }
                let elem = &f.kind.args[0];
                checker::record_type_refs(elem, types, no_vars, refs);
                if let Some(ty) = checker::resolve_type_ref(elem, types, tys) {
                    cells.insert(f.name.name.clone(), ty);
                }
            }
            "Map" => {
                if f.kind.args.len() != 2 {
                    arity_err(f, "Map", 2, errors);
                    continue;
                }
                checker::record_type_refs(&f.kind.args[0], types, no_vars, refs);
                checker::record_type_refs(&f.kind.args[1], types, no_vars, refs);
                check_store_keyable(&f.kind.args[0], "a `Map` key", types, errors);
                if let (Some(k), Some(v)) = (
                    checker::resolve_type_ref(&f.kind.args[0], types, tys),
                    checker::resolve_type_ref(&f.kind.args[1], types, tys),
                ) {
                    maps.insert(f.name.name.clone(), (k, v));
                }
            }
            "Set" => {
                if f.kind.args.len() != 1 {
                    arity_err(f, "Set", 1, errors);
                    continue;
                }
                let elem = &f.kind.args[0];
                checker::record_type_refs(elem, types, no_vars, refs);
                check_store_keyable(elem, "a `Set` element", types, errors);
                if let Some(ty) = checker::resolve_type_ref(elem, types, tys) {
                    sets.insert(f.name.name.clone(), ty);
                }
            }
            // v0.87 (ADR 0113): `Cache[K, V]` — a `Map` with per-entry TTL.
            "Cache" => {
                if f.kind.args.len() != 2 {
                    arity_err(f, "Cache", 2, errors);
                    continue;
                }
                checker::record_type_refs(&f.kind.args[0], types, no_vars, refs);
                checker::record_type_refs(&f.kind.args[1], types, no_vars, refs);
                check_store_keyable(&f.kind.args[0], "a `Cache` key", types, errors);
                // A `Cache` requires `@ttl(<Duration>)`; its millisecond value is
                // the entry lifetime. Absent → steer the author to a `Map`.
                let ttl = cache_ttl_millis(f, errors);
                if let (Some(k), Some(v), Some(ttl)) = (
                    checker::resolve_type_ref(&f.kind.args[0], types, tys),
                    checker::resolve_type_ref(&f.kind.args[1], types, tys),
                    ttl,
                ) {
                    caches.insert(f.name.name.clone(), (k, v, ttl));
                }
            }
            // v0.95 (ADR 0121): `Log[T]` — an append-only, time-indexed sequence.
            // The element type drives `append` and the lazy `Query[T]` read surface;
            // `@retain` (optional) is read by the emitter, not needed here.
            "Log" => {
                if f.kind.args.len() != 1 {
                    arity_err(f, "Log", 1, errors);
                    continue;
                }
                let elem = &f.kind.args[0];
                checker::record_type_refs(elem, types, no_vars, refs);
                if let Some(t) = checker::resolve_type_ref(elem, types, tys) {
                    logs.insert(f.name.name.clone(), t);
                }
            }
            other => {
                errors.push(
                    CompileError::new(
                        "bynk.store.kind_unsupported",
                        f.kind.head.span,
                        format!(
                            "storage kind `{other}` is not yet supported — `Cell`, `Map`, \
                             `Set`, `Cache`, and `Log` are functional in this storage-track slice"
                        ),
                    )
                    .with_note("the remaining kind (`Queue`) follows in a later slice"),
                );
            }
        }
    }
    (cells, maps, sets, caches, logs)
}

/// v0.87 (ADR 0113 D2): a `Cache` field must carry `@ttl(<Duration literal>)`;
/// return its value in milliseconds. A missing `@ttl`, or one present whose
/// first argument isn't itself a `Duration` literal (`@ttl(5)`, or
/// `@ttl(-5.minutes)` — unary negation over a `DurationLit` is not one), is
/// `bynk.store.cache_ttl_required`. Grounded during P6.7's own review
/// (#1163): no annotation-argument checker validates `@ttl`'s shape anywhere
/// else — this doc comment previously claimed otherwise — so leaving the
/// malformed case undiagnosed would let a `Cache` field with no resolvable
/// TTL reach a certified program.
fn cache_ttl_millis(f: &StoreField, errors: &mut Vec<CompileError>) -> Option<i64> {
    let ttl = f.annotations.iter().find(|a| a.name.name == "ttl");
    let Some(ttl) = ttl else {
        errors.push(
            CompileError::new(
                "bynk.store.cache_ttl_required",
                f.kind.span,
                "a `Cache` field requires a `@ttl(<duration>)` annotation — its entry lifetime",
            )
            .with_note("a keyed store with no expiry is a `Map`, not a `Cache`"),
        );
        return None;
    };
    match ttl.args.first().map(|a| &a.value.kind) {
        Some(ExprKind::DurationLit { millis, .. }) => Some(*millis),
        _ => {
            let span = ttl.args.first().map_or(ttl.span, |a| a.span);
            errors.push(
                CompileError::new(
                    "bynk.store.cache_ttl_required",
                    span,
                    "`@ttl`'s argument must be a duration literal, e.g. `5.minutes`",
                )
                .with_note("a keyed store with no expiry is a `Map`, not a `Cache`"),
            );
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn check_agent_decls(
    typed: &mut checker::TypedCommons,
    table: &UnitTable,
    cross_context: &resolver::CrossContextInfo,
    is_context: bool,
    uses_commons_type_names: &HashSet<String>,
    capability_info_map: &HashMap<String, CapabilityInfo>,
    no_vars: &HashSet<String>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    requirements: &mut RequirementSink,
    errors: &mut Vec<CompileError>,
    tys: &Arc<Types>,
) {
    for agent in table.agents.values() {
        refs.set_owner(&agent.name.name);
        // v0.81 (storage track, emission slice — ADR 0109): `store` `Cell` fields
        // are checked (kind validity, bare reads, the `:=` write form, invariant
        // resolution) *and* emitted — the cells form the agent's state record,
        // written through a staged working copy committed atomically at handler
        // end. `store_cells` maps each `Cell` field to its element type, for the
        // bare-read scope and the `:=`/invariant checks below.
        #[allow(clippy::type_complexity)]
        let (store_cells, store_maps, store_sets, store_caches, store_logs): (
            HashMap<String, TyId>,
            HashMap<String, (TyId, TyId)>,
            HashMap<String, TyId>,
            HashMap<String, (TyId, TyId, i64)>,
            HashMap<String, TyId>,
        ) = if agent.store_fields.is_empty() {
            (
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
                HashMap::new(),
            )
        } else {
            store_field_scopes(agent, &typed.types, no_vars, refs, errors, tys)
        };
        // v0.93 (ADR 0118 D4): index-hygiene warnings cross-reference `@indexed`
        // declarations against the equality filters in the handlers.
        validate_index_hygiene(agent, &typed.types, errors);
        // v0.25: the agent's key type and store field types reference types.
        checker::record_type_refs(&agent.key_type, &typed.types, no_vars, refs);
        for field in &agent.store_fields {
            for arg in &field.kind.args {
                checker::record_type_refs(arg, &typed.types, no_vars, refs);
            }
        }
        // The agent's `Cell` fields form its state record. Expose that record
        // under the name `<AgentName>State` in the type table so the body and
        // invariants can be checked against it.
        let agent_state_name = format!("{}State", agent.name.name);
        // #1697: the state record is a name the agent's handlers and invariants
        // can write, so a user type of the same name would be silently replaced
        // there by the record (and collide with its interface in the emitted
        // module). Reject it instead.
        if let Some(user) = table.types.get(&agent_state_name) {
            errors.push(
                CompileError::new(
                    "bynk.agent.state_name_conflict",
                    user.name.span,
                    format!(
                        "type `{agent_state_name}` has the name of agent `{}`'s state record",
                        agent.name.name
                    ),
                )
                .with_note(format!(
                    "inside `{}`'s handlers, `{agent_state_name}` is the record of its `Cell` fields, so this type would be replaced there; rename it",
                    agent.name.name
                )),
            );
        }
        let state_record_fields: Vec<RecordField> = agent
            .store_fields
            .iter()
            .filter(|f| f.kind.head.name == "Cell" && f.kind.args.len() == 1)
            .map(|f| RecordField {
                documentation: None,
                trivia: Default::default(),
                name: f.name.clone(),
                type_ref: f.kind.args[0].clone(),
                refinement: None,
                init: f.init.clone(),
                span: f.span,
            })
            .collect();
        // Build a synthetic Record TypeDecl and stuff it into a *clone* of
        // the resolved types so handler bodies see it.
        let synthetic_state = TypeDecl {
            derives: Vec::new(),
            derives_kw: None,
            name: Ident {
                name: agent_state_name.clone(),
                span: agent.span,
            },
            type_params: Vec::new(),
            body: TypeBody::Record(RecordBody {
                trailing_comments: Default::default(),
                fields: state_record_fields,
                span: agent.span,
            }),
            documentation: None,
            span: agent.span,
            trivia: Trivia::default(),
            private: None,
        };
        let mut types_for_handler = typed.types.clone();
        types_for_handler.insert(agent_state_name.clone(), Arc::new(synthetic_state.clone()));
        // `local_type_names` is derived from `table.types` (the pre-merge
        // local table), NOT `types_for_handler` (local+uses+consumes, plus
        // the synthetic state record) — reusing the merged table here was
        // review finding #9: it silently over-widened `.raw`/`.unsafe()`/
        // owner-only-event-emission to any consumed/used type inside an
        // agent handler body, making all three gates unreachable there.
        let resolved_for_handler = ResolvedCommons::new(
            typed.commons.clone(),
            types_for_handler,
            &table.types,
            typed.fns.clone(),
            typed.methods.clone(),
            table.agents.clone(),
            &table.events,
            cross_context.clone(),
            HashMap::new(),
            is_context,
            uses_commons_type_names.clone(),
        )
        // #1883, slice 1a: a handler body sees the unit's assets too.
        .with_assets(typed.assets.clone())
        .with_embeds(typed.embeds.clone())
        .with_consts(typed.consts.clone())
        .with_shadowed_imports(typed.shadowed_imports.clone())
        .with_extensible_enums(typed.extensible_enums.clone())
        .with_stability(typed.stability.clone());
        // v0.81: the fresh-key rule for `store Cell[T]` fields — an
        // initialiser is checked against the element type `T` (which also types
        // the init expression so the emitter can qualify variant constructors),
        // and a field with neither an initialiser nor an implicit zero is rejected.
        for field in &agent.store_fields {
            if field.kind.head.name != "Cell" || field.kind.args.len() != 1 {
                continue; // non-Cell / malformed kinds are diagnosed elsewhere
            }
            let elem = &field.kind.args[0];
            if let Some(init) = &field.init {
                checker::check_state_initialiser(
                    init,
                    elem,
                    &resolved_for_handler,
                    tys,
                    &mut typed.expr_types,
                    &mut typed.callees,
                    errors,
                    refs,
                    hints,
                    locals,
                );
            } else if checker::zero_value_ts(elem, None, &typed.types).is_none() {
                errors.push(
                    CompileError::new(
                        "bynk.agents.non_zeroable_state_field",
                        field.span,
                        format!(
                            "agent `{}` store cell `{}` has no defined zero value, so a fresh \
                             key cannot be initialised",
                            agent.name.name, field.name.name
                        ),
                    )
                    .with_note(
                        "add an initialiser (`store name: Cell[T] = value`), or use \
                         `Cell[Option[…]]` (None means \"never set\")",
                    ),
                );
            }
        }
        let state_ty = tys.intern(Ty::Named {
            name: agent_state_name.clone(),
            kind: checker::NamedKind::Record,
            args: Vec::new(),
        });
        let key_ty = checker::resolve_type_ref(&agent.key_type, &typed.types, tys)
            .unwrap_or_else(|| tys.intern(Ty::Unit));
        let mut self_scope: HashMap<String, TyId> = HashMap::new();
        // `self` is a synthetic record carrying the agent's key field, so that
        // `self.<key>` resolves. The parser treats `self.x` as FieldAccess on
        // Ident("self"), so `self` is given a one-off synthetic record type.
        let agent_self_name = format!("__{}Self", agent.name.name);
        let self_decl = TypeDecl {
            derives: Vec::new(),
            derives_kw: None,
            name: Ident {
                name: agent_self_name.clone(),
                span: agent.span,
            },
            type_params: Vec::new(),
            body: TypeBody::Record(RecordBody {
                trailing_comments: Default::default(),
                fields: vec![RecordField {
                    documentation: None,
                    trivia: Default::default(),
                    name: Ident {
                        name: agent.key_name.name.clone(),
                        span: agent.key_name.span,
                    },
                    type_ref: agent.key_type.clone(),
                    refinement: None,
                    init: None,
                    span: agent.key_name.span,
                }],
                span: agent.span,
            }),
            documentation: None,
            span: agent.span,
            trivia: Trivia::default(),
            private: None,
        };
        let mut types_for_handler = resolved_for_handler.types.clone();
        types_for_handler.insert(agent_self_name.clone(), Arc::new(self_decl.clone()));
        // Same fix as above: `local_type_names` comes from `table.types`
        // (pre-merge), not `types_for_handler` (merged, plus the synthetic
        // `self` record type).
        let resolved_for_handler = ResolvedCommons::new(
            typed.commons.clone(),
            types_for_handler,
            &table.types,
            typed.fns.clone(),
            typed.methods.clone(),
            table.agents.clone(),
            &table.events,
            cross_context.clone(),
            HashMap::new(),
            is_context,
            uses_commons_type_names.clone(),
        )
        // #1883, slice 1a: a handler body sees the unit's assets too.
        .with_assets(typed.assets.clone())
        .with_embeds(typed.embeds.clone())
        .with_consts(typed.consts.clone())
        .with_shadowed_imports(typed.shadowed_imports.clone())
        .with_extensible_enums(typed.extensible_enums.clone())
        .with_stability(typed.stability.clone());
        self_scope.insert(
            "self".to_string(),
            tys.intern(Ty::Named {
                name: agent_self_name.clone(),
                kind: checker::NamedKind::Record,
                args: Vec::new(),
            }),
        );
        // v0.81: each `Cell` store field is a bare local of its element type
        // (implicit deref in read position); the `:=` write form is checked
        // separately against `store_cells`.
        for (name, ty) in &store_cells {
            self_scope.insert(name.clone(), *ty);
        }
        let _ = key_ty;

        // Finding #36: `check_handler_body` takes the five kind-scopes as one
        // `HashMap<String, StoreField>` — a field name is only ever one kind,
        // so recombine them here rather than threading five parallel maps.
        let store_fields: HashMap<String, checker::StoreField> = store_cells
            .iter()
            .map(|(name, t)| (name.clone(), checker::StoreField::Cell(*t)))
            .chain(
                store_maps
                    .iter()
                    .map(|(name, (k, v))| (name.clone(), checker::StoreField::Map(*k, *v))),
            )
            .chain(
                store_sets
                    .iter()
                    .map(|(name, t)| (name.clone(), checker::StoreField::Set(*t))),
            )
            .chain(store_caches.iter().map(|(name, (k, v, ttl))| {
                (name.clone(), checker::StoreField::Cache(*k, *v, *ttl))
            }))
            .chain(
                store_logs
                    .iter()
                    .map(|(name, t)| (name.clone(), checker::StoreField::Log(*t))),
            )
            .collect();

        // v0.80/v0.81: invariant well-formedness — predicates are pure `Bool`
        // expressions over the agent's `store` cells (§14, ADR 0108 D5).
        checker::check_invariants(
            &agent.invariants,
            &store_cells,
            &agent.name.name,
            &resolved_for_handler,
            tys,
            &mut typed.expr_types,
            errors,
            refs,
            hints,
            locals,
            requirements,
            &mut typed.callees,
        );

        // v0.116 (testing track slice 4): step invariants — predicates over the
        // `old`/`new` state pair, checked against the synthetic state record.
        checker::check_transitions(
            &agent.transitions,
            state_ty,
            &agent.name.name,
            &resolved_for_handler,
            &mut typed.expr_types,
            errors,
            refs,
            hints,
            locals,
            requirements,
            &mut typed.callees,
            tys,
        );

        // State-migrations S2 (#2224): an old type's field defaults stand in for
        // an initialiser that field had (Decision C). They are spliced into the
        // old type's own decoder, so they follow an event default's rules.
        for step in &agent.migrations {
            let Some(TypeRef::Named(id)) = step.old.as_ref().map(|o| &o.type_ref) else {
                continue;
            };
            let Some(decl) = resolved_for_handler.types.get(&id.name).cloned() else {
                continue;
            };
            let TypeBody::Record(r) = &decl.body else {
                continue;
            };
            for field in &r.fields {
                let Some(init) = &field.init else {
                    continue;
                };
                let before = errors.len();
                checker::check_event_field_default(
                    init,
                    &field.type_ref,
                    "old-state field default",
                    &resolved_for_handler,
                    tys,
                    &mut typed.expr_types,
                    &mut typed.callees,
                    errors,
                    refs,
                    hints,
                    locals,
                );
                if errors.len() > before {
                    continue;
                }
                if let Err(reason) = crate::wire_default::lower_field_default_wire(
                    init,
                    &field.type_ref,
                    &resolved_for_handler.types,
                ) {
                    errors.push(CompileError::new(
                        "bynk.types.bad_field_default",
                        init.span,
                        format!(
                            "old-state field `{}`'s default cannot be represented on the wire: \
                             {reason}",
                            field.name.name
                        ),
                    ));
                }
            }
        }

        // State-migrations S2 (#2224): each `migrate` step's body, on its own
        // terms (purity, key-free, `:=` targets, record-literal semantics).
        // The lock-dependent checks run in the schema-registry reconcile.
        checker::check_migrations(
            agent,
            &store_cells,
            &resolved_for_handler.types,
            &resolved_for_handler,
            tys,
            &mut typed.expr_types,
            errors,
            refs,
            hints,
            locals,
            requirements,
            &mut typed.callees,
        );
        // #2221: a move a handler can make that a `transition` refuses at
        // commit, from the agent state model the editor's state view also
        // reads. The type table is the agent's own (local, `uses` and
        // `consumes`), so a state type declared elsewhere is still seen.
        errors.extend(crate::agent_state::may_violate_diagnostics(
            agent,
            &resolved_for_handler.types,
        ));

        for handler in &agent.handlers {
            // v0.99 (DECISION H): `by` is a service-edge clause — it establishes
            // the actor (`identity`/`who`) from the inbound request. An agent
            // `on call` handler is reached across the agent boundary by the
            // factory (`__makeAgent`), never from an ingress, so it has no actor
            // and the parser-accepted `by` clause would silently be dropped.
            // Rejecting it turns the deps-split taxonomy's "actor auth never
            // crosses the agent boundary" guarantee into an enforced invariant.
            if let Some(by) = &handler.by_clause {
                errors.push(
                    CompileError::new(
                        "bynk.actor.by_on_agent",
                        by.span,
                        "`by` is a service-edge clause; an agent handler has no actor",
                    )
                    .with_note(
                        "an agent `on call` handler is invoked across the agent boundary, not \
                         from an ingress — remove the `by` clause",
                    ),
                );
            }
            let mut handler_caps: HashMap<String, CapabilityInfo> = HashMap::new();
            for cap_ref in &handler.given {
                if let Some(info) =
                    resolve_given_cap_ref(cap_ref, capability_info_map, cross_context, errors, refs)
                {
                    handler_caps.insert(cap_ref.key().to_string(), info);
                }
            }
            // The handler return type must be Effect[T].
            if !matches!(handler.return_type, TypeRef::Effect(_, _)) {
                errors.push(CompileError::new(
                    "bynk.agent.return_not_effect",
                    handler.return_type.span(),
                    format!(
                        "agent handler must return `Effect[T]`, but got `{}`",
                        ts_type_ref_display(&handler.return_type)
                    ),
                ));
            }
            checker::check_handler_body(
                &resolved_for_handler,
                checker::HandlerBodyCheck {
                    capabilities: handler_caps,
                    declared_capabilities: capability_info_map.clone(),
                    agent_state_ty: Some(state_ty),
                    agent_self_scope: Some(self_scope.clone()),
                    given_anchor: Some(handler.return_type.span()),
                    report_unused: true,
                    store_fields: store_fields.clone(),
                    ..checker::HandlerBodyCheck::new(
                        &handler.body,
                        &handler.return_type,
                        &handler.params,
                        &handler.given,
                    )
                },
                checker::CheckSinks {
                    tys,
                    expr_types: &mut typed.expr_types,
                    errors,
                    refs,
                    hints,
                    locals,
                    requirements,
                    callees: &mut typed.callees,
                },
            );
        }
    }
}

/// Validate a service's `cors { }` policy (v0.131, ADR 0159). The grammar is
/// lenient — any `name: value` field parses — so the checker is where the closed
/// field set, the value shapes, and the spec-mandated wildcard/credentials
/// constraint (DECISION F) are enforced.
fn validate_cors_policy(
    service: &ServiceDecl,
    policy: &CorsPolicy,
    errors: &mut Vec<CompileError>,
) {
    // CORS is a browser-facing HTTP concern; it is meaningless on any other
    // protocol.
    if !matches!(service.protocol, ServiceProtocol::Http) {
        errors.push(
            CompileError::new(
                "bynk.http.cors_not_http",
                policy.span,
                "a `cors { }` policy is only valid on a `from http` service",
            )
            .with_note("CORS governs cross-origin browser access, which only the HTTP surface has"),
        );
        return;
    }

    // Field names are a closed set; flag anything else (the parser accepts any
    // name, per the annotation precedent).
    for field in &policy.fields {
        if !matches!(
            field.name.name.as_str(),
            "origins" | "headers" | "credentials" | "maxAge"
        ) {
            errors.push(
                CompileError::new(
                    "bynk.http.cors_unknown_field",
                    field.name.span,
                    format!("unknown `cors` field `{}`", field.name.name),
                )
                .with_note("known fields are `origins`, `headers`, `credentials`, and `maxAge`"),
            );
        }
    }

    // `origins` is required and must be a non-empty list of string literals.
    match policy.field("origins") {
        None => errors.push(CompileError::new(
            "bynk.http.cors_invalid_origins",
            policy.span,
            "a `cors { }` policy must declare `origins` — the allowed origins, or `[\"*\"]`",
        )),
        Some(expr) => match &expr.kind {
            ExprKind::ListLit(items) if !items.is_empty() => {
                for item in items {
                    if !matches!(item.kind, ExprKind::StrLit(_)) {
                        errors.push(CompileError::new(
                            "bynk.http.cors_invalid_origins",
                            item.span,
                            "each `cors` origin must be a string literal (e.g. \"https://app.example.com\" or \"*\")",
                        ));
                    }
                }
            }
            _ => errors.push(CompileError::new(
                "bynk.http.cors_invalid_origins",
                expr.span,
                "`cors` `origins` must be a non-empty list of string literals",
            )),
        },
    }

    // `headers`, when present, is a list of string literals.
    if let Some(expr) = policy.field("headers") {
        let ok = matches!(&expr.kind, ExprKind::ListLit(items)
            if items.iter().all(|i| matches!(i.kind, ExprKind::StrLit(_))));
        if !ok {
            errors.push(CompileError::new(
                "bynk.http.cors_invalid_field",
                expr.span,
                "`cors` `headers` must be a list of string literals",
            ));
        }
    }

    // `credentials`, when present, is a boolean literal.
    if let Some(expr) = policy.field("credentials")
        && !matches!(expr.kind, ExprKind::BoolLit(_))
    {
        errors.push(CompileError::new(
            "bynk.http.cors_invalid_field",
            expr.span,
            "`cors` `credentials` must be `true` or `false`",
        ));
    }

    // `maxAge`, when present, is a `Duration` literal.
    if let Some(expr) = policy.field("maxAge")
        && !matches!(expr.kind, ExprKind::DurationLit { .. })
    {
        errors.push(CompileError::new(
            "bynk.http.cors_invalid_field",
            expr.span,
            "`cors` `maxAge` must be a `Duration` literal (e.g. `1.hours`)",
        ));
    }

    // DECISION F: the Fetch spec forbids `Access-Control-Allow-Credentials: true`
    // with a wildcard origin — the browser rejects it at runtime, so catch it at
    // compile time.
    if policy.credentials() && policy.is_wildcard() {
        errors.push(
            CompileError::new(
                "bynk.http.cors_wildcard_credentials",
                policy.span,
                "`cors` cannot combine `credentials: true` with the wildcard origin `[\"*\"]`",
            )
            .with_note(
                "the Fetch spec forbids credentialed requests against a wildcard origin — \
                 list the exact origins instead",
            ),
        );
    }
}

/// v0.141 (ADR 0164): validate a service's `security { }` policy. Security
/// response headers are wire behaviour of the browser-facing HTTP surface, so the
/// section is only legal on a `from http` service; the field vocabulary is the
/// closed set `hsts`/`nosniff`; `hsts` is a *positive* `Duration` (the same rule
/// `@cache maxAge` uses) and `nosniff` a `Bool`.
fn validate_security_policy(
    service: &ServiceDecl,
    policy: &SecurityPolicy,
    errors: &mut Vec<CompileError>,
) {
    // Security headers are a browser-facing HTTP concern; they are meaningless on
    // any other protocol (mirrors the `cors_not_http` gate).
    if !matches!(service.protocol, ServiceProtocol::Http) {
        errors.push(
            CompileError::new(
                "bynk.http.security_not_http",
                policy.span,
                "a `security { }` policy is only valid on a `from http` service",
            )
            .with_note(
                "security response headers govern the browser-facing HTTP surface, \
                 which only a `from http` service has",
            ),
        );
        return;
    }

    // Field names are a closed set; flag anything else (the parser accepts any
    // name, per the CORS / annotation precedent).
    for field in &policy.fields {
        if !matches!(field.name.name.as_str(), "hsts" | "nosniff") {
            errors.push(
                CompileError::new(
                    "bynk.http.security_unknown_field",
                    field.name.span,
                    format!("unknown `security` field `{}`", field.name.name),
                )
                .with_note("known fields are `hsts` and `nosniff`"),
            );
        }
    }

    // `hsts`, when present, is a *positive* `Duration` literal — HSTS with a
    // zero/negative `max-age` is nonsensical (0 would actively *clear* the pin).
    if let Some(expr) = policy.field("hsts")
        && !matches!(&expr.kind, ExprKind::DurationLit { millis, .. } if *millis > 0)
    {
        errors.push(CompileError::new(
            "bynk.http.security_invalid_field",
            expr.span,
            "`security` `hsts` must be a positive `Duration` literal (e.g. `180.days`)",
        ));
    }

    // `nosniff`, when present, is a boolean literal.
    if let Some(expr) = policy.field("nosniff")
        && !matches!(expr.kind, ExprKind::BoolLit(_))
    {
        errors.push(CompileError::new(
            "bynk.http.security_invalid_field",
            expr.span,
            "`security` `nosniff` must be `true` or `false`",
        ));
    }
}

/// v0.142 (ADR 0165): validate a service's `limits { }` policy. A request-body
/// ceiling is wire behaviour of the HTTP surface, so the section is only legal on
/// a `from http` service; the field vocabulary is the closed set `maxBody`; and
/// `maxBody` is a *positive* `Int` byte count (there is no `Size` literal yet — a
/// `1.mb`-style literal is a named follow-on, so v1 takes an `Int`).
fn validate_limits_policy(
    service: &ServiceDecl,
    policy: &LimitsPolicy,
    errors: &mut Vec<CompileError>,
) {
    // A request-body ceiling is an HTTP-surface concern; it is meaningless on any
    // other protocol (mirrors the `cors_not_http` / `security_not_http` gate).
    if !matches!(service.protocol, ServiceProtocol::Http) {
        errors.push(
            CompileError::new(
                "bynk.http.limits_not_http",
                policy.span,
                "a `limits { }` policy is only valid on a `from http` service",
            )
            .with_note(
                "a request-body size ceiling governs the HTTP surface, \
                 which only a `from http` service has",
            ),
        );
        return;
    }

    // Field names are a closed set; flag anything else (the parser accepts any
    // name, per the CORS / security / annotation precedent).
    for field in &policy.fields {
        if field.name.name != "maxBody" {
            errors.push(
                CompileError::new(
                    "bynk.http.limits_unknown_field",
                    field.name.span,
                    format!("unknown `limits` field `{}`", field.name.name),
                )
                .with_note("the only field is `maxBody`"),
            );
        }
    }

    // `maxBody`, when present, is a *positive* `Int` literal — a byte count. Zero
    // or a negative ceiling is nonsensical (it would reject every request). There
    // is no byte `Size` literal yet, so v1 takes a plain `Int` (ADR 0165
    // DECISION C).
    if let Some(expr) = policy.field("maxBody")
        && !matches!(&expr.kind, ExprKind::IntLit { value: n, .. } if *n > 0)
    {
        errors.push(CompileError::new(
            "bynk.http.limits_invalid_field",
            expr.span,
            "`limits` `maxBody` must be a positive `Int` literal — a byte count (e.g. `1_048_576`)",
        ));
    }
}

/// #2341 (decision C): `on rejected(why: BoundaryRejection) -> HttpResult[()]`
/// (or `Effect[HttpResult[()]]`) renders the router's own rejections. At most
/// one per service, one `BoundaryRejection` parameter, no `by` (it runs before
/// any actor is known) and no `given` (it renders a body, nothing more).
fn check_rejected_handlers(table: &UnitTable, errors: &mut Vec<CompileError>) {
    for service in table.services.values() {
        let mut first: Option<Span> = None;
        for handler in &service.handlers {
            if handler.kind != HandlerKind::Rejected {
                continue;
            }
            if let Some(prev) = first {
                errors.push(
                    CompileError::new(
                        "bynk.http.duplicate_rejected",
                        handler.span,
                        format!(
                            "service `{}` already declares `on rejected`",
                            service.name.name
                        ),
                    )
                    .with_label(prev, "first declared here"),
                );
                continue;
            }
            first = Some(handler.span);
            let signature = |span: Span, msg: &str| {
                CompileError::new("bynk.http.rejected_signature", span, msg.to_string()).with_note(
                    "write `on rejected(why: BoundaryRejection) -> Effect[HttpResult[()]] { … }`",
                )
            };
            match handler.params.as_slice() {
                [p] if matches!(p.type_ref, TypeRef::BoundaryRejection(_)) => {}
                [p] => errors.push(signature(
                    p.span,
                    "`on rejected` takes the rejection, typed `BoundaryRejection`",
                )),
                _ => errors.push(signature(
                    handler.span,
                    "`on rejected` takes exactly one parameter, the `BoundaryRejection`",
                )),
            }
            if let Some(by) = &handler.by_clause {
                errors.push(signature(
                    by.span,
                    "`on rejected` has no `by`: it runs before any actor is known",
                ));
            }
            if let Some(g) = handler.given.first() {
                errors.push(signature(
                    g.span,
                    "`on rejected` has no `given`: it renders a response body and uses no capability",
                ));
            }
            let inner = match &handler.return_type {
                TypeRef::Effect(inner, _) => inner.as_ref(),
                other => other,
            };
            let unit_result = matches!(inner, TypeRef::HttpResult(t, _) if matches!(t.as_ref(), TypeRef::Unit(_)));
            if !unit_result {
                errors.push(signature(
                    handler.return_type.span(),
                    "`on rejected` returns `HttpResult[()]` (or `Effect[HttpResult[()]]`)",
                ));
            }
        }
    }
}

/// Validate an `on http METHOD "path"` handler (v0.9 §4.1):
///
/// - Path must start with `/`, must not be `/_bynk/...` (reserved).
/// - Every `:name` segment binds to a handler parameter of the same name.
/// - Every parameter is either a path parameter or named `body`.
/// - Path parameter types are constructible from `String` (`String`, refined
///   `String`, or opaque `String`).
/// - GET / DELETE handlers may not have a `body` parameter.
/// - The handler return type must be `Effect[HttpResult[T]]`.
fn validate_http_handler(
    handler: &Handler,
    method: HttpMethod,
    path: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    if !path.starts_with('/') {
        errors.push(CompileError::new(
            "bynk.http.invalid_path",
            handler.span,
            format!("HTTP path `{path}` must start with `/`"),
        ));
    }
    if path.starts_with("/_bynk/") || path == "/_bynk" {
        errors.push(
            CompileError::new(
                "bynk.http.reserved_prefix",
                handler.span,
                format!("HTTP path `{path}` uses the reserved `/_bynk/` prefix",),
            )
            .with_note("paths under `/_bynk/` are reserved for internal Bynk dispatch"),
        );
    }
    // #1883, slice 2: `/_assets/` is where content-hashed static assets are
    // served, before the Worker runs, so a handler there could be shadowed by
    // any asset; the prefix is reserved outright.
    let assets_dir = crate::assets::SERVED_PREFIX.trim_end_matches('/');
    if path == assets_dir || path.starts_with(crate::assets::SERVED_PREFIX) {
        errors.push(
            CompileError::new(
                "bynk.http.reserved_prefix",
                handler.span,
                format!("HTTP path `{path}` uses the reserved `{}` prefix", crate::assets::SERVED_PREFIX),
            )
            .with_note("paths under `/_assets/` are reserved for static assets, which the platform serves before the Worker runs"),
        );
    }
    // Parse segments and collect path-parameter names.
    let mut path_param_names: Vec<&str> = Vec::new();
    for seg in path.split('/').filter(|s| !s.is_empty()) {
        if let Some(rest) = seg.strip_prefix(':') {
            if rest.is_empty() {
                errors.push(CompileError::new(
                    "bynk.http.invalid_path",
                    handler.span,
                    format!("HTTP path `{path}` has an empty parameter segment `:`"),
                ));
            } else {
                path_param_names.push(rest);
            }
        }
    }
    // Every :name must have a matching handler parameter.
    for name in &path_param_names {
        if !handler.params.iter().any(|p| p.name.name == *name) {
            errors.push(CompileError::new(
                "bynk.http.unbound_path_param",
                handler.span,
                format!("path parameter `:{name}` has no matching handler parameter `{name}`",),
            ));
        }
    }
    // Every handler parameter must be a path param, `body` or `headers`.
    for p in &handler.params {
        let is_path = path_param_names.iter().any(|n| n == &p.name.name.as_str());
        let is_body = p.name.name == "body";
        // #2341 (decision A): `headers` is the request's headers, and must be
        // typed `RequestHeaders`.
        let is_headers = !is_path && p.name.name == "headers";
        if is_headers && !matches!(p.type_ref, TypeRef::RequestHeaders(_)) {
            errors.push(
                CompileError::new(
                    "bynk.http.headers_wrong_type",
                    p.type_ref.span(),
                    format!(
                        "an HTTP handler's `headers` parameter must be typed `RequestHeaders` (got `{}`)",
                        ts_type_ref_display(&p.type_ref),
                    ),
                )
                .with_note("`headers: RequestHeaders` gives the handler the request's headers"),
            );
        }
        if !is_path && !is_body && !is_headers {
            errors.push(
                CompileError::new(
                    "bynk.http.extra_param",
                    p.span,
                    format!(
                        "handler parameter `{}` is not a path parameter and is not named `body` or `headers`",
                        p.name.name
                    ),
                )
                .with_note(
                    "HTTP handler parameters must match a `:name` path segment, or be named `body` or `headers`",
                ),
            );
        }
        // Path params must be constructible from String.
        if is_path && !is_string_constructible(&p.type_ref, types) {
            errors.push(
                CompileError::new(
                    "bynk.http.path_param_not_stringy",
                    p.type_ref.span(),
                    format!(
                        "path parameter `{}` must have a type constructible from `String` (got `{}`)",
                        p.name.name,
                        ts_type_ref_display(&p.type_ref),
                    ),
                )
                .with_note(
                    "use `String`, a refined `String`, or an opaque type whose base is `String`",
                ),
            );
        }
        if is_body && method.forbids_body() {
            errors.push(
                CompileError::new(
                    "bynk.http.body_on_get_or_delete",
                    p.span,
                    format!(
                        "`on http {}` handlers may not declare a `body` parameter",
                        method.as_str()
                    ),
                )
                .with_note("GET and DELETE requests conventionally carry no body in Bynk v0.9"),
            );
        }
    }
    // Validate return type shape.
    let return_ok = match &handler.return_type {
        TypeRef::Effect(inner, _) => matches!(inner.as_ref(), TypeRef::HttpResult(_, _)),
        _ => false,
    };
    if !return_ok {
        errors.push(CompileError::new(
            "bynk.http.return_not_effect_http_result",
            handler.return_type.span(),
            format!(
                "`on http` handler must return `Effect[HttpResult[T]]`, but got `{}`",
                ts_type_ref_display(&handler.return_type),
            ),
        ));
    }
}

/// Validate a handler's handler-position annotations (v0.140, ADR 0163). The one
/// handler annotation is `@cache(maxAge: <Duration>, scope: public|private)`,
/// legal solely on an `on http GET` handler. This runs for *every* handler —
/// services and agents — so a misplaced `@cache` (a non-GET route, another
/// protocol, or an agent handler) is caught wherever it is written, and an
/// unknown annotation name is flagged rather than silently ignored. The
/// automatic conditional `ETag`/`304` half carries no author surface, so `@cache`
/// is the only annotation validated here.
fn validate_handler_annotations(
    handler: &Handler,
    service_cap: Option<i64>,
    errors: &mut Vec<CompileError>,
) {
    let is_get = matches!(
        handler.kind,
        HandlerKind::Http {
            method: HttpMethod::Get,
            ..
        }
    );
    // v0.142 (ADR 0165): `@limit` is the inverse of `@cache` — it caps a request
    // body, so it is valid only on a body-taking route (POST/PUT/PATCH); a GET or
    // DELETE (and any non-HTTP handler) has no body to limit.
    let is_body_method = matches!(
        handler.kind,
        HandlerKind::Http {
            method: HttpMethod::Post | HttpMethod::Put | HttpMethod::Patch,
            ..
        }
    );
    let mut seen_cache = false;
    let mut seen_limit = false;
    let mut seen_raw = false;
    for ann in &handler.annotations {
        match ann.name.name.as_str() {
            "cache" => {
                if seen_cache {
                    errors.push(CompileError::new(
                        "bynk.http.cache_duplicate",
                        ann.span,
                        "a handler carries at most one `@cache` annotation",
                    ));
                    continue;
                }
                seen_cache = true;
                if !is_get {
                    errors.push(
                        CompileError::new(
                            "bynk.http.cache_on_non_get",
                            ann.span,
                            "`@cache` is only valid on an `on http GET` handler",
                        )
                        .with_note(
                            "conditional caching applies to safe, idempotent reads — a `GET` route",
                        ),
                    );
                    continue;
                }
                validate_cache_args(ann, errors);
            }
            "limit" => {
                if seen_limit {
                    errors.push(CompileError::new(
                        "bynk.http.limit_duplicate",
                        ann.span,
                        "a handler carries at most one `@limit` annotation",
                    ));
                    continue;
                }
                seen_limit = true;
                if !is_body_method {
                    errors.push(
                        CompileError::new(
                            "bynk.http.limit_on_bodyless",
                            ann.span,
                            "`@limit` is only valid on a body-taking `on http` route (POST/PUT/PATCH)",
                        )
                        .with_note(
                            "a request-body size cap applies to routes that read a body — a GET or DELETE has none",
                        ),
                    );
                    continue;
                }
                validate_limit_args(ann, errors);
            }
            "raw" => {
                if seen_raw {
                    errors.push(CompileError::new(
                        "bynk.http.raw_duplicate",
                        ann.span,
                        "a handler carries at most one `@raw` annotation",
                    ));
                    continue;
                }
                seen_raw = true;
                let body = handler.params.iter().find(|p| p.name.name == "body");
                let Some(body) = body.filter(|_| is_body_method) else {
                    errors.push(
                        CompileError::new(
                            "bynk.http.raw_on_bodyless",
                            ann.span,
                            "`@raw` is only valid on a POST/PUT/PATCH `on http` route with a `body` parameter",
                        )
                        .with_note(
                            "a raw request body is the `body: Bytes` parameter of a body-taking route",
                        ),
                    );
                    continue;
                };
                validate_raw_args(ann, errors);
                if !matches!(body.type_ref, TypeRef::Base(BaseType::Bytes, _)) {
                    errors.push(
                        CompileError::new(
                            "bynk.http.raw_body_not_bytes",
                            body.type_ref.span(),
                            format!(
                                "the `body` of a `@raw` handler must be `Bytes`, but it is `{}`",
                                ts_type_ref_display(&body.type_ref),
                            ),
                        )
                        .with_note("a raw body is handed to the handler as its octets, undecoded"),
                    );
                }
                let route_cap = handler.limit_max_body();
                if route_cap.or(service_cap).is_none() {
                    errors.push(
                        CompileError::new(
                            "bynk.http.raw_body_unbounded",
                            ann.span,
                            "a `@raw` route needs a request-body cap",
                        )
                        .with_note(
                            "add `@limit(maxBody: …)` to the handler or `limits { maxBody = … }` to the service; an unbounded binary upload is the denial of service ADR 0165 exists to stop",
                        ),
                    );
                }
            }
            other => {
                errors.push(
                    CompileError::new(
                        "bynk.http.unknown_handler_annotation",
                        ann.name.span,
                        format!(
                            "unknown handler annotation `@{other}` — the handler annotations are `@cache`, `@limit` and `@raw`"
                        ),
                    )
                    .with_note("handler annotations are a closed set (ADR 0163, ADR 0165)"),
                );
            }
        }
    }
}

/// Validate `@cache`'s arguments on a GET handler (v0.140, ADR 0163): a required
/// `maxAge:` positive `Duration` literal (the freshness window — the one thing the
/// compiler cannot derive) and an optional `scope:` of `public`/`private`
/// (defaulting to `private` at emit time). Any other argument — a stray label or a
/// positional value — is a diagnostic; the vocabulary is closed.
fn validate_cache_args(ann: &Annotation, errors: &mut Vec<CompileError>) {
    let mut max_age: Option<&AnnotationArg> = None;
    let mut scope: Option<&AnnotationArg> = None;
    let mut immutable: Option<&AnnotationArg> = None;
    for arg in &ann.args {
        match arg.label.as_ref().map(|l| l.name.as_str()) {
            Some("maxAge") => max_age = Some(arg),
            Some("scope") => scope = Some(arg),
            Some("immutable") => immutable = Some(arg),
            _ => {
                errors.push(
                    CompileError::new(
                        "bynk.http.cache_unknown_arg",
                        arg.span,
                        "`@cache` accepts only the `maxAge:`, `scope:` and `immutable:` arguments",
                    )
                    .with_note("write `@cache(maxAge: 5.minutes, scope: private)`"),
                );
            }
        }
    }
    // `maxAge` is required and must be a *positive* `Duration` literal — the same
    // positive-duration rule the `@ttl` store annotation uses. It must also
    // resolve to a *whole* number of seconds: `Cache-Control: max-age` has no
    // finer granularity, so `lower_route_cache_ir` (`bynk-emit/src/ir/lower.rs`)
    // divides by 1000 to get there — a value with any fractional-second
    // remainder (`500.milliseconds`, or `1500.milliseconds`, which resolves to a
    // real but wrong `max-age=1`) would previously type-check cleanly and then
    // silently drop the remainder, with no diagnostic anywhere in the pipeline
    // (#1230). `>= 1000` alone would only catch the *total-loss* case
    // (`max-age=0`) and let the partial-loss case through, which is the same
    // silent-truncation defect at a smaller magnitude, not a different one —
    // review of #1231 is what found the guard needed to be exact-division, not
    // a floor.
    match max_age.map(|a| &a.value.kind) {
        Some(ExprKind::DurationLit { millis, .. }) if *millis > 0 && *millis % 1000 == 0 => {}
        Some(ExprKind::DurationLit { millis, .. }) if *millis > 0 => {
            errors.push(
                CompileError::new(
                    "bynk.http.cache_max_age_fractional_seconds",
                    max_age.unwrap().span,
                    "`@cache` `maxAge` must be a whole number of seconds",
                )
                .with_note(
                    "`Cache-Control: max-age` is whole seconds — a value with a fractional \
                     second would silently drop the remainder rather than round or reject, \
                     so it is not honoured exactly",
                ),
            );
        }
        Some(_) => {
            errors.push(CompileError::new(
                "bynk.http.cache_bad_max_age",
                max_age.unwrap().span,
                "`@cache` `maxAge` must be a positive `Duration` literal (e.g. `5.minutes`)",
            ));
        }
        None => {
            errors.push(
                CompileError::new(
                    "bynk.http.cache_bad_max_age",
                    ann.span,
                    "`@cache` requires a `maxAge:` argument — the freshness window",
                )
                .with_note(
                    "the `ETag` revalidation is automatic; only the freshness window is declared",
                ),
            );
        }
    }
    // `scope`, when present, is the bare identifier `public` or `private`.
    if let Some(scope) = scope {
        let ok = matches!(
            &scope.value.kind,
            ExprKind::Ident(id) if id.name == "public" || id.name == "private"
        );
        if !ok {
            errors.push(CompileError::new(
                "bynk.http.cache_bad_scope",
                scope.span,
                "`@cache` `scope` must be `public` or `private`",
            ));
        }
    }
    // `immutable`, when present, is a `Bool` literal (caching raw responses): the
    // author's promise that the representation never changes, emitted as the
    // RFC 8246 `immutable` directive when `true`.
    if let Some(immutable) = immutable
        && !matches!(immutable.value.kind, ExprKind::BoolLit(_))
    {
        errors.push(
            CompileError::new(
                "bynk.http.cache_bad_immutable",
                immutable.span,
                "`@cache` `immutable` must be `true` or `false`",
            )
            .with_note(
                "`immutable: true` promises the response at this URL never changes, so caches \
                 do not revalidate it within `maxAge` — true of content-addressed paths",
            ),
        );
    }
}

/// Validate `@limit`'s arguments on a body-taking route (v0.142, ADR 0165): a
/// required `maxBody:` positive `Int` literal — a byte count, the one ceiling only
/// the author knows. Any other argument — a stray label or a positional value — is
/// a diagnostic; the vocabulary is closed. A route `@limit` overrides the service
/// `limits { }` default at emit time. There is no `Size` literal yet, so the byte
/// count is a plain `Int` (DECISION C).
fn validate_limit_args(ann: &Annotation, errors: &mut Vec<CompileError>) {
    let mut max_body: Option<&AnnotationArg> = None;
    for arg in &ann.args {
        match arg.label.as_ref().map(|l| l.name.as_str()) {
            Some("maxBody") => max_body = Some(arg),
            _ => {
                errors.push(
                    CompileError::new(
                        "bynk.http.limit_unknown_arg",
                        arg.span,
                        "`@limit` accepts only the `maxBody:` argument",
                    )
                    .with_note("write `@limit(maxBody: 26_214_400)`"),
                );
            }
        }
    }
    // `maxBody` is required and must be a *positive* `Int` literal — a byte count.
    match max_body.map(|a| &a.value.kind) {
        Some(ExprKind::IntLit { value: n, .. }) if *n > 0 => {}
        Some(_) => {
            errors.push(CompileError::new(
                "bynk.http.limit_bad_max_body",
                max_body.unwrap().span,
                "`@limit` `maxBody` must be a positive `Int` literal — a byte count (e.g. `26_214_400`)",
            ));
        }
        None => {
            errors.push(
                CompileError::new(
                    "bynk.http.limit_bad_max_body",
                    ann.span,
                    "`@limit` requires a `maxBody:` argument — the byte ceiling",
                )
                .with_note(
                    "the ceiling is a policy the compiler cannot derive; only the author knows it",
                ),
            );
        }
    }
}

/// Validate `@raw`'s arguments on a body-taking route (ADR 0143's request-side
/// counterpart): a required `contentType:` string literal that is a media type
/// (`type/subtype`) or `"*/*"`. The vocabulary is closed.
fn validate_raw_args(ann: &Annotation, errors: &mut Vec<CompileError>) {
    let mut content_type: Option<&AnnotationArg> = None;
    for arg in &ann.args {
        match arg.label.as_ref().map(|l| l.name.as_str()) {
            Some("contentType") => content_type = Some(arg),
            _ => {
                errors.push(
                    CompileError::new(
                        "bynk.http.raw_unknown_arg",
                        arg.span,
                        "`@raw` accepts only the `contentType:` argument",
                    )
                    .with_note("write `@raw(contentType: \"application/octet-stream\")`"),
                );
            }
        }
    }
    match content_type.map(|a| &a.value.kind) {
        Some(ExprKind::StrLit(s)) if bynk_syntax::ast::is_media_type(s) => {}
        Some(_) => {
            errors.push(CompileError::new(
                "bynk.http.raw_bad_content_type",
                content_type.unwrap().span,
                "`@raw` `contentType` must be a media type string literal — `type/subtype`, or `\"*/*\"` to accept any",
            ));
        }
        None => {
            errors.push(
                CompileError::new(
                    "bynk.http.raw_bad_content_type",
                    ann.span,
                    "`@raw` requires a `contentType:` argument — the media type the route accepts",
                )
                .with_note("use `\"*/*\"` to accept any content type"),
            );
        }
    }
}

/// Validate an `on cron "expr" (at: Int?) -> Effect[Result[(), E]]` handler
/// (v0.10a §4.1): at most one `Int` parameter (the scheduled time, Unix epoch
/// milliseconds), a structurally well-formed schedule, and the unit-Result
/// return shape. The service-only rule is enforced earlier, in the parser
/// (`bynk.parse.handler_in_agent`).
fn validate_cron_handler(handler: &Handler, expr: &str, errors: &mut Vec<CompileError>) {
    // A cron handler takes at most one parameter — the scheduled time, typed
    // `Int` (epoch milliseconds). A scheduled trigger has no other payload.
    if handler.params.len() > 1 {
        errors.push(
            CompileError::new(
                "bynk.cron.bad_params",
                handler.params[1].span,
                "`on cron` handlers take at most one parameter (the scheduled time)",
            )
            .with_note("a scheduled trigger's only input is the time it fired"),
        );
    } else if let Some(p) = handler.params.first()
        && !matches!(p.type_ref, TypeRef::Base(BaseType::Int, _))
    {
        errors.push(
            CompileError::new(
                "bynk.cron.bad_params",
                p.type_ref.span(),
                format!(
                    "an `on cron` parameter must be `Int` (the scheduled time in epoch milliseconds), got `{}`",
                    ts_type_ref_display(&p.type_ref),
                ),
            )
            .with_note("wrap it in your own time type inside the body if you want stronger typing"),
        );
    }
    // The schedule must be five whitespace-separated fields (light structural
    // check; per-field validation is deferred — v0.10 §4.1, [DECISION 4]).
    let fields = expr.split_whitespace().count();
    if fields != 5 {
        errors.push(
            CompileError::new(
                "bynk.cron.invalid_schedule",
                handler.span,
                format!(
                    "cron expression `{expr}` must have exactly five whitespace-separated fields (got {fields})",
                ),
            )
            .with_note("the fields are: minute hour day-of-month month day-of-week"),
        );
    }
    // The return type must be `Effect[Result[(), E]]`.
    let return_ok = match &handler.return_type {
        TypeRef::Effect(inner, _) => match inner.as_ref() {
            TypeRef::Result(ok, _err, _) => matches!(ok.as_ref(), TypeRef::Unit(_)),
            _ => false,
        },
        _ => false,
    };
    if !return_ok {
        errors.push(CompileError::new(
            "bynk.cron.return_not_effect_result",
            handler.return_type.span(),
            format!(
                "`on cron` handler must return `Effect[Result[(), E]]`, but got `{}`",
                ts_type_ref_display(&handler.return_type),
            ),
        ));
    }
}

/// Validate an `on message(message: T) -> Effect[QueueResult]` handler of a
/// `from queue("name")` service (v0.10b §4.2, v0.44): a queue name Cloudflare
/// accepts (`is_queue_name`), exactly one parameter (the message, any
/// wire-deserialisable type), and the `Effect[QueueResult]` return — the verdict
/// sum, where `Ack` acknowledges the message and `Retry(...)` has it redelivered. The service-only rule is
/// enforced earlier, in the parser (`bynk.parse.handler_in_agent`).
fn validate_queue_handler(handler: &Handler, name: &str, errors: &mut Vec<CompileError>) {
    if name.is_empty() {
        errors.push(CompileError::new(
            "bynk.queue.invalid_name",
            handler.span,
            "`on queue` requires a non-empty queue name",
        ));
    } else if !crate::project_model::is_queue_name(name) {
        errors.push(
            CompileError::new(
                "bynk.queue.invalid_name",
                handler.span,
                format!("`{name}` is not a queue name Cloudflare accepts"),
            )
            .with_note(
                "a queue name is 1 to 63 lowercase letters, digits or `-`, and starts and ends with a letter or digit",
            ),
        );
    }
    // Exactly one parameter — the message. (Conventionally named `message`.)
    if handler.params.len() != 1 {
        errors.push(
            CompileError::new(
                "bynk.queue.bad_params",
                handler.span,
                format!(
                    "`on message` handlers take exactly one parameter (the message), got {}",
                    handler.params.len(),
                ),
            )
            .with_note("a queue consumer processes one message per invocation"),
        );
    }
    // v0.44: the return type must be `Effect[QueueResult]` (the verdict sum).
    let return_ok = match &handler.return_type {
        TypeRef::Effect(inner, _) => matches!(inner.as_ref(), TypeRef::QueueResult(_)),
        _ => false,
    };
    if !return_ok {
        errors.push(CompileError::new(
            "bynk.queue.return_not_queue_result",
            handler.return_type.span(),
            format!(
                "`on message` handler must return `Effect[QueueResult]`, but got `{}`",
                ts_type_ref_display(&handler.return_type),
            ),
        ));
    }
}

/// True when `r` resolves to `String`, a refined-base `String`, or an
/// opaque-base `String`. v0.9 path parameter requirement.
fn is_string_constructible(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    match r {
        TypeRef::Base(BaseType::String, _) => true,
        TypeRef::Named(id) => match types.get(&id.name).map(|t| &t.body) {
            Some(TypeBody::Refined { base, .. }) => *base == BaseType::String,
            Some(TypeBody::Opaque { base, .. }) => *base == BaseType::String,
            _ => false,
        },
        _ => false,
    }
}

/// v0.20a: function types are confined to non-boundary positions — fn/lambda
/// parameters, returns, and locals. Walk a type reference and reject any
/// function type found in a position that would serialise, persist, or cross
/// a boundary (`bynk.types.function_at_boundary`).
/// v0.102 (§2.9): true if a type *is or wraps* a held resource (`Connection`),
/// looking through `Option`/`Effect` — the shapes a held value legitimately
/// takes: an `Option[Connection]` cell value, an `Effect[Connection]` capability
/// return, a bare `Connection` handler parameter.
pub fn type_ref_is_held(r: &TypeRef) -> bool {
    match r {
        TypeRef::Connection(..) => true,
        TypeRef::Option(inner, _) | TypeRef::Effect(inner, _) => type_ref_is_held(inner),
        _ => false,
    }
}

/// v0.102 (§2.9.3): validate one agent `store` field's value types, applying the
/// held-resource storage rules. Held values are admitted in
/// `Cell[Option[Connection]]` / `Map[K, Connection]` (an exception to the
/// serialisable-value rule — hibernation preserves them, not JSON), and rejected
/// in `Set`/`Log`/`Cache`. Non-held value types fall through to the ordinary
/// boundary check.
pub fn validate_store_field_value_types(
    f: &StoreField,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    let head = f.kind.head.name.as_str();
    let reject_held_storage = |span: Span, errors: &mut Vec<CompileError>| {
        errors.push(
            CompileError::new(
                "bynk.held.unsupported_storage",
                span,
                format!(
                    "a held value cannot be stored in a `{head}` — held resources may only live in `Cell[Option[Connection]]` or `Map[K, Connection]` (§2.9.3)"
                ),
            )
            .with_note(
                "`Set` needs value-equality, and `Log`/`Cache` would retain or evict a held resource without disposing it",
            ),
        );
    };
    match head {
        // The value position(s) where a held resource is admitted.
        "Cell" => match f.kind.args.first() {
            Some(v) if type_ref_is_held(v) => {} // admitted
            Some(v) => reject_fn_types(v, "an agent store field", types, errors),
            None => {}
        },
        "Map" => match f.kind.args.as_slice() {
            [k, v] => {
                reject_fn_types(k, "an agent store field", types, errors); // key
                if !type_ref_is_held(v) {
                    reject_fn_types(v, "an agent store field", types, errors);
                }
            }
            args => {
                for arg in args {
                    reject_fn_types(arg, "an agent store field", types, errors);
                }
            }
        },
        // Kinds that reject held values outright.
        "Set" | "Cache" | "Log" => {
            for arg in &f.kind.args {
                if type_ref_is_held(arg) {
                    reject_held_storage(arg.span(), errors);
                } else {
                    reject_fn_types(arg, "an agent store field", types, errors);
                }
            }
        }
        _ => {
            for arg in &f.kind.args {
                reject_fn_types(arg, "an agent store field", types, errors);
            }
        }
    }
}

/// The `what` labels of the boundary positions whose rule differs by position
/// (a `Stream` or a `bynk.crypto` key passes through some). [`reject_fn_types`]
/// decides by them, so every caller names its position through these rather
/// than a literal, and a reworded label can't silently change a check.
pub const CAPABILITY_SIGNATURE: &str = "a capability operation signature";
pub const RECORD_FIELD: &str = "a record field";
pub const SUM_PAYLOAD: &str = "a sum-variant payload";

pub fn reject_fn_types(
    r: &TypeRef,
    what: &str,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    match r {
        TypeRef::Fn(_, _, span) => {
            errors.push(
                CompileError::new(
                    "bynk.types.function_at_boundary",
                    *span,
                    format!(
                        "a function type cannot appear in {what} — functions cannot serialise or cross a boundary"
                    ),
                )
                .with_note(
                    "function types are confined to fn/lambda parameters, returns, and locals",
                ),
            );
        }
        // v0.91 (ADR 0115 D2): a `Query[T]` is non-storable and non-boundary —
        // built, passed within an agent, and executed, never persisted or sent.
        TypeRef::Query(_, span) => {
            errors.push(
                CompileError::new(
                    "bynk.types.query_at_boundary",
                    *span,
                    format!(
                        "a `Query` type cannot appear in {what} — a query is built and executed in place, never persisted or sent across a boundary"
                    ),
                )
                .with_note(
                    "terminate the query (`.collect`/`.first`/…) and store or send the result instead",
                ),
            );
        }
        // #2380 (L5): `TokenClaims` are an `Oidc` actor's verified token, read
        // through its binder where the request arrives — never stored or sent.
        TypeRef::TokenClaims(span) => {
            errors.push(
                CompileError::new(
                    "bynk.types.claims_at_boundary",
                    *span,
                    format!(
                        "a `TokenClaims` type cannot appear in {what} — it is a verified token's claims, read in the handler that received the token, never persisted or sent"
                    ),
                )
                .with_note(
                    "read the claim you need (`who.claims.get(\"repository\")`) and store or send that value instead",
                ),
            );
        }
        // #2341 (decision A): `RequestHeaders` is an HTTP handler's view of
        // the request it is answering — read where it arrives, never stored
        // or sent. Its one legal position, a handler's `headers` parameter,
        // is not checked here.
        TypeRef::RequestHeaders(span) => {
            errors.push(
                CompileError::new(
                    "bynk.types.headers_at_boundary",
                    *span,
                    format!(
                        "a `RequestHeaders` type cannot appear in {what} — it is the request's headers, read in the handler that receives them, never persisted or sent"
                    ),
                )
                .with_note(
                    "read the header you need (`headers.get(\"…\")`) and store or send that value instead",
                ),
            );
        }
        // #2341 (decision C): a `BoundaryRejection` is the router's account of
        // a request it refused, read by `on rejected` and never kept.
        TypeRef::BoundaryRejection(span) => {
            errors.push(
                CompileError::new(
                    "bynk.types.rejection_at_boundary",
                    *span,
                    format!(
                        "a `BoundaryRejection` type cannot appear in {what} — it describes a request the router refused, read by `on rejected`, never persisted or sent"
                    ),
                )
                .with_note(
                    "match on the rejection in `on rejected` and store or send what you need from it instead",
                ),
            );
        }
        // K-streams (#2623): a capability operation's provider runs in the
        // caller's isolate, so a stream passes through its signature as the
        // same iterable; and a record or sum may carry one, becoming
        // non-boundary itself (the `Named` arm below finds it at each use).
        TypeRef::Stream(..) if stream_passes(what) => {}
        // v0.100: a `Stream[T]` is non-storable and non-boundary — a live
        // value-over-time source, built and consumed in place, never persisted
        // or sent.
        TypeRef::Stream(_, span) => {
            errors.push(
                CompileError::new(
                    "bynk.types.stream_at_boundary",
                    *span,
                    format!(
                        "a `Stream` type cannot appear in {what} — a stream is a live value-over-time source, never persisted or sent across a boundary"
                    ),
                )
                .with_note(
                    "drain the stream (`.collect()`) and store or send the resulting `List` instead",
                ),
            );
        }
        // v0.102: a `Connection[F]` (a held resource) is non-boundary — built
        // and disposed in place under the linearity discipline, never persisted
        // or sent across a boundary.
        TypeRef::Connection(_, span) => {
            errors.push(
                CompileError::new(
                    "bynk.types.held_at_boundary",
                    *span,
                    format!(
                        "a `Connection` type cannot appear in {what} — a held resource is built and disposed in place, never persisted or sent across a boundary"
                    ),
                )
                .with_note(
                    "hold the connection in agent state (`Cell[Option[Connection]]` / `Map[K, Connection]`) instead of crossing a boundary with it",
                ),
            );
        }
        // v0.20b: the boundary rule looks through collections — a
        // `List[Int -> Int]` field is still `function_at_boundary`.
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            reject_fn_types(a, what, types, errors);
            reject_fn_types(b, what, types, errors);
        }
        TypeRef::Option(a, _)
        | TypeRef::Effect(a, _)
        | TypeRef::HttpResult(a, _)
        | TypeRef::List(a, _)
        | TypeRef::Set(a, _) => reject_fn_types(a, what, types, errors),
        // v0.119: a `History[Agent]` reaching a declared position is already
        // reported by the resolver (`bynk.history.outside_property`); nothing to
        // add here.
        TypeRef::History(_, _) => {}
        // v0.174 (#592): a generic record instantiation is boundary-serialisable
        // through its monomorphised codec (`serialise_Paginated_User`) — so the
        // application itself is admitted, and the rule instead looks *through* it
        // into the type arguments. A non-serialisable argument (a function, a
        // `Query`, …) is rejected there, with the argument's own boundary error.
        // (ADR 0183 Decision C's blanket `generic_record_at_boundary` rejection
        // was the previous behaviour.) A *recursive* generic record — one that
        // transitively contains itself, through any wrapper or generic argument —
        // has no finite set of monomorphised codecs, so it is still rejected
        // here (the resolver's `recursive_record_field` guard only catches a
        // direct self-edge, not recursion through an `Option`/`List` wrapper).
        TypeRef::App { name, args, span } => {
            if generic_record_is_recursive(&name.name, types) {
                errors.push(
                    CompileError::new(
                        "bynk.generics.recursive_generic_at_boundary",
                        *span,
                        format!(
                            "recursive generic record `{}` cannot appear in {what} — it has no finite monomorphised codec",
                            name.name
                        ),
                    )
                    .with_note(
                        "a generic record that transitively contains itself is not yet \
                         boundary-serialisable; use a concrete (non-generic) recursive type, \
                         or break the cycle",
                    ),
                );
            }
            for a in args {
                reject_fn_types(a, what, types, errors);
            }
            // K-streams (#2623): a generic record whose own fields carry a
            // stream (`Wrap[T] = { s: Stream[T] }`) is found through its
            // declaration; a stream argument was reported just above.
            if !stream_passes(what)
                && let Some(path) =
                    bynk_syntax::ast::stream_path(&TypeRef::Named(name.clone()), types)
            {
                errors.push(stream_carrying_at_boundary(&name.name, &path, *span, what));
            }
        }
        // #1883, slice 1a: an `Asset` names a file the platform serves; it
        // is not data. What crosses a boundary or is stored is its `.url`.
        TypeRef::Asset(span) => {
            errors.push(
                CompileError::new(
                    "bynk.types.asset_at_boundary",
                    *span,
                    format!(
                        "an `Asset` type cannot appear in {what} — an asset names a file the platform serves, not a value to persist or send"
                    ),
                )
                .with_note("store or send the asset's `.url` (a `String`) instead"),
            );
        }
        // P4c (#2628): a `SqlStatement` restored or received from outside
        // would carry SQL that was never a literal. `D1.batch` takes one, so a
        // capability operation's signature is exempt, as for keys.
        TypeRef::Named(id)
            if what != CAPABILITY_SIGNATURE
                && crate::checker::is_sql_statement_type(&id.name, types) =>
        {
            errors.push(crate::checker::statement_at_boundary(id.span, what));
        }
        // K4: a `bynk.crypto` key holds a non-extractable platform key. It is
        // imported where it is used and lives for that call: persisting,
        // serialising or sending it would either leak the secret or be
        // meaningless, so it is non-storable and non-boundary like a `Stream`.
        // A capability operation is where a key is meant to flow (`Crypto.sign`
        // takes one), in-process, so its signature is exempt.
        TypeRef::Named(id)
            if what != CAPABILITY_SIGNATURE && is_crypto_key_type(&id.name, types) =>
        {
            errors.push(
                CompileError::new(
                    "bynk.types.key_at_boundary",
                    id.span,
                    format!(
                        "a `{}` cannot appear in {what} — a key is held non-extractable by the platform and lives for the call that imported it, never persisted, logged or sent across a boundary",
                        id.name
                    ),
                )
                .with_note(
                    "keep the key bytes in `Secrets` and import the key again where it is used (`Crypto.importSigningKey` / `Crypto.importVerifyKey`)",
                ),
            );
        }
        // K-streams (#2623): a record or sum that carries a stream, at any
        // depth, is non-boundary like the stream.
        TypeRef::Named(id) if !stream_passes(what) => {
            if let Some(path) = bynk_syntax::ast::stream_path(r, types) {
                errors.push(stream_carrying_at_boundary(&id.name, &path, id.span, what));
            }
        }
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::QueueResult(_)
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
        | TypeRef::Unit(_) => {}
    }
}

/// K-streams (#2623): the positions a `Stream` may appear in. A capability
/// operation's provider runs in the caller's isolate, so nothing in its
/// signature is serialised; a record field or sum payload makes its type
/// stream-carrying, which is then checked where the type is used.
fn stream_passes(what: &str) -> bool {
    [CAPABILITY_SIGNATURE, RECORD_FIELD, SUM_PAYLOAD].contains(&what)
}

/// K-streams (#2623): `bynk.types.stream_at_boundary` for a type that carries
/// a stream in a field, naming the field chain it is reached through.
fn stream_carrying_at_boundary(
    name: &str,
    path: &[String],
    span: Span,
    what: &str,
) -> CompileError {
    let chain = path
        .iter()
        .map(|f| format!("`{f}`"))
        .collect::<Vec<_>>()
        .join(", then ");
    CompileError::new(
        "bynk.types.stream_at_boundary",
        span,
        format!(
            "`{name}` cannot appear in {what} — it carries a `Stream`, a live value-over-time source, never persisted or sent across a boundary"
        ),
    )
    .with_note(format!(
        "the stream is reached through {chain}; drain it (`.collect()`) into a type without one before it crosses"
    ))
}

/// Whether `name`, in the project's type table, is one of `bynk.crypto`'s key
/// types ([`crate::firstparty::CRYPTO_KEY_TYPES`]). Found by the declaration's
/// span file, which is the unit's reserved first-party id, so a user type that
/// happens to share the name is not mistaken for one.
fn is_crypto_key_type(
    name: &str,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> bool {
    crate::firstparty::CRYPTO_KEY_TYPES.contains(&name)
        && types.get(name).is_some_and(|decl| {
            crate::firstparty::unit(crate::firstparty::CRYPTO_UNIT).is_some_and(|unit| {
                decl.span.file == crate::project_model::firstparty_file_id(unit)
            })
        })
}

/// #1170: `TypedCommons::actor_bindings` persistence — every case
/// `handler_actor_binding` itself distinguishes (Some vs. None, single
/// actor vs. sum, the binder-shadows-param suppression), pinned through
/// the real `check_context_declarations` entry point on a certified
/// program, not by calling `handler_actor_binding` directly.
#[cfg(test)]
mod actor_binding_persistence_tests {
    use super::*;
    use crate::checker::CheckedProgram;
    use crate::{resolver, symbols};
    use bynk_project::UnitKind;
    use bynk_syntax::ast::{ActorDecl, Commons, CommonsItem, ServiceDecl, SourceUnit};
    use bynk_syntax::{lexer, parser};

    /// Parse+resolve+check+context-check a whole `context` unit from
    /// source, stopping short of `certify` — mirrors `bynk-emit`'s own
    /// `checked_context_program` test helper (`bynk-emit/src/ir/lower.rs`)
    /// closely, but populates `services`/`actors` on the `UnitTable` too
    /// (that helper's own agent-only scope never needed them). Returns the
    /// raw `(TypedCommons, errors)` pair rather than a `CheckedProgram` so
    /// [`binder_shadowing_a_param_persists_no_binding`] can inspect
    /// `actor_bindings` even on a source that *cannot* certify (a hard
    /// `bynk.actor.binder_shadows_param` error) — every other test here
    /// wraps this in [`checked_context_program`] instead.
    fn checked_context_commons(source: &str) -> (checker::TypedCommons, Vec<CompileError>) {
        let tokens = lexer::tokenize(source).expect("lex");
        let unit = parser::parse_unit(&tokens, source).expect("parse");
        let SourceUnit::Context(ctx) = unit else {
            panic!("expected a context unit, got {unit:?}")
        };
        let commons = Commons {
            name: ctx.name,
            items: ctx.items,
            uses: ctx.uses,
            documentation: ctx.documentation,
            form: ctx.form,
            span: ctx.span,
            trivia: ctx.trivia,
            trailing_comments: ctx.trailing_comments,
        };
        let resolved = resolver::resolve(commons).expect("resolve");
        let mut typed = checker::check(resolved).expect("check");
        let services: HashMap<String, ServiceDecl> = typed
            .commons
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Service(s) => Some((s.name.name.clone(), s.clone())),
                _ => None,
            })
            .collect();
        let actors: HashMap<String, ActorDecl> = typed
            .commons
            .items
            .iter()
            .filter_map(|item| match item {
                CommonsItem::Actor(a) => Some((a.name.name.clone(), a.clone())),
                _ => None,
            })
            .collect();
        let table = symbols::UnitTable {
            kind: Some(UnitKind::Context),
            types: typed.types.clone(),
            services,
            actors,
            ..symbols::UnitTable::default()
        };
        let tys = typed.ty_intern.clone();
        let errors = check_context_declarations(
            &mut typed,
            &table,
            &resolver::CrossContextInfo::default(),
            true,
            &HashSet::new(),
            &HashMap::new(),
            &mut RefSink::new(),
            &mut HintSink::new(),
            &mut LocalsSink::new(),
            &mut RequirementSink::new(),
            &tys,
        );
        (typed, errors)
    }

    fn checked_context_program(source: &str) -> CheckedProgram {
        let (typed, errors) = checked_context_commons(source);
        checker::certify(typed, errors).expect("certify")
    }

    fn find_service<'a>(typed: &'a checker::TypedCommons, name: &str) -> &'a ServiceDecl {
        typed
            .commons
            .items
            .iter()
            .find_map(|item| match item {
                CommonsItem::Service(s) if s.name.name == name => Some(s),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no service named `{name}` in this fixture"))
    }

    #[test]
    fn single_actor_by_clause_persists_the_binder_and_sealed_identity_ty() {
        let program = checked_context_program(
            r#"
context demo

type UserId = String

actor Buyer { auth = Internal, identity = UserId }

service Api {
  on call(ping: String) -> Effect[String] by u: Buyer {
    Effect.pure(ping)
  }
}
"#,
        );
        let handler = &find_service(program.program(), "Api").handlers[0];
        let (binder, ty) = program
            .program()
            .actor_binding(handler.span)
            .unwrap_or_else(|| panic!("expected a persisted actor binding for this handler"));
        assert_eq!(binder, "u");
        let tys = &program.program().ty_intern;
        let Ty::Actor(identity_ty) = &*tys.get(*ty) else {
            panic!("expected Ty::Actor, got {:?}", tys.get(*ty))
        };
        assert_eq!(
            identity_ty.display(tys),
            "UserId",
            "the actor's own declared `identity = UserId` type, sealed"
        );
    }

    #[test]
    fn prelude_caller_actor_persists_a_string_identity_binding() {
        // `Caller` (v0.54) is a prelude actor — no local `actor` declaration
        // needed — whose identity is the calling-context id, `String`.
        let program = checked_context_program(
            r#"
context demo

service Api {
  on call(ping: String) -> Effect[String] by c: Caller {
    Effect.pure(c.identity)
  }
}
"#,
        );
        let handler = &find_service(program.program(), "Api").handlers[0];
        let (binder, ty) = program
            .program()
            .actor_binding(handler.span)
            .unwrap_or_else(|| panic!("expected a persisted actor binding for this handler"));
        assert_eq!(binder, "c");
        let string_ty = program
            .program()
            .ty_intern
            .intern(Ty::Base(bynk_syntax::ast::BaseType::String));
        let expected = program.program().ty_intern.intern(Ty::Actor(string_ty));
        assert_eq!(*ty, expected);
    }

    #[test]
    fn sum_by_clause_persists_an_actor_sum_binding() {
        // Mirrors `bynkc/tests/fixtures/positive/916_bytes_http_sum_body`:
        // an HTTP route's `by who: User | Visitor` sum, `User` a real
        // `Bearer`-scheme local actor, `Visitor` the prelude unit-identity
        // actor — a sum's own peers must carry distinguishable schemes
        // (`bynk.actor.duplicate_sum_scheme`), which two `Internal`-scheme
        // actors (the only scheme a `call` handler admits) cannot, so this
        // one case needs `from http` rather than the plain `call` protocol
        // every other test here uses.
        let program = checked_context_program(
            r#"
context demo

type UserId = String

actor User { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }

service Api from http {
  on GET("/whoami") () -> Effect[HttpResult[String]] by who: User | Visitor {
    match who {
      User(_) => Ok("user")
      Visitor => Ok("visitor")
    }
  }
}
"#,
        );
        let handler = &find_service(program.program(), "Api").handlers[0];
        let (binder, ty) = program
            .program()
            .actor_binding(handler.span)
            .unwrap_or_else(|| panic!("expected a persisted actor binding for this handler"));
        assert_eq!(binder, "who");
        let tys = &program.program().ty_intern;
        let Ty::ActorSum(members) = &*tys.get(*ty) else {
            panic!("expected Ty::ActorSum, got {:?}", tys.get(*ty))
        };
        assert_eq!(members.len(), 2);
        assert_eq!(members[0].0, "User");
        assert_eq!(members[0].1.display(tys), "UserId");
        assert_eq!(members[1].0, "Visitor");
        assert_eq!(
            members[1].1.display(tys),
            "()",
            "Visitor is a unit-identity prelude actor"
        );
    }

    #[test]
    fn binderless_by_clause_persists_no_binding() {
        let program = checked_context_program(
            r#"
context demo

type UserId = String

actor Buyer { auth = Internal, identity = UserId }

service Api {
  on call(ping: String) -> Effect[String] by Buyer {
    Effect.pure(ping)
  }
}
"#,
        );
        let handler = &find_service(program.program(), "Api").handlers[0];
        assert!(
            program.program().actor_binding(handler.span).is_none(),
            "a binder-less `by <Actor>` clause verifies-and-discards — no identity is bound, \
             so no persisted entry should exist for it either"
        );
    }

    #[test]
    fn no_by_clause_persists_no_binding() {
        let program = checked_context_program(
            r#"
context demo

service Api {
  on call(ping: String) -> Effect[String] {
    Effect.pure(ping)
  }
}
"#,
        );
        let handler = &find_service(program.program(), "Api").handlers[0];
        assert!(program.program().actor_binding(handler.span).is_none());
    }

    #[test]
    fn binder_shadowing_a_param_persists_no_binding() {
        // `handler_actor_binding` suppresses the binding when the binder name
        // collides with a declared param (`bynk.actor.binder_shadows_param`)
        // — the body scope keeps the real parameter, not the actor. Pinning
        // this through persistence too, not just through the in-scope type.
        //
        // The shadow is a *hard* diagnostic — this source never certifies —
        // so this test reads `typed.actor_bindings` straight off the
        // pre-`certify` `TypedCommons` ([`checked_context_commons`]) rather
        // than going through [`checked_context_program`], which would panic
        // on `.expect("certify")` before this assertion ever ran.
        let (typed, _errors) = checked_context_commons(
            r#"
context demo

type UserId = String

actor Buyer { auth = Internal, identity = UserId }

service Api {
  on call(u: String) -> Effect[String] by u: Buyer {
    Effect.pure(u)
  }
}
"#,
        );
        let handler = &find_service(&typed, "Api").handlers[0];
        assert!(typed.actor_binding(handler.span).is_none());
    }

    #[test]
    fn multiple_handlers_persist_distinct_bindings_keyed_per_handler() {
        // Review of #1170: every fixture above declares exactly one handler,
        // so none of them can tell "keyed per handler" apart from "keyed per
        // service" (or from an over-broad insert) — a single span in play
        // reads the same either way. Three handlers, only two with a `by`
        // binder, pins both: each binder lands on its own handler's own
        // span, and the binder-less handler contributes no entry at all.
        let program = checked_context_program(
            r#"
context demo

type UserId = String

actor Buyer { auth = Internal, identity = UserId }

service Api {
  on call(ping: String) -> Effect[String] by u: Buyer {
    Effect.pure(ping)
  }
  on call(ping: String) -> Effect[String] by v: Buyer {
    Effect.pure(ping)
  }
  on call(ping: String) -> Effect[String] {
    Effect.pure(ping)
  }
}
"#,
        );
        let service = find_service(program.program(), "Api");
        assert_eq!(service.handlers.len(), 3);
        let (first, second, third) = (
            &service.handlers[0],
            &service.handlers[1],
            &service.handlers[2],
        );
        let (binder, _) = program
            .program()
            .actor_binding(first.span)
            .unwrap_or_else(|| panic!("expected a persisted binding for the first handler"));
        assert_eq!(binder, "u");
        let (binder, _) = program
            .program()
            .actor_binding(second.span)
            .unwrap_or_else(|| panic!("expected a persisted binding for the second handler"));
        assert_eq!(binder, "v");
        assert!(
            program.program().actor_binding(third.span).is_none(),
            "the third handler declares no `by` clause at all"
        );
        assert_eq!(
            program.program().actor_bindings.len(),
            2,
            "exactly the two `by`-bearing handlers, nothing extra persisted for the third"
        );
    }
}
