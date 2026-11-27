use crate::http_pass_through::check_pass_through_routes;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::builtin_names::methods::{OF, UNSAFE};
use crate::checker::{
    self, CapabilityInfo, CapabilityOpInfo, Ty, TyId, TypedExpr, Types, statement_at_boundary,
};
use crate::hints::HintSink;
use crate::index::{RefSink, SymbolKind};
use crate::kernel_op::{FormStatic, JsonStatic, StaticOp, YamlStatic};
use crate::locals::LocalsSink;
use crate::requirements::RequirementSink;
use crate::resolver::{self, ResolvedCommons};
use crate::symbols::{ConsumedType, UnitTable, record_provides_clause_ref, resolve_given_cap_ref};
use bynk_project::detect_provider_dependency_cycles;
use bynk_syntax::ast::*;
use bynk_syntax::error::{Applicability, CompileError};
use bynk_syntax::span::Span;

/// The base a derived capability (`capability Stock = D1`) was made from, as
/// `(unit, base)`. One definition for handlers and test/property bodies, so
/// the #2865 typed-read recognition cannot drift between them (#2995).
pub(crate) fn derived_from(
    decl: &CapabilityDecl,
    flattened_caps: &HashMap<String, String>,
) -> Option<(String, String)> {
    decl.derived.as_ref().and_then(|d| {
        flattened_caps
            .get(&d.base.name)
            .map(|unit| (unit.clone(), d.base.name.clone()))
    })
}

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
        codable_params: checker::codable::codable_op_params(op),
        key_params: checker::codable::key_op_params(op),
        reply_policy: checker::codable::op_reply_policy(op),
        boundary_conflict: None,
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
        // #2868: a `given` function is typed with the context's declarations,
        // after this pass; `check_handler_constraints` walks it.
        if let CommonsItem::Fn(f) = item
            && f.given.is_empty()
        {
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
            CommonsItem::Service(s) => {
                check_handler_inputs_not_external_opaque(s, typed, consumed_types, &mut errors);
                (s.handlers.iter().map(|h| &h.body).collect(), Vec::new())
            }
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
            // #2868: a `given` function, typed with the context's declarations.
            // #2882: a function `given_fn_refusal` rejected was not typed
            // (`check_given_fns`), so its body is not walked either. This pass
            // runs on contexts only.
            CommonsItem::Fn(f)
                if !f.given.is_empty() && checker::given_fn_refusal(f, true).is_none() =>
            {
                (
                    vec![&f.body],
                    f.requires
                        .iter()
                        .chain(&f.ensures)
                        .map(|c| &c.predicate)
                        .collect(),
                )
            }
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

/// The types an expression decodes from JSON: the target of `Json.decode` /
/// `Json.decodeSql`, and each type that fills a `Codable` witness of a callee
/// (its decode half runs inside the callee). Encoding is not a decode.
fn decode_targets(e: &Expr, typed: &checker::TypedCommons) -> Vec<TyId> {
    use checker::Callee;
    use checker::codable::{WitnessKind, witness_list};
    match typed.callees.get(&e.id) {
        Some(Callee::Intrinsic {
            op:
                StaticOp::Json(JsonStatic::Decode | JsonStatic::DecodeSql)
                | StaticOp::Yaml(YamlStatic::Decode)
                | StaticOp::Form(FormStatic::Decode),
            ..
        }) => match typed.expr_ty(e.id).as_deref() {
            Some(Ty::Result(t, _)) => vec![*t],
            _ => Vec::new(),
        },
        // #2865: a typed `D1` read decodes its row.
        Some(Callee::D1Select { row, .. }) => vec![*row],
        // #4105: a typed Analytics read decodes its rows.
        Some(Callee::AnalyticsSelect { row, .. }) => vec![*row],
        // #4099: `Ai.generateValue[T]` decodes the reply as `T`.
        Some(Callee::AiGenerateValue { value, .. }) => vec![*value],
        Some(
            Callee::Fn(decl, ws)
            | Callee::FnValue(decl, ws)
            | Callee::Method(decl, ws)
            | Callee::Static(decl, ws),
        ) => witness_list(decl, &typed.types)
            .iter()
            .zip(ws)
            .filter(|(w, _)| w.kind == WitnessKind::Codec)
            .map(|(_, t)| *t)
            .collect(),
        _ => Vec::new(),
    }
}

/// The first externally-owned opaque type that decoding `t` would create: a
/// consumed type its owner exports opaque, or one declared `opaque`, found
/// through named types' fields and payloads as well as through the type
/// itself. Such a value can only be made by the owning context (#2794).
fn external_opaque_in(
    t: TyId,
    typed: &checker::TypedCommons,
    consumed: &HashMap<String, ConsumedType>,
    tys: &Arc<Types>,
) -> Option<(String, String)> {
    fn names(t: TyId, tys: &Arc<Types>, out: &mut Vec<String>) {
        match &*tys.get(t) {
            Ty::Named { name, args, .. } => {
                out.push(name.clone());
                args.iter().for_each(|a| names(*a, tys, out));
            }
            Ty::Option(a) | Ty::List(a) | Ty::Set(a) => names(*a, tys, out),
            Ty::Result(a, b) | Ty::Map(a, b) => {
                names(*a, tys, out);
                names(*b, tys, out);
            }
            _ => {}
        }
    }
    let mut stack = Vec::new();
    names(t, tys, &mut stack);
    external_opaque_reaching(stack, typed, consumed)
}

/// As [`external_opaque_in`], from the type names a position mentions.
fn external_opaque_reaching(
    mut stack: Vec<String>,
    typed: &checker::TypedCommons,
    consumed: &HashMap<String, ConsumedType>,
) -> Option<(String, String)> {
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(n) = stack.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        let decl = typed.types.get(&n);
        if let Some(ct) = consumed.get(&n)
            && (ct.visibility == Visibility::Opaque
                || decl.is_some_and(|d| matches!(d.body, TypeBody::Opaque { .. })))
        {
            return Some((n, ct.owning_context.clone()));
        }
        let Some(decl) = decl else {
            continue;
        };
        let vars: HashSet<&str> = decl
            .type_params
            .iter()
            .map(|p| p.name.name.as_str())
            .collect();
        let mut found = Vec::new();
        match &decl.body {
            TypeBody::Record(r) => r
                .fields
                .iter()
                .for_each(|f| crate::project_model::type_ref_names(&f.type_ref, &vars, &mut found)),
            TypeBody::Sum(sum) => sum.variants.iter().for_each(|v| {
                v.payload.iter().for_each(|p| {
                    crate::project_model::type_ref_names(&p.type_ref, &vars, &mut found)
                })
            }),
            TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {}
        }
        stack.extend(found);
    }
    None
}

/// #2794: decoding creates a value, so a context cannot decode a type another
/// context owns and exports opaque, through `Json.decode`, `Json.decodeSql` or a
/// `Codable` witness: its codec would check no more than the base type, because
/// the owner's predicate is the owner's secret. A reply from the owner's own
/// service is not this: that decode is generated, and a skewed owner is caught
/// by the contract hash.
fn check_decode_of_external_opaque(
    e: &Expr,
    typed: &checker::TypedCommons,
    consumed: &HashMap<String, ConsumedType>,
    errors: &mut Vec<CompileError>,
    tys: &Arc<Types>,
) {
    if consumed.is_empty() {
        return;
    }
    for t in decode_targets(e, typed) {
        if let Some((name, owner)) = external_opaque_in(t, typed, consumed, tys) {
            errors.push(
                CompileError::new(
                    "bynk.context.external_decode",
                    e.span,
                    format!(
                        "cannot decode `{}` here — `{name}` is owned by context `{owner}`",
                        tys.display(t),
                    ),
                )
                .with_note(
                    "decoding creates a value, and values of an externally-owned opaque type can only be created inside the owning context; take the value from a service of `{owner}` instead".replace("{owner}", &owner),
                ),
            );
            return;
        }
    }
}

/// #2794: an HTTP or queue handler decodes its parameters from bytes the
/// context did not produce, so a parameter that reaches a type another context
/// owns and exports opaque would create one of its values from the base type
/// alone. A `call` handler's arguments come from a Bynk caller that holds such
/// a value, so it is not an input in this sense. A `from Events(E)` subscriber is
/// one: its event arrives as the publisher's payload, decoded here, so a
/// payload field of such a type is refused (the event's declaration is walked
/// like any record's).
fn check_handler_inputs_not_external_opaque(
    s: &ServiceDecl,
    typed: &checker::TypedCommons,
    consumed: &HashMap<String, ConsumedType>,
    errors: &mut Vec<CompileError>,
) {
    if consumed.is_empty() || matches!(s.protocol, ServiceProtocol::Call | ServiceProtocol::Cron) {
        return;
    }
    // A held WebSocket decodes every inbound frame as `in_type`, whichever
    // handler the frame reaches.
    if let ServiceProtocol::WebSocket { in_type, .. } = &s.protocol {
        let mut names = Vec::new();
        crate::project_model::type_ref_names(in_type, &HashSet::new(), &mut names);
        if let Some((name, owner)) = external_opaque_reaching(names, typed, consumed) {
            errors.push(
                CompileError::new(
                    "bynk.context.external_decode",
                    in_type.span(),
                    format!(
                        "cannot take inbound frames of this type here — `{name}` is owned by context `{owner}`"
                    ),
                )
                .with_note(
                    "a frame is decoded from bytes this context did not produce, which creates a value; values of an externally-owned opaque type can only be created inside the owning context",
                ),
            );
            return;
        }
    }
    for h in &s.handlers {
        for p in &h.params {
            // #3752: a pass-through route's `InboundRequest` is the router's own
            // request, never decoded.
            if crate::http_pass_through::is_inbound_request(&p.type_ref, &typed.types) {
                continue;
            }
            let mut names = Vec::new();
            crate::project_model::type_ref_names(&p.type_ref, &HashSet::new(), &mut names);
            if let Some((name, owner)) = external_opaque_reaching(names, typed, consumed) {
                errors.push(
                    CompileError::new(
                        "bynk.context.external_decode",
                        p.type_ref.span(),
                        format!(
                            "cannot take `{}` as an input here — `{name}` is owned by context `{owner}`",
                            p.name.name,
                        ),
                    )
                    .with_note(
                        "an input is decoded from bytes this context did not produce, which creates a value; values of an externally-owned opaque type can only be created inside the owning context",
                    ),
                );
            }
        }
    }
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
    check_decode_of_external_opaque(e, typed, consumed, errors, tys);
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
                // #3948, #4213: a type its owner exports `transparent` takes every
                // ordinary construction form from a consumer: `.of` on a refined
                // one, a variant of a sum, as a record literal already did. An
                // opaque export, a declared `opaque` type and `.unsafe` (an
                // unchecked assertion, not a construction) stay the owner's.
                let owner_only = ct.visibility == Visibility::Opaque
                    || matches!(
                        typed.types.get(&id.name).map(|d| &d.body),
                        Some(TypeBody::Opaque { .. })
                    );
                let is_construct = method.name == UNSAFE
                    || (owner_only
                        && (method.name == OF
                            || matches!(
                                typed.types.get(&id.name).map(|d| &d.body),
                                Some(TypeBody::Sum(s)) if s.variants.iter().any(|v| v.name.name == method.name),
                            )));
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
                    arms.first().map(|a| a.pattern.peel_as()),
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
        Pattern::Refined { inner, .. } | Pattern::As { inner, .. } => {
            opaque_record_patterns(inner, consumed, top, errors)
        }
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

    let resolved = declarations_resolved(
        typed,
        table,
        cross_context,
        is_context,
        uses_commons_type_names,
    );

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
                    derived_from: derived_from(decl, &cross_context.flattened_caps),
                },
            )
        })
        .collect();
    // v0.17: flattened capabilities (`consumes U { Cap }`) enter the local map
    // under their bare names.
    capability_info_map.extend(flattened_capability_infos(typed, cross_context, tys));

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
    check_given_fns(
        typed,
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

    // #4105: every handler body is checked, so each `AnalyticsSql.select[T]` can be written from the
    // record its instance writes.
    checker::analytics::resolve_selects(&mut typed.callees, &typed.types, tys, &mut errors);

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
    let mut agents: Vec<&AgentDecl> = table.agents.values().map(|a| &**a).collect();
    agents.sort_by_key(|a| a.span.start);
    for a in agents {
        // #3793: the source-only rules for a `store` field's `replaces`.
        errors.extend(crate::field_renames::check_store(a));
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
            if let Err(reason) = crate::wire_default::lower_event_field_default_wire(
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
                        errors.push(crate::annotation_consts::with_const_origin(
                            CompileError::new(
                                "bynk.event.bad_schema_version",
                                arg.span,
                                "`@schema`'s argument must be a positive `Int` literal, or an `Int` constant bound to one",
                            ),
                            arg,
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

/// The resolved-commons snapshot the declaration checks type bodies against:
/// `typed`'s merged tables with the unit's local ones. Shared by a context's
/// declarations and (#3959) a commons' `given` functions.
pub(crate) fn declarations_resolved(
    typed: &checker::TypedCommons,
    table: &UnitTable,
    cross_context: &resolver::CrossContextInfo,
    is_context: bool,
    uses_commons_type_names: &HashSet<String>,
) -> ResolvedCommons {
    // Build a resolved-commons snapshot for the per-handler checker.
    // We synthesise a ResolvedCommons by reusing typed.types / typed.fns /
    // typed.methods; the resolver wouldn't add anything new. `ResolvedCommons::new`
    // derives `local_type_names`/`event_type_names` from `table` — the
    // *pre-merge* local table — rather than `typed.types` (already
    // local+uses+consumes merged); see its doc comment for why that
    // distinction matters (owner-only emission, spine #936).
    ResolvedCommons::new(
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
    .with_local_fns(&table.fns)
    // #1883, slice 1a: a handler body sees the unit's assets too.
    .with_assets(typed.assets.clone())
    .with_embeds(typed.embeds.clone())
    .with_consts(typed.consts.clone())
    .with_shadowed_imports(typed.shadowed_imports.clone())
    .with_scope_facts_of(typed)
    .with_stability(typed.stability.clone())
}

/// v0.17: the capabilities a unit flattens in with `consumes U { Cap }`, by
/// bare name, resolved from the consumed unit's exported capability so bare
/// `given Cap` / `Cap.op(…)` type-check as if local. #3959: a commons' braced
/// `consumes` of a first-party unit's capabilities reaches them the same way.
pub(crate) fn flattened_capability_infos(
    typed: &checker::TypedCommons,
    cross_context: &resolver::CrossContextInfo,
    tys: &Arc<Types>,
) -> HashMap<String, CapabilityInfo> {
    // v0.17: flattened capabilities (`consumes U { Cap }`) enter the local map
    // under their bare names, resolved from the consumed unit's exported
    // capability so bare `given Cap` / `Cap.op(…)` type-check as if local.
    let mut out: HashMap<String, CapabilityInfo> = HashMap::new();
    let mut receiving_cache: Option<checker::Projection> = None;
    for (cap, unit) in &cross_context.flattened_caps {
        let Some(xcap) = cross_context
            .consumed_capabilities
            .get(unit)
            .and_then(|m| m.get(cap))
        else {
            continue;
        };
        // #2634: an operation may name a type the unit declares but does not
        // export (an adapter's `Token`), or one the consumer also declares.
        // As the qualified `U.Cap.op` call does, resolve the signature against
        // the unit's own table; a name the consumer binds to a structurally
        // different type is refused (`bynk.boundary.structural_mismatch`)
        // rather than read as the consumer's. The refusal is per op, at its call.
        let unit_types = cross_context
            .consumed_types
            .get(unit)
            .unwrap_or(&typed.types);
        let (receiving, _) = receiving_cache.get_or_insert_with(|| {
            checker::projected_types(&typed.types, &cross_context.consumed_types, tys)
        });
        let ops = xcap
            .ops
            .iter()
            .map(|op| {
                let vars: HashSet<String> = op.type_params.iter().cloned().collect();
                let mut conflict: Option<(String, String, bool)> = None;
                let mut resolve = |tr: &TypeRef, in_param: bool| {
                    // #4111: a type the unit consumes from an adapter (`bynk.crypto`'s `Secret`)
                    // is not in its contract view, which reads consumed contexts only, so a name
                    // the view lacks is read as `bynk`'s own type of that name. It is resolved
                    // against `bynk`'s declarations and never the consumer's table, where a type
                    // of the consumer's own called `Secret` would answer to it.
                    let (ty, signature_types) =
                        match checker::resolve_type_ref_in(tr, unit_types, &vars, tys) {
                            Some(ty) => (ty, unit_types),
                            None => {
                                match checker::resolve_type_ref_in(tr, bynk_types(), &vars, tys) {
                                    Some(ty) => (ty, bynk_types()),
                                    None => (tys.intern(Ty::Unit), unit_types),
                                }
                            }
                        };
                    // The same-name check reads the table the type came from, so a consumer type
                    // called `Secret` is refused against `bynk`'s, not mistaken for it.
                    if conflict.is_none()
                        && let Some(name) = checker::rebrand_conflict(
                            ty,
                            signature_types,
                            &typed.types,
                            receiving,
                            tys,
                        )
                    {
                        conflict = Some((unit.clone(), name, in_param));
                    }
                    checker::rebrand_return_type(ty, &typed.types, tys)
                };
                let params = op.params.iter().map(|(_, tr)| resolve(tr, true)).collect();
                let return_ty = resolve(&op.return_type, false);
                CapabilityOpInfo {
                    name: op.name.clone(),
                    type_params: op.type_params.clone(),
                    params,
                    param_names: op.params.iter().map(|(n, _)| n.clone()).collect(),
                    return_ty,
                    codable_params: op.codable_params.clone(),
                    key_params: op.key_params.clone(),
                    reply_policy: op.reply_policy,
                    boundary_conflict: conflict,
                }
            })
            .collect();
        out.insert(
            cap.clone(),
            CapabilityInfo {
                name: cap.clone(),
                ops,
                derived_from: None,
            },
        );
    }
    out
}

/// #2868: type-check this file's context functions that declare `given`. They
/// wait for the context's capability table, which a plain function's check
/// (`check_record_in`) does not have. A function the declaration rules refuse
/// (`given_fn_refusal`) was reported there and is not checked again.
#[allow(clippy::too_many_arguments)]
fn check_given_fns(
    typed: &mut checker::TypedCommons,
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
    let fns: Vec<Arc<FnDecl>> = typed
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Fn(f) if !f.given.is_empty() => Some(Arc::new(f.clone())),
            _ => None,
        })
        .collect();
    for f in fns {
        if checker::given_fn_refusal(&f, resolved.is_context).is_some() {
            continue;
        }
        refs.set_owner(f.name.display());
        let mut capabilities: HashMap<String, CapabilityInfo> = HashMap::new();
        for cap_ref in &f.given {
            if let Some(info) =
                resolve_given_cap_ref(cap_ref, capability_info_map, cross_context, errors, refs)
            {
                capabilities.insert(cap_ref.key().to_string(), info);
            }
        }
        checker::check_context_fn(
            &f,
            checker::FnCaps {
                capabilities,
                declared_capabilities: capability_info_map.clone(),
            },
            resolved,
            &mut typed.expr_types,
            &mut typed.callees,
            errors,
            refs,
            hints,
            locals,
            requirements,
            tys,
        );
        refs.clear_owner();
    }
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

/// #3766: an `on call` service has exactly one `on call` handler. Both were
/// emitted as members of one object literal, so the last silently shadowed
/// the first and its body was dead code. Reported on each handler past the
/// first (services name-sorted for a deterministic order), in the shape of
/// `bynk.event.duplicate_handler` and `bynk.email.duplicate_handler`.
pub(crate) fn check_one_call_handler(table: &UnitTable, errors: &mut Vec<CompileError>) {
    let mut services: Vec<&ServiceDecl> = table
        .services
        .values()
        .filter(|s| matches!(s.protocol, ServiceProtocol::Call))
        .collect();
    services.sort_by(|a, b| a.name.name.cmp(&b.name.name));
    for service in services {
        let calls: Vec<_> = service
            .handlers
            .iter()
            .filter(|h| h.kind == HandlerKind::Call)
            .collect();
        for extra in calls.iter().skip(1) {
            errors.push(
                CompileError::new(
                    "bynk.service.duplicate_call_handler",
                    extra.span,
                    format!(
                        "the service `{}` has more than one `on call` handler — it needs exactly one",
                        service.name.name
                    ),
                )
                .with_label(calls[0].span, "the service's `on call` handler")
                .with_note(
                    "a service is called by its name, so it has one entry; to offer a second operation, declare a second service",
                ),
            );
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
pub(crate) fn check_service_protocols(
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
                    .with_note("a `from websocket` service holds exactly one `on open`, and one `on close` (the departure), and optionally one `on message` (inbound)"),
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
            check_ws_close_arity(service, opens.len(), &closes, errors);
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
                    for (i, p) in close_route_params(close, visible_types).iter().enumerate() {
                        if op
                            .get(i)
                            .is_none_or(|o| resolve_ty(&p.type_ref) != resolve_ty(&o.type_ref))
                        {
                            route_mismatch(p, errors);
                        }
                    }
                }
            }
            // #3046: the principal is established once, by `on open`'s `by`, and
            // carried for the connection's life (the Workers socket attachment).
            // `on message`/`on close` read it rather than re-verifying per frame, so
            // their `by` must name the same kind of principal: the same single actor
            // or the same actor-sum members. Binder names may differ, and a sum's
            // member order is irrelevant (the order only matters to open's
            // first-wins resolution). A checker rule, not a Workers-only one: the
            // contract is the language's, whatever a target does with the principal.
            if let [open] = opens.as_slice() {
                let actors_of = |h: &Handler| -> Option<std::collections::BTreeSet<String>> {
                    h.by_clause
                        .as_ref()
                        .map(|by| by.actors.iter().map(|a| a.name.clone()).collect())
                };
                let render = |h: &Handler| -> String {
                    match &h.by_clause {
                        Some(by) => by
                            .actors
                            .iter()
                            .map(|a| a.name.as_str())
                            .collect::<Vec<_>>()
                            .join(" | "),
                        None => "no `by` clause".to_string(),
                    }
                };
                let open_actors = actors_of(open);
                for h in messages.iter().chain(closes.iter()) {
                    if actors_of(h) != open_actors {
                        let which = if matches!(h.kind, HandlerKind::Message) {
                            "on message"
                        } else {
                            "on close"
                        };
                        errors.push(
                            CompileError::new(
                                "bynk.ws.lifecycle_by_mismatch",
                                h.by_clause.as_ref().map_or(h.span, |by| by.span),
                                format!(
                                    "the `{which}` handler of `{}` is `by` {}, but `on open` is `by` {} — a connection's principal is established once, at `on open`, so the lifecycle handlers must name the same actor(s)",
                                    service.name.name,
                                    render(h),
                                    render(open),
                                ),
                            )
                            .with_note(
                                "give `on message`/`on close` the same `by` actor (or the same actor-sum members) as `on open`; the binder name may differ. The principal is carried from `on open` for the connection's life, never re-verified per frame",
                            ),
                        );
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
                            "transfer the connection to an agent unconditionally (not inside an `if`/`match`); key it by a handler parameter, a `const` or a literal so the upgrade can be routed",
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
                    | (ServiceProtocol::Email { .. }, HandlerKind::Message)
                    // v0.103/v0.106: a `from websocket` admits `on open` (the
                    // upgrade), and the inbound/close lifecycle `on message`/`on
                    // close` (slice 3b-iii).
                    | (
                        ServiceProtocol::WebSocket { .. },
                        HandlerKind::Open | HandlerKind::Message | HandlerKind::Close
                    )
                    // Events (#936): `from Events(E)` admits exactly `on event(e: E)`.
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
        ServiceProtocol::Email { .. } => "from email",
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
        TypeRef::HttpResult(t, None, _) => format!("HttpResult[{}]", ts_type_ref_display(t)),
        TypeRef::HttpResult(t, Some(e), _) => format!(
            "HttpResult[{}, {}]",
            ts_type_ref_display(t),
            ts_type_ref_display(e)
        ),
        TypeRef::QueueResult(_) => "QueueResult".to_string(),
        TypeRef::Asset(_) => "Asset".to_string(),
        TypeRef::RequestHeaders(_) => "RequestHeaders".to_string(),
        TypeRef::RequestQuery(_) => "RequestQuery".to_string(),
        TypeRef::RequestCookies(_) => "RequestCookies".to_string(),
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
        TypeRef::DecodeError(_) => "DecodeError".to_string(),
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
        if by.is_sum()
            && local.is_some_and(|a| {
                a.refinement.is_some()
                    && !is_caller_refinement(a, table)
                    && !is_sender_refinement(a, table)
                    && !is_producer_refinement(a, table)
            })
        {
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
        if local.is_some() {
            refs.record(actor_ref.span, SymbolKind::Actor, &actor_ref.name);
        }
        let contract = by_actor_contract(&actor_ref.name, local, table);
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
                         (`Visitor`, `Scheduler`, `Producer`, `Sender`, `Caller`)",
                    ),
                );
            }
            continue;
        };
        errors.extend(actor_narrowing_refusal(
            protocol,
            by,
            local.map(|a| {
                (
                    is_producer_refinement(a, table),
                    is_sender_refinement(a, table),
                )
            }),
            by_mixes_signed(by, table, &actor_ref.name),
        ));
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
                    // I3 (#1971): a foreign caller calls a service, and (#3899) a foreign
                    // producer sends to a queue consumer.
                    _ if contract.scheme == Scheme::Foreign => {
                        "a `Foreign` actor is admitted only on an `on call` handler or a queue consumer's `on message`"
                    }
                    ServiceProtocol::WebSocket { .. } if contract.scheme == Scheme::ApiKey => {
                        crate::api_key::WEBSOCKET_NOTE
                    }
                    ServiceProtocol::Http => {
                        "public HTTP routes take an anonymous actor — write `by v: Visitor`"
                    }
                    _ => "internal protocols (call/cron/queue) take an `Internal` actor",
                }),
            );
        }
        errors.extend(crate::api_key::by_member_refusal(
            protocol,
            by,
            contract.scheme,
            actor_ref.span,
        ));
        // #3904: a WebSocket actor sum takes `None` and `Bearer` members; a cookie or an OIDC token is
        // a single actor (it needs the `Origin` check or the `bynk.oidc` pair, not the sum's first-wins).
        if matches!(protocol, ServiceProtocol::WebSocket { .. })
            && by.is_sum()
            && matches!(contract.scheme, Scheme::Oidc | Scheme::SignedCookie)
        {
            errors.push(
                CompileError::new(
                    "bynk.actor.scheme_not_admissible",
                    actor_ref.span,
                    format!(
                        "a `{}` actor is not admissible as a member of a WebSocket actor sum",
                        contract.scheme.as_str(),
                    ),
                )
                .with_note(
                    "a `from websocket` `on open` takes a `SignedCookie` or `Oidc` actor on its own (`by user: User`); an actor sum there takes `None` and `Bearer` members",
                ),
            );
        }
        // v0.54: the `Caller` prelude actor yields a `CallerId` (the calling
        // context's name), a cross-context `on call` concept — it is admissible
        // only on the `Call` protocol, even though its `Internal` scheme is
        // otherwise valid on cron/queue (those take `Scheduler`/`Producer`).
        let is_caller = (!table.actors.contains_key(&actor_ref.name)
            && actors::prelude_actor(&actor_ref.name).map(|c| c.identity)
                == Some(actors::Identity::CallerId))
            || local.is_some_and(|a| is_caller_refinement(a, table));
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
    // #3380: a `Signature` actor computes its HMAC over the whole body before the
    // handler runs, so the body cannot be a stream the handler reads afterwards.
    if let Some(params) = params
        && members
            .iter()
            .any(|(_, c)| c.scheme == actors::Scheme::Signature)
        && let Some(body) = params
            .iter()
            .find(|p| p.name.name == "body" && is_byte_stream(&p.type_ref))
    {
        errors.push(
            CompileError::new(
                "bynk.http.stream_body_signature",
                body.type_ref.span(),
                "a `Signature` route cannot take a streamed `body`",
            )
            .with_note(
                "the signature is an HMAC over the whole body, verified before the handler runs; take `body: Bytes` for a signed webhook, or sign the head (a header or the path) and verify the content out of band",
            ),
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

/// #3386: the config value kinds a scheme admits. The config grammar also reads a duration
/// (`12.hours`) and a bare identifier (`Lax`), but only `SignedCookie` takes either; for every other
/// scheme they are refused here, because their seams read a string or an integer and would otherwise
/// drop the value quietly (a `Signature` `tolerance = 5.minutes` reads as no tolerance at all, which
/// would turn the replay window off). A repeated `SignedCookie` key is refused too, since the first
/// would win quietly in a scheme that guards a session.
fn check_scheme_arg_kinds(actor: &ActorDecl, errors: &mut Vec<CompileError>) {
    use crate::actors::Scheme;
    let signed_cookie =
        actor.auth.as_ref().and_then(|a| Scheme::from_name(&a.name)) == Some(Scheme::SignedCookie);
    let mut seen: HashSet<&str> = HashSet::new();
    for arg in &actor.auth_config {
        if signed_cookie && !seen.insert(arg.key.name.as_str()) {
            errors.push(
                CompileError::new(
                    "bynk.actor.signed_cookie_duplicate_key",
                    arg.key.span,
                    format!(
                        "the scheme config key `{}` is given more than once",
                        arg.key.name
                    ),
                )
                .with_note(
                    "only the first value would be read, so the second is refused: keep one",
                ),
            );
        }
        if !signed_cookie
            && matches!(
                arg.value,
                bynk_syntax::ast::SchemeArgValue::Duration { .. }
                    | bynk_syntax::ast::SchemeArgValue::Ident(_)
            )
        {
            errors.push(
                CompileError::new(
                    "bynk.actor.scheme_arg_kind",
                    arg.span,
                    format!(
                        "the scheme config value for `{}` must be a string or an integer",
                        arg.key.name
                    ),
                )
                .with_note(
                    "a duration or a bare name is accepted only by `SignedCookie`; give a whole number (a `Signature` `tolerance` is in seconds: `tolerance = 300`) or a quoted string",
                ),
            );
        }
    }
}

/// The keys a `Bearer` actor's config admits (#3821): the signing `secret`, the `issuer` and `audience`
/// its tokens must carry, and the `trustFrom` variable that can replace the last two at run time.
const BEARER_KEYS: &[&str] = &["secret", "issuer", "audience", "trustFrom"];

/// #3821: the `Bearer` config checks that are not about the secret or the identity: the key set is
/// closed (so an `issuer` that was never enforced cannot be written and ignored), `issuer`, `audience`
/// and `trustFrom` are non-empty strings, and two `Bearer` actors on one secret must each declare an
/// `audience`, and different ones, or a token minted for one is accepted by the other.
fn check_bearer_actor(
    actor: &ActorDecl,
    auth: &Ident,
    table: &UnitTable,
    errors: &mut Vec<CompileError>,
) {
    for (i, arg) in actor.auth_config.iter().enumerate() {
        // A repeated key would be read once and the other dropped, which is what a check you wrote must
        // never do (the shared-secret rule below compares the one value that is read).
        if actor.auth_config[..i]
            .iter()
            .any(|earlier| earlier.key.name == arg.key.name)
        {
            errors.push(
                CompileError::new(
                    "bynk.actor.bearer_bad_arg",
                    arg.key.span,
                    format!("the `Bearer` config key `{}` is given more than once", arg.key.name),
                )
                .with_note(
                    "the first would win quietly in a scheme that guards a token; give each key once",
                ),
            );
        }
        if !BEARER_KEYS.contains(&arg.key.name.as_str()) {
            errors.push(
                CompileError::new(
                    "bynk.actor.bearer_unknown_key",
                    arg.key.span,
                    format!("unknown `Bearer` config key `{}`", arg.key.name),
                )
                .with_note(
                    "the keys are `secret`, `issuer`, `audience` and `trustFrom`; a key that is not one of them would be ignored, and a token check you wrote would not run",
                ),
            );
        }
    }
    for key in ["issuer", "audience", "trustFrom"] {
        if let Some(arg) = actor.scheme_arg(key)
            && arg.value.as_str().is_none_or(str::is_empty)
        {
            errors.push(
                CompileError::new(
                    "bynk.actor.bearer_bad_arg",
                    arg.span,
                    format!("a `Bearer` actor's `{key}` must be a non-empty string"),
                )
                .with_note(
                    "write `auth = Bearer(secret = \"AUTH_SECRET\", issuer = \"web\", audience = \"api\")`; `trustFrom` names the environment variable whose JSON `{\"issuer\", \"audience\"}` replaces them",
                ),
            );
        }
    }
    let Some(secret) = actor.scheme_arg("secret").and_then(|a| a.value.as_str()) else {
        return;
    };
    if let Some(arg) = actor.scheme_arg("trustFrom")
        && arg.value.as_str() == Some(secret)
    {
        errors.push(
            CompileError::new(
                "bynk.actor.bearer_bad_arg",
                arg.span,
                "a `Bearer` actor's `trustFrom` names its own signing secret",
            )
            .with_note(
                "`trustFrom` names a different environment variable, whose JSON `{ \"issuer\", \"audience\" }` replaces the declared values; the secret holds the signing key",
            ),
        );
    }
    let mine = actor.scheme_arg("audience").and_then(|a| a.value.as_str());
    for other in table.actors.values() {
        // Report each pair once, at the actor that sorts later, so the order of the table does not matter.
        if other.name.name >= actor.name.name
            || other
                .auth
                .as_ref()
                .and_then(|a| crate::actors::Scheme::from_name(a.name.as_str()))
                != Some(crate::actors::Scheme::Bearer)
            || other.scheme_arg("secret").and_then(|a| a.value.as_str()) != Some(secret)
        {
            continue;
        }
        let theirs = other.scheme_arg("audience").and_then(|a| a.value.as_str());
        // One `trustFrom` variable shared by the pair would give both the same audience at run time
        // (a staging override is the natural use), whatever each declares.
        let my_trust = actor.scheme_arg("trustFrom").and_then(|a| a.value.as_str());
        let shared_trust = my_trust.is_some()
            && my_trust == other.scheme_arg("trustFrom").and_then(|a| a.value.as_str());
        if !shared_trust && mine.is_some() && theirs.is_some() && mine != theirs {
            continue;
        }
        if shared_trust {
            errors.push(
                CompileError::new(
                    "bynk.actor.bearer_secret_shared",
                    auth.span,
                    format!(
                        "`Bearer` actors `{}` and `{}` share the secret `{secret}` and the `trustFrom` variable `{}`, so that variable can give both the same audience and a token minted for one is accepted by the other",
                        other.name.name,
                        actor.name.name,
                        my_trust.unwrap_or_default()
                    ),
                )
                .with_note(
                    "give each actor its own `trustFrom` variable (or none), and declare a different `audience` on each; mint each actor's tokens with its own audience, which `bynk token <Actor>` does. Or give each actor its own secret",
                ),
            );
            continue;
        }
        errors.push(
            CompileError::new(
                "bynk.actor.bearer_secret_shared",
                auth.span,
                format!(
                    "`Bearer` actors `{}` and `{}` share the secret `{secret}` without distinct audiences, so a token minted for one is accepted by the other",
                    other.name.name, actor.name.name
                ),
            )
            .with_note(format!(
                "declare a different `audience` on each (`audience = \"{}\"` and `audience = \"{}\"`) and mint each actor's tokens with its own audience; `bynk token <Actor>` does that. Or give each actor its own secret",
                other.name.name.to_lowercase(),
                actor.name.name.to_lowercase()
            )),
        );
    }
}

/// The keys a `SignedCookie` actor's config admits (#3386).
const SIGNED_COOKIE_KEYS: &[&str] = &[
    "cookie", "secret", "previous", "ttl", "sameSite", "sessions",
];

/// A `ttl` above this many seconds draws `signed_cookie_ttl_long` (30 days).
const SIGNED_COOKIE_LONG_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// Whether `name` is an RFC 6265 `token` (a legal cookie name).
fn is_cookie_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// #3386: validate one `SignedCookie` actor declaration: the closed key set, a
/// `secret` and a `ttl`, a legal and preferably `__Host-` cookie name, a
/// distinct secret, and a string-constructible identity minted from `sub`.
fn check_signed_cookie_actor(
    actor: &ActorDecl,
    auth: &Ident,
    table: &UnitTable,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    let bad_arg = |span: Span, key: &str, want: &str, errors: &mut Vec<CompileError>| {
        errors.push(
            CompileError::new(
                "bynk.actor.signed_cookie_bad_arg",
                span,
                format!("a `SignedCookie` actor's `{key}` must be {want}"),
            )
            .with_note(
                "write `auth = SignedCookie(secret = \"<ENV_NAME>\", ttl = 12.hours)`; the optional keys are `cookie`, `previous`, `sameSite` and `sessions`",
            ),
        );
    };
    for arg in &actor.auth_config {
        if !SIGNED_COOKIE_KEYS.contains(&arg.key.name.as_str()) {
            errors.push(
                CompileError::new(
                    "bynk.actor.signed_cookie_unknown_key",
                    arg.key.span,
                    format!("unknown `SignedCookie` config key `{}`", arg.key.name),
                )
                .with_note(
                    "the keys are `secret`, `ttl`, `cookie`, `previous`, `sameSite` and `sessions`",
                ),
            );
        }
    }
    // The signing key: named, a string, and not another actor's secret.
    match actor.scheme_arg("secret") {
        None => errors.push(
            CompileError::new(
                "bynk.actor.signed_cookie_missing_secret",
                auth.span,
                "a `SignedCookie` actor must name its signing secret",
            )
            .with_note(
                "write `auth = SignedCookie(secret = \"<ENV_NAME>\", ttl = 12.hours)` — the env var holding the signing key (`bynk deploy` mints it when unset)",
            ),
        ),
        Some(arg) if arg.value.as_str().is_none() => {
            bad_arg(arg.span, "secret", "an env binding name, as a string", errors);
        }
        Some(_) => {}
    }
    let previous = actor.scheme_arg("previous");
    if let Some(arg) = previous
        && arg.value.as_str().is_none()
    {
        bad_arg(
            arg.span,
            "previous",
            "an env binding name, as a string",
            errors,
        );
    }
    // Distinct keys: this actor's `secret` and `previous` may not be another
    // actor's secret (or previous), nor each other, or one scheme's credential
    // could be replayed against another.
    let mine: Vec<(&str, &str, Span)> = ["secret", "previous"]
        .iter()
        .filter_map(|k| {
            let a = actor.scheme_arg(k)?;
            Some((*k, a.value.as_str()?, a.span))
        })
        .collect();
    if let [(_, a, _), (_, b, span)] = mine.as_slice()
        && a == b
    {
        errors.push(
            CompileError::new(
                "bynk.actor.signed_cookie_secret_shared",
                *span,
                "a `SignedCookie` actor's `previous` key is the same variable as its `secret`",
            )
            .with_note("`previous` is the key being rotated out — give it its own variable"),
        );
    }
    for other in table.actors.values() {
        if other.name.name == actor.name.name {
            continue;
        }
        let other_names: Vec<&str> = ["secret", "previous"]
            .iter()
            .filter_map(|k| other.scheme_arg(k)?.value.as_str())
            .collect();
        for (key, value, span) in &mine {
            if other_names.contains(value) {
                errors.push(
                    CompileError::new(
                        "bynk.actor.signed_cookie_secret_shared",
                        *span,
                        format!(
                            "the `{key}` `{value}` of `SignedCookie` actor `{}` is also used by actor `{}`",
                            actor.name.name, other.name.name
                        ),
                    )
                    .with_note(
                        "a session key signs one actor's cookies only — sharing it would let a credential for one actor be replayed against another; give each its own variable",
                    ),
                );
            }
        }
    }
    // Expiry: required, a positive whole number of seconds, bounded by a warning.
    match actor.scheme_arg("ttl") {
        None => {
            let mut err = CompileError::new(
                "bynk.actor.signed_cookie_missing_ttl",
                auth.span,
                "a `SignedCookie` actor must declare a `ttl`",
            )
            .with_note(
                "an immortal session cookie is the failure to avoid — write `ttl = 12.hours`; a stateless cookie cannot be revoked before it expires, so keep it short",
            );
            // #3431: insert `ttl = 12.hours` after the last config entry. Adding
            // a key touches nothing else, but the lifetime is the author's
            // choice, so it is offered (`MaybeIncorrect`), never taken by
            // fix-all. No entry to anchor on (no config at all) ⇒ note only.
            if let Some(last) = actor.auth_config.last() {
                let at = Span::new_in(last.span.file, last.span.end, last.span.end);
                err = err.with_suggestion(
                    "add `ttl = 12.hours` (a stateless cookie cannot be revoked before it expires, so keep it short)",
                    vec![(at, ", ttl = 12.hours".to_string())],
                    Applicability::MaybeIncorrect,
                );
            }
            errors.push(err);
        }
        Some(arg) => match arg.value.as_duration_millis() {
            Some(ms) if ms > 0 && ms % 1000 == 0 => {
                if ms / 1000 > SIGNED_COOKIE_LONG_TTL_SECS {
                    errors.push(
                        CompileError::new(
                            "bynk.actor.signed_cookie_ttl_long",
                            arg.span,
                            "this `SignedCookie` `ttl` is longer than 30 days",
                        )
                        .with_note(
                            "a stateless signed cookie cannot be revoked before it expires: a copy a user already took keeps working for the whole `ttl`. Shorten it, or use a server-side session (see `examples/sessions`) if you need to log out everywhere",
                        ),
                    );
                }
            }
            _ => errors.push(
                CompileError::new(
                    "bynk.actor.signed_cookie_bad_ttl",
                    arg.span,
                    "a `SignedCookie` `ttl` must be a positive whole number of seconds",
                )
                .with_note(
                    "write a `Duration` literal such as `ttl = 12.hours` or `ttl = 30.minutes`",
                ),
            ),
        },
    }
    // The cookie's name: a token, and `__Host-` unless the author chose otherwise.
    if let Some(arg) = actor.scheme_arg("cookie") {
        match arg.value.as_str() {
            None => bad_arg(arg.span, "cookie", "a cookie name, as a string", errors),
            Some(name) if !is_cookie_token(name) => errors.push(
                CompileError::new(
                    "bynk.actor.signed_cookie_bad_name",
                    arg.span,
                    format!("`{name}` is not a legal cookie name"),
                )
                .with_note(
                    "a cookie name is an RFC 6265 token: letters, digits and ``!#$%&'*+-.^_`|~``, no spaces, `=`, `;`, `,` or quotes",
                ),
            ),
            Some(name) if !name.to_ascii_lowercase().starts_with("__host-") => errors.push(
                CompileError::new(
                    "bynk.actor.signed_cookie_unprefixed",
                    arg.span,
                    format!("the session cookie `{name}` is not a `__Host-` cookie"),
                )
                .with_note(
                    "a `__Host-` name stops a sibling subdomain setting or overwriting the cookie; the default is `__Host-session`",
                ),
            ),
            Some(_) => {}
        }
    }
    if let Some(arg) = actor.scheme_arg("sessions")
        && !matches!(arg.value.as_ident(), Some("true" | "false"))
    {
        bad_arg(arg.span, "sessions", "`true` or `false`", errors);
    }
    if let Some(arg) = actor.scheme_arg("sameSite")
        && !matches!(
            arg.value.as_ident(),
            Some("Lax" | "Strict" | "Unrestricted")
        )
    {
        bad_arg(
            arg.span,
            "sameSite",
            "`Lax`, `Strict` or `Unrestricted`",
            errors,
        );
    }
    match &actor.identity {
        None => errors.push(
            CompileError::new(
                "bynk.actor.signed_cookie_identity_not_string_constructible",
                auth.span,
                "a `SignedCookie` actor must declare a string-constructible `identity`",
            )
            .with_note(
                "the verified identity is minted from the cookie's `sub` claim — declare `identity = T` where `T` is a refined or opaque `String`",
            ),
        ),
        Some(id) if !is_string_constructible(id, types) => errors.push(
            CompileError::new(
                "bynk.actor.signed_cookie_identity_not_string_constructible",
                id.span(),
                "a `SignedCookie` actor's identity must be string-constructible",
            )
            .with_note(
                "the identity is minted from the cookie's `sub` claim (a string) — use a refined or opaque `String` type",
            ),
        ),
        Some(_) => {}
    }
}

/// #3386: route-level refusals for a handler guarded by a `SignedCookie` actor.
/// The signed value is consumed at the seam and never reaches handler code, so a
/// typed `cookies` record may not read the session cookie by name; and a shared
/// cache would serve one user's answer to another.
fn check_signed_cookie_route(
    handler: &Handler,
    table: &UnitTable,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    let Some(seam) = crate::actors::signed_cookie_seam_for(handler, &table.actors) else {
        return;
    };
    // A typed `cookies` field whose wire name (its own, or `@name`) is the session cookie.
    if let Some(p) = handler.params.iter().find(|p| p.name.name == "cookies")
        && let TypeRef::Named(rec) = &p.type_ref
        && let Some(TypeBody::Record(r)) = types.get(&rec.name).map(|d| &d.body)
    {
        for f in &r.fields {
            let wire = f.wire_name().unwrap_or_else(|| f.name.name.clone());
            if wire == seam.cookie {
                errors.push(
                    CompileError::new(
                        "bynk.actor.signed_cookie_read_directly",
                        f.span,
                        format!(
                            "the field `{}` reads the session cookie `{}` that the `SignedCookie` actor `{}` consumes",
                            f.name.name, seam.cookie, seam.actor
                        ),
                    )
                    .with_note(
                        "the signed value is verified at the boundary and never reaches the handler — read the identity from the `by` binder; the router also hides this cookie from a raw `RequestCookies` view",
                    ),
                );
            }
        }
    }
    for ann in handler
        .annotations
        .iter()
        .filter(|a| a.name.name == "cache")
    {
        let public = ann.args.iter().any(|a| {
            a.label.as_ref().is_some_and(|n| n.name == "scope")
                && matches!(&a.value.kind, ExprKind::Ident(id) if id.name == "public")
        });
        if public {
            errors.push(
                CompileError::new(
                    "bynk.actor.signed_cookie_public_cache",
                    ann.span,
                    "a `@cache(scope: public)` route is guarded by a `SignedCookie` actor, so a shared cache could replay one user's response to another",
                )
                .with_note("use `scope: private`, or drop `@cache`"),
            );
        }
    }
}

/// #3890: whether an actor declaration refines the prelude `Caller` (no local
/// actor named `Caller` shadows it).
pub(crate) fn is_caller_refinement(actor: &bynk_syntax::ast::ActorDecl, table: &UnitTable) -> bool {
    actor
        .refinement
        .as_ref()
        .is_some_and(|r| r.base.name == "Caller" && !table.actors.contains_key("Caller"))
}

/// The contract a `by` clause's actor resolves to: a local declaration (a refinement's contract is
/// its base's scheme, refinement elimination, v0.53; the invariant rides the seam, not the scheme;
/// a malformed refinement already errored at its decl) or a prelude actor. A refinement over a
/// prelude actor (`Caller` #3890, `Sender` #3888, `Producer` #3899) has that actor's contract.
fn by_actor_contract(
    name: &str,
    local: Option<&bynk_syntax::ast::ActorDecl>,
    table: &UnitTable,
) -> Option<crate::actors::Contract> {
    use crate::actors::{self, Scheme};
    let Some(a) = local else {
        return actors::prelude_actor(name);
    };
    if is_caller_refinement(a, table) {
        return actors::prelude_actor("Caller");
    }
    if is_sender_refinement(a, table) {
        return actors::prelude_actor("Sender");
    }
    if is_producer_refinement(a, table) {
        return actors::prelude_actor("Producer");
    }
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
}

/// #3888: whether an actor declaration refines the prelude `Sender` (no local
/// actor named `Sender` shadows it).
pub(crate) fn is_sender_refinement(actor: &bynk_syntax::ast::ActorDecl, table: &UnitTable) -> bool {
    actor
        .refinement
        .as_ref()
        .is_some_and(|r| r.base.name == "Sender" && !table.actors.contains_key("Sender"))
}

/// #3899: whether an actor declaration refines the prelude `Producer` (no local
/// actor named `Producer` shadows it).
pub(crate) fn is_producer_refinement(
    actor: &bynk_syntax::ast::ActorDecl,
    table: &UnitTable,
) -> bool {
    actor
        .refinement
        .as_ref()
        .is_some_and(|r| r.base.name == "Producer" && !table.actors.contains_key("Producer"))
}

/// #3899: a queue consumer may name a `Producer` refinement or a `Foreign` actor, and delivery
/// enforces either: the consumer admits only messages signed by one of them. A sum that also names
/// an unsigned actor, or a lone signed actor with a binder, is not enforced, so it is refused rather
/// than accepted unenforced.
fn actor_narrowing_refusal(
    protocol: &ServiceProtocol,
    by: &bynk_syntax::ast::ByClause,
    refinement: Option<(bool, bool)>,
    mixes_signed: bool,
) -> Option<CompileError> {
    let (producer_refinement, sender_refinement) = refinement.unwrap_or_default();
    // #3888: likewise a refinement over `Sender` narrows inbound mail, and only an email
    // handler can name one: elsewhere its `Dmarc(…)` is never checked.
    if sender_refinement && !matches!(protocol, ServiceProtocol::Email { .. }) {
        return Some(
            CompileError::new(
                "bynk.actor.scheme_not_admissible",
                by.span,
                format!(
                    "a refinement over `Sender` is not admissible on a `{}` handler",
                    protocol_label(protocol),
                ),
            )
            .with_note(
                "`Sender` refinements narrow which mail an email handler acts on — name one \
                 only on `on message` of a `from email` service",
            ),
        );
    }
    let on_queue = matches!(protocol, ServiceProtocol::Queue { .. });
    // A refinement over `Producer` narrows a queue's senders, and only a queue
    // consumer can name one; elsewhere nothing would enforce its `IsContext(…)`,
    // so it is refused rather than read as narrowed.
    if producer_refinement && !on_queue {
        return Some(
            CompileError::new(
                "bynk.actor.scheme_not_admissible",
                by.span,
                format!(
                    "a refinement over `Producer` is not admissible on a `{}` handler",
                    protocol_label(protocol),
                ),
            )
            .with_note(
                "`Producer` refinements narrow who may send to a queue consumer — name one \
                 only on `on message` of a `from queue` service",
            ),
        );
    }
    // #3899: a signed actor (a `Foreign` actor, or a `Producer` refinement) is enforced: the
    // consumer admits only messages signed by one. Beside an unsigned actor, or with a binder on a
    // lone one (nothing binds an identity for it), it is not.
    if !on_queue || !mixes_signed {
        return None;
    }
    Some(
        CompileError::new(
            "bynk.actor.queue_actor_not_enforced",
            by.span,
            "a queue consumer cannot narrow its actor this way yet: a `Producer` refinement or a `Foreign` actor beside an unsigned one, or with a binder, is not enforced on `on message`",
        )
        .with_note(
            "a signed message carries no identity beyond its actor, and an unsigned one carries \
             none to tell it from a signed one, so accepting this `by` would admit unsigned \
             messages while reading as if it did not — write `by Producer`, or name only \
             `Foreign` actors and `Producer` refinements, a lone one with no binder",
        ),
    )
}

/// #3899: whether `actor` is the first signed member (a `Foreign` actor or a `Producer` refinement)
/// of a `by` clause that names it in a form a queue consumer cannot enforce: a sum that also names an
/// unsigned actor (its messages carry no signature to tell them from the signed ones), or a single
/// signed actor with a binder (it identifies no one beyond itself, so nothing binds a value for the
/// body). A sum of only signed actors resolves by the message's signer, and a lone signed actor with
/// no binder needs nothing. Said once, on the first signed member, not on each.
fn by_mixes_signed(by: &bynk_syntax::ast::ByClause, table: &UnitTable, actor: &str) -> bool {
    use crate::actors::Scheme;
    let signed = |name: &str| {
        table
            .actors
            .get(name)
            .is_some_and(|a| is_producer_refinement(a, table))
            || by_actor_contract(name, table.actors.get(name), table)
                .is_some_and(|c| c.scheme == Scheme::Foreign)
    };
    let refused = if by.is_sum() {
        !by.actors.iter().all(|a| signed(&a.name))
    } else {
        by.binder.is_some()
    };
    refused
        && by
            .actors
            .iter()
            .find(|a| signed(&a.name))
            .is_some_and(|a| a.name == actor)
}

/// A handler with no `by` clause: edge protocols (HTTP, WebSocket) have no safe
/// default actor; the internal protocols inherit one.
fn check_missing_by(handler: &Handler, protocol: &ServiceProtocol, errors: &mut Vec<CompileError>) {
    if crate::actors::default_actor(protocol).is_some() {
        return;
    }
    if matches!(protocol, ServiceProtocol::Call) {
        // Only a real `on call` handler: a stray kind in a call service has its own error.
        if handler.kind == HandlerKind::Call {
            errors.push(missing_by_on_call(handler));
        }
        return;
    }
    // #3899: a queue consumer states who may send to it.
    if matches!(protocol, ServiceProtocol::Queue { .. }) {
        if handler.kind == HandlerKind::Message {
            errors.push(missing_by_on_queue(handler));
        }
        return;
    }
    // #3888: inbound mail states whether it must be authenticated.
    if matches!(protocol, ServiceProtocol::Email { .. }) {
        if handler.kind == HandlerKind::Message {
            errors.push(missing_by_on_email(handler));
        }
        return;
    }
    // v0.103 (D-A): a WebSocket upgrade authenticates at the edge before the
    // connection is accepted — `on open` must name its actor, no anonymous upgrade.
    let (msg, note) = match protocol {
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
        CompileError::new("bynk.actor.missing_by_on_http", handler.span, msg).with_note(note),
    );
}

/// Where a handler's `given` clause sits: after its return type, and after its
/// own `by` clause when it writes one (`-> T by A given C`). A `by` inherited
/// from a service header lies before the handler and does not move the anchor.
fn given_anchor_after_by(handler: &Handler) -> Span {
    let ret = handler.return_type.span();
    match &handler.by_clause {
        Some(by) if by.span.start >= ret.end => by.span,
        _ => ret,
    }
}

/// #3891: an `on call` handler states its actor, as an HTTP route does. The
/// fix-it inserts `by Caller` (the former silent default: any consuming
/// context) after the return type.
fn missing_by_on_call(handler: &Handler) -> CompileError {
    let at = handler.return_type.span().end;
    CompileError::new(
        "bynk.actor.missing_by_on_call",
        handler.span,
        "an `on call` handler must declare its actor with a `by` clause",
    )
    .with_note(
        "`by Caller` admits any context that consumes this one; a refinement or a `Foreign` actor narrows it. \
         Migrate a project with `bynk check --fix=bynk.actor.missing_by_on_call`",
    )
    .with_suggestion(
        "add `by Caller`",
        vec![(Span::new_in(handler.span.file, at, at), " by Caller".to_string())],
        Applicability::MachineApplicable,
    )
}

/// #3899: an `on message` handler states who may send to it. The fix-it inserts `by Producer`
/// (the former silent default: anyone on the account) after the return type.
fn missing_by_on_queue(handler: &Handler) -> CompileError {
    let at = handler.return_type.span().end;
    CompileError::new(
        "bynk.actor.missing_by_on_queue",
        handler.span,
        "an `on message` handler must declare its actor with a `by` clause",
    )
    .with_note(
        "`by Producer` admits any producer on the Cloudflare account, which is what a queue consumer did before the clause was required. \
         Migrate a project with `bynk check --fix=bynk.actor.missing_by_on_queue`",
    )
    .with_suggestion(
        "add `by Producer`",
        vec![(Span::new_in(handler.span.file, at, at), " by Producer".to_string())],
        Applicability::MachineApplicable,
    )
}

/// #3888: an email handler states what mail it acts on. The fix-it inserts `by Sender` (the former
/// silent default: any mail, whatever its DMARC verdict) after the return type.
fn missing_by_on_email(handler: &Handler) -> CompileError {
    let at = handler.return_type.span().end;
    CompileError::new(
        "bynk.actor.missing_by_on_email",
        handler.span,
        "an email `on message` handler must declare its actor with a `by` clause",
    )
    .with_note(
        "`by Sender` admits any mail, whatever its DMARC verdict, which is what an email handler did before the clause \
         was required. Migrate a project with `bynk check --fix=bynk.actor.missing_by_on_email`",
    )
    .with_suggestion(
        "add `by Sender`",
        vec![(Span::new_in(handler.span.file, at, at), " by Sender".to_string())],
        Applicability::MachineApplicable,
    )
}

/// Once per context, after every actor declaration is read: the checks on foreign callers' secrets.
fn check_foreign_secret_dupes(table: &UnitTable, errors: &mut Vec<CompileError>) {
    use crate::actors::Scheme;
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
}

/// A refinement actor's declaration (`actor Admin = <Base> where <pred>`): over
/// the prelude `Caller` (#3890) the predicate is `IsContext(…)`; over a declared
/// `Bearer` actor (v0.53) it is the closed claim-predicate set.
fn check_refinement_actor(
    actor: &bynk_syntax::ast::ActorDecl,
    table: &UnitTable,
    refs: &mut RefSink,
    errors: &mut Vec<CompileError>,
) {
    use crate::actors::{self, Scheme};
    // v0.53: a refinement actor (`actor Admin = User where <pred>`) carries
    // an authorisation invariant. Its base MUST be a declared `Bearer` actor
    // (only Bearer carries claims to authorise against), and its `where`
    // predicate MUST be in the closed claim-predicate set.
    if let Some(r) = &actor.refinement
        && is_sender_refinement(actor, table)
    {
        // #3888: a refinement over `Sender` narrows which mail is admitted, by
        // the closed `Dmarc(…)` vocabulary.
        match actors::parse_sender_predicate(&r.predicate) {
            Ok(pred) if pred.admitted().is_empty() => errors.push(
                CompileError::new(
                    "bynk.actor.sender_admits_nothing",
                    r.predicate.span,
                    format!(
                        "the refinement `{}` admits no DMARC verdict, so every message would be refused",
                        actor.name.name
                    ),
                )
                .with_note(
                    "the predicate is false for `DmarcPass`, `DmarcFail` and `DmarcNone` alike — \
                     check the `&&` and `!` it combines",
                ),
            ),
            Ok(_) => {}
            Err(span) => errors.push(
                CompileError::new(
                    "bynk.actor.refinement_predicate_unsupported",
                    span,
                    "a refinement over `Sender` must be `Dmarc(<DmarcVerdict>, …)`, composed with `&&`, `||`, `!`",
                )
                .with_note(
                    "the DMARC verdict is the only thing a `Sender` refinement tests: name \
                     `DmarcPass`, `DmarcFail` or `DmarcNone`",
                ),
            ),
        }
        return;
    }
    if let Some(r) = &actor.refinement
        && (is_caller_refinement(actor, table) || is_producer_refinement(actor, table))
    {
        // #3890: a refinement over the prelude `Caller` narrows which
        // contexts may call, by the closed `IsContext(…)` vocabulary; #3899
        // gives a refinement over `Producer` the same vocabulary.
        if let Err(span) = actors::parse_caller_predicate(&r.predicate) {
            errors.push(
                    CompileError::new(
                        "bynk.actor.refinement_predicate_unsupported",
                        span,
                        format!(
                            "a refinement over `{}` must be `IsContext(<context>, …)`, composed with `&&`, `||`, `!`",
                            r.base.name
                        ),
                    )
                    .with_note(
                        "the context is the only thing a `Caller` or `Producer` carries; claims such as \
                         `hasClaim` belong to a `Bearer` actor",
                    ),
                );
        }
        return;
    }
    if let Some(r) = &actor.refinement {
        let base = table.actors.get(&r.base.name);
        let base_is_bearer = base.is_some_and(|b| {
            b.refinement.is_none()
                && b.auth.as_ref().and_then(|a| Scheme::from_name(&a.name)) == Some(Scheme::Bearer)
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
                         slice; a general typed-claims surface is a later slice (`IsContext(…)` \
                         belongs to a refinement over `Caller`)",
                    ),
                );
        }
    }
}

fn check_actor_contracts(
    table: &UnitTable,
    resolved: &ResolvedCommons,
    refs: &mut RefSink,
    errors: &mut Vec<CompileError>,
) {
    use crate::actors::Scheme;

    // Pass 1 — actor declaration well-formedness.
    for actor in table.actors.values() {
        refs.set_owner(&actor.name.name);
        if actor.refinement.is_some() {
            check_refinement_actor(actor, table, refs, errors);
            continue;
        }
        let Some(auth) = &actor.auth else {
            continue;
        };
        check_scheme_arg_kinds(actor, errors);
        match Scheme::from_name(&auth.name) {
            None => errors.push(
                CompileError::new(
                    "bynk.actor.unknown_scheme",
                    auth.span,
                    format!("unknown authentication scheme `{}`", auth.name),
                )
                .with_note(
                    "the authentication schemes are `None`, `Internal`, `Bearer`, `Signature`, `Oidc`, `Foreign`, and `SignedCookie`",
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
                check_bearer_actor(actor, auth, table, errors);
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
            // #3386: a signed session cookie.
            Some(Scheme::SignedCookie) => {
                check_signed_cookie_actor(actor, auth, table, &resolved.types, errors);
            }
            // #3939: an API key actor.
            Some(Scheme::ApiKey) => {
                crate::api_key::check_api_key_actor(actor, auth, table, resolved, errors);
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

    check_foreign_secret_dupes(table, errors);

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
                    check_signed_cookie_route(handler, table, &resolved.types, errors);
                }
                // #2341 (decision C): `on rejected` has no actor; its own
                // checks (`check_rejected_handlers`) refuse a `by`.
                _ if handler.kind == HandlerKind::Rejected => {}
                None => check_missing_by(handler, &service.protocol, errors),
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

/// #3909 (decision B): a route no request can reach is an error. Within a context,
/// every path the route matches is matched first by other routes of its method
/// (see [`crate::route_order`]); overlap alone is not an error.
fn check_unreachable_routes(table: &UnitTable, errors: &mut Vec<CompileError>) {
    let mut routes: Vec<(HttpMethod, &str, Span)> = Vec::new();
    for service in table.services.values() {
        for handler in &service.handlers {
            if let HandlerKind::Http { method, path } = &handler.kind {
                routes.push((*method, path.as_str(), handler.span));
            }
        }
    }
    let keys: Vec<(&str, &str)> = routes.iter().map(|(m, p, _)| (m.as_str(), *p)).collect();
    // #3752 (D5): an `ANY` route is beaten by an ordinary route of any method.
    let found = crate::route_order::shadowed(&keys)
        .into_iter()
        .chain(crate::route_order::pass_through_shadowed(&keys));
    for (r, covering) in found {
        let (method, path, span) = routes[r];
        let mut err = CompileError::new(
            "bynk.http.unreachable_route",
            span,
            format!(
                "route `{} {path}` can never be reached: every path it matches is answered first by another route",
                method.as_str()
            ),
        )
        .with_note(
            "at the first segment where two routes differ, a literal segment beats a `:name` one, and a `:name` beats a `*name` rest; remove this route, or make it match a path the others do not",
        );
        for c in covering {
            err = err.with_label(
                routes[c].2,
                format!(
                    "`{} {}` answers it first",
                    routes[c].0.as_str(),
                    routes[c].1
                ),
            );
        }
        errors.push(err);
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
    crate::email::check_service_protocols_and_email(table, &typed.types, errors, tys);

    // v0.45: actor-contract well-formedness and the handler `by`-clause checks.
    check_actor_contracts(table, resolved, refs, errors);

    // #2341 (decision C): the shape of each service's `on rejected`.
    check_rejected_handlers(table, errors);

    // #3343: `@name("…")` on the fields of a request record.
    validate_field_names(table, &typed.types, errors);

    let cf_is_std = cf_type_is_std(cross_context, table);
    // v0.9: validate HTTP handler shape and check for duplicate routes
    // across all services in this context.
    let mut route_first_span: HashMap<(HttpMethod, String), Span> = HashMap::new();
    for service in table.services.values() {
        for handler in &service.handlers {
            let HandlerKind::Http { method, path } = &handler.kind else {
                continue;
            };
            validate_http_handler(handler, *method, path, &typed.types, cf_is_std, errors);
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

    check_unreachable_routes(table, errors);
    crate::http_rest::check_rest_routes(table, &typed.stability, errors);
    check_pass_through_routes(table, &typed.types, &typed.stability, errors);

    // v0.140 (ADR 0163): validate handler-position annotations (`@cache`) across
    // every handler — services and agents — so a misplaced annotation is caught
    // wherever it is written, not only on well-formed HTTP routes.
    for service in table.services.values() {
        let service_cap = service.limits.as_ref().and_then(|p| p.max_body());
        for handler in &service.handlers {
            let kinds = route_actor_kinds(handler, service, table);
            validate_handler_annotations(handler, service_cap, kinds, errors);
        }
    }
    crate::http_rate_limit::validate_rate_limits(table, errors);
    note_defaulted_body_caps(table, errors);
    for agent in table.agents.values() {
        crate::agent_busy::validate_agent_annotations(agent, errors);
        let handlers = agent.handlers.iter();
        handlers.for_each(|h| validate_handler_annotations(h, None, (false, false), errors));
    }

    // v0.131 (ADR 0159): validate each service's `cors { }` policy.
    for service in table.services.values() {
        validate_service_cors(service, table, errors);
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

    // #3877: validate each service's `delivery { }` policy, and warn of a queue
    // consumer with no dead-letter queue.
    for service in table.services.values() {
        crate::queue_delivery::validate_delivery(service, errors);
    }

    validate_error_policies(table, errors);
    validate_consumed_headers(table, &typed.types, errors);

    // #2858: validate each service's `json { }` policy, then refuse the one
    // ambiguous shape (`Option[Option[T]]`) in any plain-mode route's response.
    for service in table.services.values() {
        if let Some(policy) = &service.json {
            validate_json_policy(service, policy, errors);
        }
        if !matches!(service.protocol, ServiceProtocol::Http) {
            continue;
        }
        for handler in &service.handlers {
            if !matches!(handler.kind, HandlerKind::Http { .. })
                || effective_json_options(service.json.as_ref(), handler) != JsonOptions::Plain
            {
                continue;
            }
            let TypeRef::Effect(inner, _) = &handler.return_type else {
                continue;
            };
            let TypeRef::HttpResult(body, _, _) = inner.as_ref() else {
                continue;
            };
            if let Some(payload) = null_payload_option(body, &resolved.types, &mut HashSet::new()) {
                errors.push(
                    CompileError::new(
                        "bynk.json.nested_option_plain",
                        body.span(),
                        format!(
                            "`Option[{payload}]`: `Some({some})` and `None` would both write `null` in plain JSON",
                            some = if payload == "()" { "()" } else { "…" },
                        ),
                    )
                    .with_note(
                        "an `Option` payload that can itself be `null` (`Option[T]` or `()`) is ambiguous; model it as a sum type, or keep this route tagged with `@json(options: tagged)`",
                    ),
                );
            }
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
            validate_cron_handler(handler, expr, table, errors);
            check_duplicate_schedule(handler, expr, &mut schedule_first_span, errors);
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
            validate_queue_handler(handler, name, table, errors);
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
            let borrowed_held = borrowed_connection(handler, is_ws_lifecycle);
            checker::check_handler_body(
                resolved,
                checker::HandlerBodyCheck {
                    capabilities: handler_caps,
                    declared_capabilities: capability_info_map.clone(),
                    given_anchor: Some(given_anchor_after_by(handler)),
                    report_unused: true,
                    actor_binding,
                    borrowed_held,
                    // #2683: a WebSocket handler runs in the connection-hosting
                    // Durable Object, whose `deps` carry no execution context.
                    service_handler: !is_ws_lifecycle,
                    http_non_get: is_non_get_http(handler),
                    ..checker::HandlerBodyCheck::new(
                        &handler.body,
                        &handler.return_type,
                        &params_for_check,
                        &handler.given,
                    )
                    .with_claims_of(handler, &table.actors)
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
            // #3890: a refinement over the prelude `Caller` yields a `CallerId`.
            // #3899: and one over `Producer` the context that signed the message, as a sum member
            // carries it.
            if is_caller_refinement(local, table) || is_producer_refinement(local, table) {
                return tys.intern(checker::Ty::Base(bynk_syntax::ast::BaseType::String));
            }
            // #3888: a refinement over the prelude `Sender` yields its identity.
            if is_sender_refinement(local, table) {
                return actor_identity_ty_guarded("Sender", table, resolved, seen, tys);
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
        // #3328: `Sender`'s `SenderIdentity` is minted by the Workers `email()`
        // entry. It is the first-party record from `bynk.cloudflare`, so it types
        // as that record when the context imports it (`consumes bynk.cloudflare
        // { SenderIdentity, … }`) and as the unit value when it does not, as the
        // other platform-tag actors read.
        Some(Identity::Declared(name)) => checker::resolve_type_ref(
            &bynk_syntax::ast::TypeRef::Named(bynk_syntax::ast::Ident {
                name,
                span: bynk_syntax::span::Span::default(),
            }),
            &resolved.types,
            tys,
        )
        .unwrap_or_else(|| tys.intern(checker::Ty::Unit)),
        _ => tys.intern(checker::Ty::Unit),
    }
}

/// The closed storage-kind catalogue (design notes §10). Every kind is functional
/// but `Queue`, which parses and validates as a known kind and is refused
/// (`bynk.store.kind_unsupported`).
const STORAGE_KINDS: &[&str] = &["Cell", "Map", "Set", "Log", "Queue", "Cache"];

/// The closed storage-annotation registry (ADR 0111 D2/D3): each `@name` with the
/// storage kind(s) it attaches to. `functional` is whether anything acts on the
/// annotation: it is `true` for `@ttl`, `@retain` and `@indexed`, and `false`
/// only for `@bounded`, which is recognised and refused
/// (`bynk.store.annotation_unsupported`) until it has a meaning.
struct AnnotationSpec {
    name: &'static str,
    kinds: &'static [&'static str],
    functional: bool,
}

const ANNOTATIONS: &[AnnotationSpec] = &[
    AnnotationSpec {
        name: "ttl",
        kinds: &["Cache"],
        functional: true,
    },
    AnnotationSpec {
        name: "retain",
        kinds: &["Log"],
        functional: true,
    },
    AnnotationSpec {
        name: "indexed",
        kinds: &["Map"],
        functional: true,
    },
    // #4038: a value may appear on at most one entry; implies `@indexed(by:)` on the same field.
    AnnotationSpec {
        name: "unique",
        kinds: &["Map"],
        functional: true,
    },
    AnnotationSpec {
        name: "bounded",
        kinds: &["Queue", "Log"],
        functional: false,
    },
];

/// Validate a `store` field's annotations against the closed registry (ADR 0111):
/// an unknown name is `bynk.store.unknown_annotation`; a known name on the wrong
/// kind is `bynk.store.annotation_kind_mismatch`; a known name on the right kind
/// that nothing acts on yet is `bynk.store.annotation_unsupported`. `head` is
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
                    format!("`@{name}` is recognised but has no effect yet"),
                )
                .with_note(if head == "Log" {
                    "leave the annotation off; to limit a `Log`, use `@retain`, which drops \
                     entries older than a period"
                } else {
                    "leave the annotation off"
                }),
            );
            continue;
        }
        // v0.93 (ADR 0118): `@indexed(by: k, …)` — each `by:` names a
        // **value-keyable field of the map's value type** to maintain a secondary
        // index on. Validate the keys here, now the kind/value type are known.
        if name == "indexed" || name == "unique" {
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
    let name = ann.name.name.as_str();
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
    // #4038: `@unique` is a constraint, so a form that would silently enforce nothing, or something other
    // than what it reads as, is refused: a bare one has no field, and a second `by:` would make each
    // field unique alone, not the pair.
    if name == "unique" && ann.args.is_empty() {
        errors.push(CompileError::new(
            "bynk.index.bad_argument",
            ann.span,
            "`@unique` needs a field: `@unique(by: <field>)`",
        ));
    }
    if name == "unique" && ann.args.len() > 1 {
        errors.push(
            CompileError::new(
                "bynk.index.bad_argument",
                ann.args[1].span,
                "`@unique` takes one `by:` field",
            )
            .with_note("each `by:` would make its field unique on its own, not the pair; a constraint over several fields is not supported"),
        );
    }
    for arg in &ann.args {
        // Only `by:` labels are admitted on `@indexed` and `@unique`.
        let Some(label) = &arg.label else {
            errors.push(CompileError::new(
                "bynk.index.bad_argument",
                arg.span,
                format!("`@{name}` arguments are `by: <field>` labels naming a field to index on"),
            ));
            continue;
        };
        if label.name != "by" {
            errors.push(CompileError::new(
                "bynk.index.bad_argument",
                arg.span,
                format!("`@{name}` takes `by:` arguments, not `{}:`", label.name),
            ));
            continue;
        }
        let ExprKind::Ident(key) = &arg.value.kind else {
            errors.push(CompileError::new(
                "bynk.index.bad_argument",
                arg.value.span,
                format!("`@{name}(by: …)` names a field of the map's value type"),
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
                        "`@{name}(by: {0})` — the map's value type has no field `{0}`",
                        key.name
                    ),
                ));
            }
            Some(field)
                if !type_ref_is_scalar_store_key(&field.type_ref, types)
                    && let Some(blocker) =
                        crate::checker::keying::typeref_not_key(&field.type_ref, &[], types) =>
            {
                errors.push(
                    CompileError::new(
                        "bynk.index.unkeyable_key",
                        arg.value.span,
                        format!(
                            "`@{name}(by: {0})` — field `{0}` is not `Key`, and `{1}` is not",
                            key.name, blocker.part
                        ),
                    )
                    .with_note(crate::checker::keying::key_note(&blocker)),
                );
            }
            Some(_) => {}
        }
    }
}

/// A store `Set`'s element and a store `Map`'s or `Cache`'s key must be `Key`, the rule value `Map`
/// keys already follow (#4037). A `String`, an `Int` or a refined or opaque type over one keeps
/// the stored form it always had (`String(k)`); any other `Key` type (a record, a sum, an
/// `Option`, a `Decimal`, `Bytes`, `Bool`, an `Instant`) is stored under its canonical encoding
/// (`__bynkKeyEncode`). A name that doesn't resolve is reported by the resolver, so it is skipped
/// here.
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
    if type_ref_is_scalar_store_key(t, types) {
        return;
    }
    if let Some(blocker) = crate::checker::keying::typeref_not_key(t, &[], types) {
        errors.push(
            CompileError::new(
                "bynk.store.unkeyable_key",
                t.span(),
                format!("{what} must be `Key`, and `{}` is not", blocker.part),
            )
            .with_note(crate::checker::keying::key_note(&blocker)),
        );
    }
}

/// Whether a `TypeRef` is a *scalar* store key (ADR 0110 D5): `Int`/`String`, including a
/// refined/opaque named type over them. Such a key persists as `String(k)`, as it always has;
/// every other `Key` type persists under its canonical encoding (#4037).
pub fn type_ref_is_scalar_store_key(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
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
    // #4038: the (map, field) pairs under `@unique(by:)`, which maintains its index itself.
    let mut unique: HashSet<(String, String)> = HashSet::new();
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
        for arg in f
            .annotations
            .iter()
            .filter(|a| a.name.name == "unique")
            .flat_map(|a| &a.args)
        {
            if let ExprKind::Ident(k) = &arg.value.kind {
                unique.insert((f.name.name.clone(), k.name.clone()));
            }
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
                .is_some_and(|v| v.iter().any(|(f, _)| f == field))
                || unique.contains(&(map.to_string(), field.to_string()));
            if is_declared {
                return;
            }
            let keyable = value_fields.get(map).is_some_and(|fs| {
                fs.iter().any(|rf| {
                    rf.name.name == field && type_ref_is_scalar_store_key(&rf.type_ref, types)
                })
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
            if unique.contains(&(map.clone(), field.clone())) {
                errors.push(
                    CompileError::new(
                        "bynk.store.unique_implies_indexed",
                        *span,
                        format!(
                            "`@unique(by: {field})` on `{map}` already maintains the index `@indexed(by: {field})` declares"
                        ),
                    )
                    .with_note("remove the `@indexed`: a unique field is indexed, and its equality filters route through the unique index"),
                );
                continue;
            }
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

/// Finding #36: `check_handler_body` takes the five kind-scopes as one
/// `HashMap<String, StoreField>` — a field name is only ever one kind, so
/// recombine them here rather than threading five parallel maps.
fn combine_store_fields(
    store_cells: &HashMap<String, TyId>,
    store_maps: &HashMap<String, (TyId, TyId)>,
    store_sets: &HashMap<String, TyId>,
    store_caches: &HashMap<String, (TyId, TyId, i64)>,
    store_logs: &HashMap<String, TyId>,
) -> HashMap<String, checker::StoreField> {
    store_cells
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
        .chain(
            store_caches.iter().map(|(name, (k, v, ttl))| {
                (name.clone(), checker::StoreField::Cache(*k, *v, *ttl))
            }),
        )
        .chain(
            store_logs
                .iter()
                .map(|(name, t)| (name.clone(), checker::StoreField::Log(*t))),
        )
        .collect()
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
                            "storage kind `{other}` is not yet supported — use `Cell`, `Map`, \
                             `Set`, `Cache` or `Log`"
                        ),
                    )
                    .with_note("`Queue` is the one storage kind with no implementation yet"),
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
                    "`@ttl`'s argument must be a duration literal, e.g. `5.minutes`, or a `Duration` constant bound to one",
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
                annotations: Vec::new(),
                replaces: None,
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
            annotations: Vec::new(),
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
        .with_local_fns(&table.fns)
        // #1883, slice 1a: a handler body sees the unit's assets too.
        .with_assets(typed.assets.clone())
        .with_embeds(typed.embeds.clone())
        .with_consts(typed.consts.clone())
        .with_shadowed_imports(typed.shadowed_imports.clone())
        .with_scope_facts_of(typed)
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
            } else if !checker::is_zeroable(elem, None, &typed.types) {
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
            annotations: Vec::new(),
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
                    annotations: Vec::new(),
                    replaces: None,
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
        .with_local_fns(&table.fns)
        // #1883, slice 1a: a handler body sees the unit's assets too.
        .with_assets(typed.assets.clone())
        .with_embeds(typed.embeds.clone())
        .with_consts(typed.consts.clone())
        .with_shadowed_imports(typed.shadowed_imports.clone())
        .with_scope_facts_of(typed)
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
        // `HashMap<String, StoreField>` — see [`combine_store_fields`].
        let store_fields = combine_store_fields(
            &store_cells,
            &store_maps,
            &store_sets,
            &store_caches,
            &store_logs,
        );

        // v0.80/v0.81: invariant well-formedness — predicates are pure `Bool`
        // expressions over the agent's `store` cells (§14, ADR 0108 D5).
        let collections = checker::PredicateCollections::of(&store_fields);
        checker::check_invariants(
            &agent.invariants,
            &store_cells,
            &collections,
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
            &collections,
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
                if let Err(reason) = crate::wire_default::lower_event_field_default_wire(
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

/// A service's CORS checks: its `cors { }` policy (ADR 0159) and, #3329, the
/// cross-site cookie warning that depends on it.
fn validate_service_cors(service: &ServiceDecl, table: &UnitTable, errors: &mut Vec<CompileError>) {
    if let Some(policy) = &service.cors {
        validate_cors_policy(service, policy, errors);
    }
    check_ws_cookie_origins(service, table, errors);
    warn_cross_site_cookie_without_credentials(service, errors);
    warn_issue_on_safe_method(service, table, errors);
}

/// #3904 (`bynk.ws.cookie_needs_origins`): a `from websocket` service whose `on open` authenticates a
/// `SignedCookie` actor must declare `cors { origins: [...], credentials: true }`. A browser sends the
/// cookie on a cross-site upgrade by itself and WebSockets are not covered by CORS, so the upgrade
/// checks `Origin` against exactly these origins (cross-site WebSocket hijacking); without a list
/// there is nothing to check it against, and a wildcard would admit every site.
fn check_ws_cookie_origins(
    service: &ServiceDecl,
    table: &UnitTable,
    errors: &mut Vec<CompileError>,
) {
    if !matches!(service.protocol, ServiceProtocol::WebSocket { .. }) {
        return;
    }
    let Some(open) = service.handlers.iter().find(|h| {
        matches!(h.kind, HandlerKind::Open)
            && crate::actors::signed_cookie_seam_for(h, &table.actors).is_some()
    }) else {
        // Only a cookie upgrade checks `Origin`, so a `cors` policy on any other WebSocket service
        // would read as an allow-list and restrict nothing: refuse it.
        if let Some(policy) = &service.cors {
            errors.push(
                CompileError::new(
                    "bynk.http.cors_not_http",
                    policy.span,
                    "a `cors { }` policy on a `from websocket` service takes effect only when its `on open` is `by` a `SignedCookie` actor",
                )
                .with_note(
                    "the upgrade checks `Origin` against `origins` only for a cookie, which a browser sends from any site; a `Bearer` or `Oidc` token is offered by the page itself, so there is no origin list to apply",
                ),
            );
        }
        return;
    };
    // Of the policy's fields only `origins` and `credentials` apply to an upgrade.
    for field in service.cors.iter().flat_map(|c| &c.fields) {
        if matches!(field.name.name.as_str(), "headers" | "expose" | "maxAge") {
            errors.push(
                CompileError::new(
                    "bynk.http.cors_unknown_field",
                    field.name.span,
                    format!(
                        "the `cors` field `{}` has no effect on a `from websocket` service",
                        field.name.name
                    ),
                )
                .with_note("only `origins` and `credentials` apply to a WebSocket upgrade"),
            );
        }
    }
    // The session-live check of a `sessions = true` actor is not made on an upgrade yet, so a revoked
    // session could still open a socket: refuse the combination rather than accept it quietly.
    if crate::actors::signed_cookie_seam_for(open, &table.actors).is_some_and(|s| s.sessions) {
        errors.push(
            CompileError::new(
                "bynk.actor.scheme_not_admissible",
                open.span,
                "a `SignedCookie` actor with `sessions = true` is not admissible on a WebSocket `on open`",
            )
            .with_note(
                "the upgrade does not check the session is still live, so a revoked session could open a socket; use a `SignedCookie` actor without `sessions`, or a `Bearer` actor",
            ),
        );
    }
    let ok = service
        .cors
        .as_ref()
        .is_some_and(|c| c.credentials() && !c.is_wildcard() && !c.origins().is_empty());
    if !ok {
        errors.push(
            CompileError::new(
                "bynk.ws.cookie_needs_origins",
                service.cors.as_ref().map_or(open.span, |c| c.span),
                format!(
                    "the `from websocket` service `{}` authenticates a `SignedCookie`, so it must declare `cors {{ origins: [...], credentials: true }}` with the exact origins that may open it",
                    service.name.name
                ),
            )
            .with_note(
                "a browser sends the cookie on a cross-site upgrade by itself and a WebSocket is not covered by CORS, so the upgrade is refused with `403` unless its `Origin` is exactly one of these origins; a wildcard would admit every site",
            ),
        );
    }
}

/// #3825 (`bynk.actor.signed_cookie_issue_on_safe_method`, a warning): a `GET` that issues a session. A
/// state-changing route that issues one is checked for login CSRF by fetch metadata and the service's
/// `cors` origins, which a `GET` is outside of (a link click, a prefetch, an `<img>` all send one), so a
/// sign-in done as a `GET` is unprotected. Steer the author to `POST`.
fn warn_issue_on_safe_method(
    service: &ServiceDecl,
    table: &UnitTable,
    errors: &mut Vec<CompileError>,
) {
    for handler in &service.handlers {
        if !matches!(
            handler.kind,
            HandlerKind::Http {
                method: bynk_syntax::ast::HttpMethod::Get,
                ..
            }
        ) {
            continue;
        }
        // A `GET` guarded by a `SignedCookie` actor (a sliding session that re-issues its caller's own
        // identity) has nothing to plant: the caller already has the session.
        if crate::actors::signed_cookie_seam_for(handler, &table.actors).is_some() {
            continue;
        }
        for actor in crate::actors::issued_cookie_actors(handler, &table.actors) {
            errors.push(
                CompileError::new(
                    "bynk.actor.signed_cookie_issue_on_safe_method",
                    handler.span,
                    format!("this `GET` route issues a `{actor}` session, so it is not checked for login CSRF"),
                )
                .with_note(
                    "a route that issues a session and is not a `GET` is refused when its request is cross-site (by fetch metadata, then `Origin` against the service's `cors` origins); a `GET` is outside that check, so a hostile page can make a signed-out visitor's browser sign in as someone else. Issue the session from a `POST`",
                ),
            );
        }
    }
}

/// #3329 (`bynk.http.cookie_cross_site_without_credentials`, a warning): a
/// `from http` service whose handlers attach a `Cookie { … sameSite:
/// SameSite.Unrestricted … }` (`withCookie`/`clearCookie`) but whose `cors`
/// policy lacks `credentials: true` (or that has none). The browser withholds
/// the cookie from credential-less cross-site requests, so it can never return.
fn warn_cross_site_cookie_without_credentials(
    service: &ServiceDecl,
    errors: &mut Vec<CompileError>,
) {
    if !matches!(service.protocol, ServiceProtocol::Http) {
        return;
    }
    if service.cors.as_ref().is_some_and(|c| c.credentials()) {
        return;
    }
    let mut hit: Option<Span> = None;
    for handler in &service.handlers {
        let mut exprs: Vec<&Expr> = Vec::new();
        for s in &handler.body.statements {
            bynk_syntax::ast::statement_exprs(s, &mut exprs);
        }
        exprs.push(&handler.body.tail);
        let mut visit = |e: &Expr| {
            if hit.is_some() {
                return;
            }
            let ExprKind::MethodCall { method, args, .. } = &e.kind else {
                return;
            };
            if method.name != "withCookie" && method.name != "clearCookie" {
                return;
            }
            let Some(arg) = args.first() else { return };
            let ExprKind::RecordConstruction {
                type_name, fields, ..
            } = &arg.kind
            else {
                return;
            };
            let unrestricted = type_name.name == "Cookie" && fields.iter().any(|f| {
                f.name.name == "sameSite"
                    && matches!(
                        f.value.as_ref().map(|v| &v.kind),
                        Some(ExprKind::FieldAccess { field, .. }) if field.name == "Unrestricted"
                    )
            });
            if unrestricted {
                hit = Some(arg.span);
            }
        };
        fn walk(e: &Expr, f: &mut dyn FnMut(&Expr)) {
            f(e);
            for c in bynk_syntax::ast::expr_children(e) {
                walk(c, f);
            }
        }
        for e in exprs {
            walk(e, &mut visit);
        }
        if hit.is_some() {
            break;
        }
    }
    let Some(span) = hit else { return };
    errors.push(
        CompileError::new(
            "bynk.http.cookie_cross_site_without_credentials",
            span,
            format!(
                "service `{}` sets a `SameSite.Unrestricted` cookie but does not declare `cors {{ credentials: true }}`, so a cross-origin `fetch` can never send it back",
                service.name.name
            ),
        )
        .with_note(
            "add `cors { origins: [...], credentials: true }` if a page on another origin must send it back; a cookie used only in cross-site navigations, embeds or SSO callbacks needs no `cors`, and `SameSite.Lax` suits one that need not cross sites",
        ),
    );
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
    // #3904: a `from websocket` service takes one too, for its upgrade's `Origin` check.
    if !matches!(
        service.protocol,
        ServiceProtocol::Http | ServiceProtocol::WebSocket { .. }
    ) {
        errors.push(
            CompileError::new(
                "bynk.http.cors_not_http",
                policy.span,
                "a `cors { }` policy is only valid on a `from http` or `from websocket` service",
            )
            .with_note("CORS governs cross-origin browser access, which only the HTTP and WebSocket surfaces have"),
        );
        return;
    }

    // Field names are a closed set; flag anything else (the parser accepts any
    // name, per the annotation precedent).
    for field in &policy.fields {
        if !matches!(
            field.name.name.as_str(),
            "origins" | "headers" | "expose" | "credentials" | "maxAge"
        ) {
            errors.push(
                CompileError::new(
                    "bynk.http.cors_unknown_field",
                    field.name.span,
                    format!("unknown `cors` field `{}`", field.name.name),
                )
                .with_note(
                    "known fields are `origins`, `headers`, `expose`, `credentials`, and `maxAge`",
                ),
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
            ExprKind::ListLit(items, ..) if !items.is_empty() => {
                for item in items {
                    match &item.kind {
                        // #4504: a literal a browser can never send matches nothing.
                        ExprKind::StrLit(origin) => validate_cors_origin_shape(origin, item.span, errors),
                        _ => errors.push(CompileError::new(
                            "bynk.http.cors_invalid_origins",
                            item.span,
                            "each `cors` origin must be a string literal or a `String` constant (e.g. \"https://app.example.com\" or \"*\")",
                        )),
                    }
                }
            }
            // A bare name that did not resolve (ADR 0826): the pass left it as
            // written, so say what the constant must be rather than repeat the
            // rule a reader may think it already meets.
            ExprKind::Ident(id) => errors.push(
                CompileError::new(
                    "bynk.http.cors_invalid_origins",
                    expr.span,
                    format!(
                        "`{}` does not name a `List[String]` constant bound to a list of string literals",
                        id.name
                    ),
                )
                .with_note(
                    "a constant counts as its literal only when it is declared exactly `List[String]` \
                     and bound to a list of string literals, or to another such constant (an \
                     `env(…)` constant is, once `bynk.toml` gives it a value)",
                ),
            ),
            _ => errors.push(CompileError::new(
                "bynk.http.cors_invalid_origins",
                expr.span,
                "`cors` `origins` must be a non-empty list of string literals or `String` constants, or a `List[String]` constant",
            )),
        },
    }

    // `headers`, when present, is a list of string literals.
    if let Some(expr) = policy.field("headers") {
        let ok = matches!(&expr.kind, ExprKind::ListLit(items, ..)
            if items.iter().all(|i| matches!(i.kind, ExprKind::StrLit(_))));
        if !ok {
            errors.push(CompileError::new(
                "bynk.http.cors_invalid_field",
                expr.span,
                "`cors` `headers` must be a list of string literals",
            ));
        }
    }

    // #3925: `expose`, when present, is a list of header names (RFC 9110 tokens),
    // or the wildcard `["*"]` alone.
    validate_cors_expose(policy, errors);

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
            "`cors` `maxAge` must be a `Duration` literal (e.g. `1.hour`)",
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
    refuse_null_origin_with_credentials(policy, errors);
}

/// #4504: `"null"` is the `Origin` a browser sends from a sandboxed frame or a `data:` page, which
/// any other site can create, so naming it is as open as the wildcard. Beside `credentials: true`
/// it would let any page act with the visitor's cookies and `Authorization`, so it is refused as
/// the wildcard is. Without credentials it is compared like any other entry.
fn refuse_null_origin_with_credentials(policy: &CorsPolicy, errors: &mut Vec<CompileError>) {
    if !policy.credentials() {
        return;
    }
    let Some(ExprKind::ListLit(items, ..)) = policy.field("origins").map(|e| &e.kind) else {
        return;
    };
    for item in items {
        if matches!(&item.kind, ExprKind::StrLit(origin) if origin == "null") {
            errors.push(
                CompileError::new(
                    "bynk.http.cors_wildcard_credentials",
                    item.span,
                    "`cors` cannot combine `credentials: true` with the origin `\"null\"`",
                )
                .with_note(
                    "a browser sends `Origin: null` from a sandboxed frame or a `data:` page, \
                     which any other site can create, so allowing it with credentials lets any \
                     page act with the visitor's cookies; list the exact origins instead, or \
                     set `credentials: false`",
                ),
            );
        }
    }
}

/// #4504: one literal `origins` entry. The router compares a request's `Origin` with it
/// exactly, so an entry that is not in the form a browser sends (`scheme://host[:port]`, lower
/// case, no path or trailing slash) matches nothing and every cross-origin request from that
/// site is refused. An entry that named a constant was rewritten to its value before this runs,
/// an `env(…)` constant to the value `bynk.toml` gives the environment being built.
fn validate_cors_origin_shape(origin: &str, span: Span, errors: &mut Vec<CompileError>) {
    let Some(problem) = crate::cors_origin::check(origin) else {
        return;
    };
    let mut error = CompileError::new(
        "bynk.http.cors_invalid_origins",
        span,
        format!("`cors` origin `{origin}` {}", problem.reason),
    )
    .with_note(
        "the router compares the request's `Origin` with each entry exactly, and a browser sends \
         `scheme://host[:port]` with the scheme and host in lower case, no path, query, \
         fragment or trailing slash, and no default port; any other entry matches nothing, so \
         every request from that site is refused",
    );
    if let Some(fixed) = problem.suggestion {
        error = error.with_note(format!("write `{fixed}`"));
        if problem.scheme_assumed {
            error = error.with_note(
                "the entry has no scheme, so `https` is assumed; use the scheme the site is \
                 served on (a browser sends `http://…` from a page served over `http`)",
            );
        }
    }
    errors.push(error);
}

/// #3925: the `expose` list: header names that are RFC 9110 tokens, or `["*"]`
/// alone (a `*` beside names is not the wildcard the Fetch standard defines).
fn validate_cors_expose(policy: &CorsPolicy, errors: &mut Vec<CompileError>) {
    let Some(expr) = policy.field("expose") else {
        return;
    };
    // The Fetch standard ignores the wildcard on a credentialed response, so a
    // `["*"]` that looks like it exposes everything would expose nothing.
    if policy.credentials() && policy.is_wildcard_expose() {
        errors.push(
            CompileError::new(
                "bynk.http.cors_wildcard_expose_credentials",
                expr.span,
                "`cors` cannot combine `credentials: true` with `expose: [\"*\"]`",
            )
            .with_note(
                "on a credentialed response a browser reads `*` as a header literally named `*`, \
                 not as every header; list the header names instead",
            ),
        );
    }
    let ExprKind::ListLit(items, ..) = &expr.kind else {
        errors.push(CompileError::new(
            "bynk.http.cors_invalid_field",
            expr.span,
            "`cors` `expose` must be a list of header names (string literals)",
        ));
        return;
    };
    let names: Vec<(&Expr, Option<&str>)> = items
        .iter()
        .map(|i| {
            (
                i,
                match &i.kind {
                    ExprKind::StrLit(s) => Some(s.as_str()),
                    _ => None,
                },
            )
        })
        .collect();
    for (item, name) in &names {
        match name {
            None => errors.push(CompileError::new(
                "bynk.http.cors_invalid_field",
                item.span,
                "each `cors` `expose` entry must be a header name written as a string literal",
            )),
            Some("*") if names.len() > 1 => errors.push(
                CompileError::new(
                    "bynk.http.cors_invalid_field",
                    item.span,
                    "`\"*\"` in `cors` `expose` must be the only entry",
                )
                .with_note("the wildcard exposes every header; list names or write `[\"*\"]`, not both"),
            ),
            Some(n) if *n != "*" && !is_header_token(n) => errors.push(
                CompileError::new(
                    "bynk.http.cors_invalid_field",
                    item.span,
                    format!("`{n}` is not a valid header name for `cors` `expose`"),
                )
                .with_note("a header name is letters, digits and ``!#$%&'*+-.^_`|~``, with no spaces, colons or commas"),
            ),
            _ => {}
        }
    }
}

/// RFC 9110 §5.6.2: a `token`, the grammar of a header field name.
fn is_header_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
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
        if !matches!(
            field.name.name.as_str(),
            "hsts" | "nosniff" | "frameAncestors" | "referrerPolicy" | "csp"
        ) {
            errors.push(
                CompileError::new(
                    "bynk.http.security_unknown_field",
                    field.name.span,
                    format!("unknown `security` field `{}`", field.name.name),
                )
                .with_note(
                    "known fields are `hsts`, `nosniff`, `frameAncestors`, `referrerPolicy` and `csp`",
                ),
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
    validate_html_security_fields(policy, errors);
}

/// #3923: whether `s` is an exact origin a `frame-ancestors` source list may carry:
/// `http://` or `https://`, then a host (and optional port), with no path, no
/// wildcard and nothing that could end the directive or the header.
fn is_exact_origin(s: &str) -> bool {
    let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
    else {
        return false;
    };
    !rest.is_empty()
        && rest
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']'))
}

/// #3923: the three HTML-response fields of a `security { }` policy:
/// `frameAncestors` (a list of exact origins, `"self"`, or `["*"]` beside a `csp`),
/// `referrerPolicy` (one of the standard tokens, in camel case) and `csp` (a string
/// literal with no line break). A bad value is `bynk.http.security_invalid_field`.
fn validate_html_security_fields(policy: &SecurityPolicy, errors: &mut Vec<CompileError>) {
    if let Some(expr) = policy.field("frameAncestors") {
        validate_frame_ancestors(expr, policy, errors);
    }
    validate_scalar_html_fields(policy, errors);
}

/// #3923: `frameAncestors` is a list of exact origins or `"self"`; `["*"]` stands alone
/// and needs an explicit `csp`.
fn validate_frame_ancestors(expr: &Expr, policy: &SecurityPolicy, errors: &mut Vec<CompileError>) {
    let invalid = |span, message: String| {
        CompileError::new("bynk.http.security_invalid_field", span, message)
    };
    let ExprKind::ListLit(items, ..) = &expr.kind else {
        errors.push(invalid(
            expr.span,
            "`security` `frameAncestors` must be a list of string literals".to_string(),
        ));
        return;
    };
    let wildcard = items
        .iter()
        .any(|i| matches!(&i.kind, ExprKind::StrLit(s) if s == "*"));
    if wildcard && (items.len() != 1 || policy.csp().is_none()) {
        errors.push(
            invalid(
                expr.span,
                "`frameAncestors: [\"*\"]` must stand alone and needs an explicit `csp`".to_string(),
            )
            .with_note("allowing every site to frame a page is never the result of an accident: say so beside the policy you mean it for"),
        );
    }
    for item in items {
        match &item.kind {
            ExprKind::StrLit(s) if s == "*" || s == "self" || is_exact_origin(s) => {}
            ExprKind::StrLit(s) => errors.push(
                invalid(
                    item.span,
                    format!("`frameAncestors` entry `{s}` is not an exact origin"),
                )
                .with_note("write an origin such as \"https://partner.example\", or \"self\"; wildcards are refused, as in `cors`"),
            ),
            _ => errors.push(invalid(
                item.span,
                "each `frameAncestors` entry must be a string literal".to_string(),
            )),
        }
    }
}

/// #3923: `referrerPolicy` and `csp`, the two scalar HTML-response fields.
fn validate_scalar_html_fields(policy: &SecurityPolicy, errors: &mut Vec<CompileError>) {
    if let Some(expr) = policy.field("referrerPolicy") {
        let ok = matches!(&expr.kind, ExprKind::Ident(id)
            if REFERRER_POLICIES.iter().any(|(camel, _)| *camel == id.name));
        if !ok {
            let names: Vec<&str> = REFERRER_POLICIES.iter().map(|(c, _)| *c).collect();
            errors.push(
                CompileError::new(
                    "bynk.http.security_invalid_field",
                    expr.span,
                    "`security` `referrerPolicy` must be one of the standard values, in camel case",
                )
                .with_note(format!("the values are {}", names.join(", "))),
            );
        }
    }
    if let Some(expr) = policy.field("csp") {
        match &expr.kind {
            ExprKind::StrLit(s) if !s.contains(['\r', '\n']) && !s.trim().is_empty() => {}
            _ => errors.push(
                CompileError::new(
                    "bynk.http.security_invalid_field",
                    expr.span,
                    "`security` `csp` must be a non-empty string literal with no line break",
                )
                .with_note("the policy is sent verbatim as one header value"),
            ),
        }
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
    // #3905: a `from websocket` service takes `limits { maxFrame }`, its own frame ceiling.
    if matches!(service.protocol, ServiceProtocol::WebSocket { .. }) {
        validate_ws_limits_policy(policy, errors);
        return;
    }
    if !matches!(service.protocol, ServiceProtocol::Http) {
        errors.push(
            CompileError::new(
                "bynk.http.limits_not_http",
                policy.span,
                "a `limits { }` policy is only valid on a `from http` or `from websocket` service",
            )
            .with_note(
                "a size ceiling governs the HTTP request body or a WebSocket frame, \
                 which only a `from http` or `from websocket` service has",
            ),
        );
        return;
    }

    // Field names are a closed set; flag anything else (the parser accepts any
    // name, per the CORS / security / annotation precedent).
    for field in &policy.fields {
        if !matches!(field.name.name.as_str(), "maxBody" | "maxFile" | "maxParts") {
            errors.push(
                CompileError::new(
                    "bynk.http.limits_unknown_field",
                    field.name.span,
                    format!("unknown `limits` field `{}`", field.name.name),
                )
                .with_note("the fields are `maxBody`, and (for `@multipart` routes) `maxFile` and `maxParts`; `maxFrame` bounds a `from websocket` service"),
            );
        }
    }
    // #3276: `maxFile` and `maxParts` are positive `Int` counts too.
    for name in ["maxFile", "maxParts"] {
        if let Some(expr) = policy.field(name)
            && !matches!(&expr.kind, ExprKind::IntLit { value: n, .. } if *n > 0)
        {
            errors.push(CompileError::new(
                "bynk.http.limits_invalid_field",
                expr.span,
                format!("`limits` `{name}` must be a positive `Int` literal"),
            ));
        }
    }

    // `maxBody`, when present, is a *positive* `Int` literal — a byte count. Zero
    // or a negative ceiling is nonsensical (it would reject every request). There
    // is no byte `Size` literal yet, so v1 takes a plain `Int` (ADR 0165
    // DECISION C).
    if let Some(expr) = policy.field("maxBody")
        && !matches!(&expr.kind, ExprKind::IntLit { value: n, .. } if (1..=MAX_BODY_CEILING).contains(n))
    {
        errors.push(
            CompileError::new(
                "bynk.http.limits_invalid_field",
                expr.span,
                "`limits` `maxBody` must be a positive `Int` literal — a byte count (e.g. `1_048_576`) — no larger than 524_288_000",
            )
            .with_note(MAX_BODY_NOTE),
        );
    }
}

/// #3905: a `from websocket` service's `limits { }`: the closed field set is `maxFrame`, a positive
/// `Int` byte count no larger than Cloudflare's own 32 MiB inbound message limit. The request-body
/// fields belong to `from http`.
fn validate_ws_limits_policy(policy: &LimitsPolicy, errors: &mut Vec<CompileError>) {
    for field in &policy.fields {
        if field.name.name != "maxFrame" {
            errors.push(
                CompileError::new(
                    "bynk.http.limits_unknown_field",
                    field.name.span,
                    format!(
                        "unknown `limits` field `{}` on a `from websocket` service",
                        field.name.name
                    ),
                )
                .with_note(
                    "a WebSocket service's only field is `maxFrame`; `maxBody`, `maxFile` and `maxParts` bound an HTTP request",
                ),
            );
        }
    }
    if let Some(expr) = policy.field("maxFrame")
        && !matches!(&expr.kind, ExprKind::IntLit { value: n, .. } if (1..=MAX_FRAME_CEILING).contains(n))
    {
        errors.push(
            CompileError::new(
                "bynk.http.limits_invalid_field",
                expr.span,
                "`limits` `maxFrame` must be a positive `Int` literal, a byte count (e.g. `262_144`), no larger than 33_554_432",
            )
            .with_note(
                "a frame over `maxFrame` is closed with `1009` before it is parsed, and one the server sends over it faults the send; 33_554_432 (32 MiB) is Cloudflare's own limit on an inbound message, and the default is 1_048_576",
            ),
        );
    }
}

/// #2858: validate a service's `json { }` policy. The response encoding is a
/// property of the HTTP surface, so the section is only legal on a `from http`
/// service; the closed field set is `options`, whose value is the bare
/// identifier `tagged` or `plain`.
fn validate_json_policy(
    service: &ServiceDecl,
    policy: &JsonPolicy,
    errors: &mut Vec<CompileError>,
) {
    if !matches!(service.protocol, ServiceProtocol::Http) {
        errors.push(
            CompileError::new(
                "bynk.http.json_not_http",
                policy.span,
                "a `json { }` policy is only valid on a `from http` service",
            )
            .with_note("the response JSON encoding governs the HTTP surface, which only a `from http` service has"),
        );
        return;
    }
    for field in &policy.fields {
        if field.name.name != "options" {
            errors.push(
                CompileError::new(
                    "bynk.http.json_unknown_field",
                    field.name.span,
                    format!("unknown `json` field `{}`", field.name.name),
                )
                .with_note("the only field is `options`"),
            );
        }
    }
    if let Some(expr) = policy.field("options")
        && JsonOptions::from_expr(expr).is_none()
    {
        errors.push(CompileError::new(
            "bynk.http.json_invalid_field",
            expr.span,
            "`json` `options` must be `tagged` or `plain`",
        ));
    }
}

/// #4266: what to do instead of reading a header the route's actor consumes, for the note on the
/// diagnostic: the credential itself is never the handler's to read, only the identity it proves.
fn consumed_header_note(wire: &str, session_cookie: bool) -> &'static str {
    if wire == "cookie" && session_cookie {
        "read the other cookies through a typed `cookies` record, which the session cookie is also removed from; the signed-in identity is the actor's"
    } else if wire == "authorization" {
        "the router verifies the credential and hands the handler the actor's identity instead; use that, or drop the field"
    } else {
        "the router verifies this header and the handler never sees it; drop the field"
    }
}

/// #4266: a typed `headers` field that reads a header the route's actor seam consumes. The router
/// hides that header from the record (`authorization` on a `Bearer`, `Oidc` or `Signature` route,
/// the session cookie out of `cookie` on a `SignedCookie` one), so the field always sees it
/// absent: a required field answers `400` on every request, and an `Option` or `List` one is
/// always `None` or empty.
fn validate_consumed_headers(
    table: &UnitTable,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    for service in table.services.values() {
        for handler in &service.handlers {
            let Some(p) = handler.params.iter().find(|p| p.name.name == "headers") else {
                continue;
            };
            let TypeRef::Named(id) = &p.type_ref else {
                continue;
            };
            let Some(TypeBody::Record(record)) = types.get(&id.name).map(|t| &t.body) else {
                continue;
            };
            let consumed = crate::actors::consumed_credential_headers(handler, &table.actors);
            let session = crate::actors::signed_cookie_seam_for(handler, &table.actors);
            for f in &record.fields {
                let wire = f
                    .wire_name()
                    .unwrap_or_else(|| f.name.name.clone())
                    .to_ascii_lowercase();
                let (actor, what) = match consumed.iter().find(|(h, _)| *h == wire) {
                    Some((_, actor)) => (actor.clone(), format!("the `{wire}` header")),
                    None => match &session {
                        Some(s) if wire == "cookie" => (
                            s.actor.clone(),
                            format!(
                                "the `cookie` header, less its session cookie `{}`",
                                s.cookie
                            ),
                        ),
                        _ => continue,
                    },
                };
                let optional = matches!(f.type_ref, TypeRef::Option(..) | TypeRef::List(..));
                // The session cookie is removed from `cookie`, so that field is absent unless
                // the request carries another cookie too; the others are always absent.
                let always = wire != "cookie" || session.is_none();
                let (code, why) = match (optional, always) {
                    (true, true) => (
                        "bynk.http.headers_field_consumed_optional",
                        "so it is always absent here",
                    ),
                    (true, false) => (
                        "bynk.http.headers_field_consumed_optional",
                        "so it is absent unless the request carries another cookie",
                    ),
                    (false, true) => (
                        "bynk.http.headers_field_consumed",
                        "so this field is always absent and every request is answered `400`",
                    ),
                    (false, false) => (
                        "bynk.http.headers_field_consumed",
                        "so this field is absent, and the request is answered `400`, unless it carries another cookie",
                    ),
                };
                errors.push(
                    CompileError::new(
                        code,
                        f.span,
                        format!(
                            "field `{}` of the `headers` record reads {what}, which `{actor}` consumes, {why}",
                            f.name.name
                        ),
                    )
                    .with_note(consumed_header_note(&wire, session.is_some())),
                );
            }
        }
    }
}

/// #3916: validate each service's `errors { }` policy.
fn validate_error_policies(table: &UnitTable, errors: &mut Vec<CompileError>) {
    for service in table.services.values() {
        if let Some(policy) = &service.errors {
            validate_errors_policy(service, policy, errors);
        }
    }
}

/// #3916: validate a service's `errors { }` policy. The error-body shape is a
/// property of the HTTP surface, so the section is only legal on a `from http`
/// service; the closed field set is `format`, whose value is the bare
/// identifier `bynk` or `problem`.
fn validate_errors_policy(
    service: &ServiceDecl,
    policy: &ErrorsPolicy,
    errors: &mut Vec<CompileError>,
) {
    if !matches!(service.protocol, ServiceProtocol::Http) {
        errors.push(
            CompileError::new(
                "bynk.http.errors_not_http",
                policy.span,
                "an `errors { }` policy is only valid on a `from http` service",
            )
            .with_note("the error-body shape governs the HTTP surface, which only a `from http` service has"),
        );
        return;
    }
    for field in &policy.fields {
        if field.name.name != "format" {
            errors.push(
                CompileError::new(
                    "bynk.http.errors_unknown_field",
                    field.name.span,
                    format!("unknown `errors` field `{}`", field.name.name),
                )
                .with_note("the only field is `format`"),
            );
        }
    }
    match policy.field("format") {
        Some(expr) if ErrorFormat::from_expr(expr).is_none() => {
            errors.push(CompileError::new(
                "bynk.http.errors_invalid_field",
                expr.span,
                "`errors` `format` must be `bynk` or `problem`",
            ));
        }
        _ => {}
    }
}

/// #3916: the placement rules of `@errors`: at most one, and only on an `on http`
/// handler. `false` when the annotation is misplaced (already reported), so its
/// arguments are not checked.
fn check_errors_annotation(
    ann: &Annotation,
    handler: &Handler,
    seen: &mut bool,
    errors: &mut Vec<CompileError>,
) -> bool {
    if *seen {
        errors.push(CompileError::new(
            "bynk.http.errors_duplicate",
            ann.span,
            "a handler carries at most one `@errors` annotation",
        ));
        return false;
    }
    *seen = true;
    if !matches!(handler.kind, HandlerKind::Http { .. }) {
        errors.push(
            CompileError::new(
                "bynk.http.errors_not_http",
                ann.span,
                "`@errors` is only valid on an `on http` handler",
            )
            .with_note("it chooses the shape of an HTTP error body"),
        );
        return false;
    }
    validate_errors_args(ann, errors);
    true
}

/// #3916: validate `@errors(format: bynk | problem)` on an HTTP handler.
fn validate_errors_args(ann: &Annotation, errors: &mut Vec<CompileError>) {
    let mut format: Option<&AnnotationArg> = None;
    for arg in &ann.args {
        match arg.label.as_ref().map(|l| l.name.as_str()) {
            Some("format") => format = Some(arg),
            _ => errors.push(
                CompileError::new(
                    "bynk.http.errors_unknown_field",
                    arg.span,
                    "`@errors` accepts only the `format:` argument",
                )
                .with_note("write `@errors(format: problem)`"),
            ),
        }
    }
    match format {
        None => errors.push(
            CompileError::new(
                "bynk.http.errors_invalid_field",
                ann.span,
                "`@errors` needs a `format:` argument",
            )
            .with_note("write `@errors(format: problem)`"),
        ),
        Some(arg) if ErrorFormat::from_expr(&arg.value).is_none() => {
            errors.push(CompileError::new(
                "bynk.http.errors_invalid_field",
                arg.span,
                "`@errors` `format` must be `bynk` or `problem`",
            ));
        }
        Some(_) => {}
    }
}

/// #2858: validate `@json(options: tagged | plain)` on an HTTP handler.
fn validate_json_args(ann: &Annotation, errors: &mut Vec<CompileError>) {
    let mut options: Option<&AnnotationArg> = None;
    for arg in &ann.args {
        match arg.label.as_ref().map(|l| l.name.as_str()) {
            Some("options") => options = Some(arg),
            _ => errors.push(
                CompileError::new(
                    "bynk.http.json_unknown_arg",
                    arg.span,
                    "`@json` accepts only the `options:` argument",
                )
                .with_note("write `@json(options: plain)`"),
            ),
        }
    }
    match options {
        None => errors.push(
            CompileError::new(
                "bynk.http.json_invalid_arg",
                ann.span,
                "`@json` needs an `options:` argument",
            )
            .with_note("write `@json(options: plain)`"),
        ),
        Some(arg) if JsonOptions::from_expr(&arg.value).is_none() => {
            errors.push(CompileError::new(
                "bynk.http.json_invalid_arg",
                arg.span,
                "`@json` `options` must be `tagged` or `plain`",
            ));
        }
        Some(_) => {}
    }
}

/// #2858: the first `Option[U]` met writing `t` as plain JSON whose payload `U`
/// can itself encode as `null` (`Option[T]` or `()`), as `U` displayed. Plain
/// encoding would write `Some(null-payload)` and `None` identically. The walk
/// goes through containers, named records and sums, and generic instantiations
/// (with their type arguments substituted); `seen` guards a recursive type.
pub(crate) fn null_payload_option(
    t: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    seen: &mut HashSet<String>,
) -> Option<String> {
    match t {
        TypeRef::Option(inner, _) => {
            if matches!(inner.as_ref(), TypeRef::Option(..) | TypeRef::Unit(_)) {
                return Some(ts_type_ref_display(inner));
            }
            null_payload_option(inner, types, seen)
        }
        TypeRef::List(a, _)
        | TypeRef::Set(a, _)
        | TypeRef::Effect(a, _)
        | TypeRef::HttpResult(a, _, _) => null_payload_option(a, types, seen),
        TypeRef::Map(k, v, _) | TypeRef::Result(k, v, _) => {
            null_payload_option(k, types, seen).or_else(|| null_payload_option(v, types, seen))
        }
        TypeRef::Named(id) => {
            if !seen.insert(id.name.clone()) {
                return None;
            }
            match types.get(&id.name).map(|d| &d.body) {
                Some(TypeBody::Record(r)) => r
                    .fields
                    .iter()
                    .find_map(|f| null_payload_option(&f.type_ref, types, seen)),
                Some(TypeBody::Sum(s)) => s
                    .variants
                    .iter()
                    .flat_map(|v| &v.payload)
                    .find_map(|f| null_payload_option(&f.type_ref, types, seen)),
                _ => None,
            }
        }
        TypeRef::App { name, args, .. } => {
            if !seen.insert(bynk_check_wire_key(&name.name, args)) {
                return None;
            }
            let fields = crate::wire::record_inst_fields(&name.name, args, types)
                .map(|fs| fs.into_iter().map(|(_, t)| t).collect::<Vec<_>>())
                .or_else(|| {
                    crate::wire::sum_inst_variants(&name.name, args, types).map(|vs| {
                        vs.into_iter()
                            .flat_map(|(_, fs)| fs.into_iter().map(|(_, t)| t))
                            .collect()
                    })
                })
                .unwrap_or_default();
            args.iter()
                .find_map(|a| null_payload_option(a, types, seen))
                .or_else(|| {
                    fields
                        .iter()
                        .find_map(|f| null_payload_option(f, types, seen))
                })
        }
        _ => None,
    }
}

/// The `seen` key of a generic instantiation.
fn bynk_check_wire_key(name: &str, args: &[TypeRef]) -> String {
    crate::wire::inst_codec_suffix(name, args)
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
            let unit_result = matches!(inner, TypeRef::HttpResult(t, _, _) if matches!(t.as_ref(), TypeRef::Unit(_)));
            if !unit_result {
                errors.push(signature(
                    handler.return_type.span(),
                    "`on rejected` returns `HttpResult[()]` (or `Effect[HttpResult[()]]`)",
                ));
            }
        }
    }
}

/// #3753: `*`, `{` and `}` are reserved in a route segment, literal or
/// `:name`, so a route form can be added later without changing what an
/// existing route means. #3752 spent `*` on a final `*name` rest, which the
/// caller reads before asking here. Reports the segment's first reserved
/// character, once, and returns whether it found one, so the caller skips the
/// segment rather than also reporting a parameter named `key*`.
fn reserved_route_char(
    seg: &str,
    path: &str,
    handler: &Handler,
    errors: &mut Vec<CompileError>,
) -> bool {
    let Some(ch) = seg.chars().find(|c| matches!(c, '*' | '{' | '}')) else {
        return false;
    };
    errors.push(
        CompileError::new(
            "bynk.http.reserved_route_syntax",
            handler.span,
            format!("`{ch}` in the route `{path}` is reserved for a future route form"),
        )
        .with_note(if ch == '*' {
            "a rest segment is `*` and then a name, and only as the route's whole last segment (`/files/*key`)".to_string()
        } else if seg.starts_with(':') {
            format!("drop `{ch}` from the parameter's name: a `:name` segment already takes any value, so check it in the handler")
        } else {
            format!("to match a literal `{ch}`, take the segment as a `:name` parameter and check it in the handler")
        }),
    );
    true
}

/// The `connection` a `from websocket` `on message`/`on close` handler borrows (not owned, so no disposal obligation).
fn borrowed_connection(
    handler: &Handler,
    is_ws_lifecycle: bool,
) -> std::collections::HashSet<String> {
    if is_ws_lifecycle && matches!(handler.kind, HandlerKind::Message | HandlerKind::Close) {
        std::iter::once("connection".to_string()).collect()
    } else {
        std::collections::HashSet::new()
    }
}

/// #4389: whether `handler` is an `on http` handler of a method other than `GET`.
fn is_non_get_http(handler: &Handler) -> bool {
    matches!(&handler.kind, HandlerKind::Http { method, .. } if *method != HttpMethod::Get)
}

/// Validate an `on http METHOD "path"` handler (v0.9 §4.1):
///
/// - Path must start with `/`, must not be `/_bynk/...` (reserved).
/// - Every `:name` segment binds to a handler parameter of the same name.
/// - Every parameter is either a path parameter or named `body`, `headers` or
///   `query`.
/// - Path parameter types are those of a typed `query` field, less `Option` and
///   `List` (#3910): a `String`, `Int`, `Bool` or `Decimal`, an enum, or a refined
///   or opaque type over one of those.
/// - GET / DELETE handlers may not have a `body` parameter.
/// - The handler return type must be `Effect[HttpResult[T]]`.
fn validate_http_handler(
    handler: &Handler,
    method: HttpMethod,
    path: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    cf_is_std: bool,
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
    // #3753: a refused route has no reliable parameter list, so its handler
    // parameters are not also reported as unbound (`extra_param`).
    let mut refused = false;
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    for (i, seg) in segs.iter().copied().enumerate() {
        // #3752 (D2): a final `*name` is a rest, bound like a parameter; a final
        // bare `*` is `bynk.http.rest_unnamed` (`crate::http_rest`).
        if i + 1 == segs.len() {
            if let Some(name) = crate::http_rest::rest_name(seg) {
                path_param_names.push(name);
                continue;
            }
            if crate::http_rest::is_unnamed_rest(seg) {
                refused = true;
                continue;
            }
        }
        if reserved_route_char(seg, path, handler, errors) {
            refused = true;
            continue;
        }
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
                format!(
                    "path parameter `{}{name}` has no matching handler parameter `{name}`",
                    if crate::http_rest::rest_name(path) == Some(name) {
                        "*"
                    } else {
                        ":"
                    },
                ),
            ));
        }
    }
    // Every handler parameter must be a path param, `body` or `headers`.
    for p in &handler.params {
        let is_path = path_param_names.iter().any(|n| n == &p.name.name.as_str());
        let is_body = p.name.name == "body";
        // #2341 (decision A): `headers` is the request's headers: `RequestHeaders`, or (#3911)
        // a record `T` the router decodes and validates.
        let is_headers = !is_path && p.name.name == "headers";
        if is_headers {
            validate_headers_param(p, types, errors);
        }
        // #2854: `query` is the request's query string, a record `T` the router
        // decodes and validates, or the raw `RequestQuery` view.
        let is_query = !is_path && p.name.name == "query";
        if is_query {
            validate_query_param(p, types, errors);
        }
        // #3277: `cookies` is the request's cookies, a record `T` the router decodes
        // and validates.
        let is_cookies = !is_path && p.name.name == "cookies";
        if is_cookies {
            validate_cookies_param(p, types, errors);
        }
        // #3513 C: `cf` is what Cloudflare derived about the connection, and must be
        // typed `RequestCf` (from `bynk.cloudflare`).
        let is_cf = !is_path && p.name.name == "cf";
        if is_cf
            && !(cf_is_std && matches!(&p.type_ref, TypeRef::Named(id) if id.name == "RequestCf"))
        {
            errors.push(
                CompileError::new(
                    "bynk.http.cf_wrong_type",
                    p.type_ref.span(),
                    format!(
                        "an HTTP handler's `cf` parameter must be typed `RequestCf` (got `{}`)",
                        ts_type_ref_display(&p.type_ref),
                    ),
                )
                .with_note(
                    "`cf: RequestCf` gives the handler what Cloudflare derived about the request; it is the `RequestCf` of `bynk.cloudflare` (`consumes bynk.cloudflare { RequestCf }`), not a type of your own",
                ),
            );
        }
        // #3276: the `body` of a `@form` handler is a record of form leaves.
        if is_body
            && let Some(kind) = handler
                .annotations
                .iter()
                .find(|a| matches!(a.name.name.as_str(), "form" | "multipart"))
                .map(|a| a.name.name.as_str())
        {
            validate_form_body(p, kind, types, errors);
            if let Some(by) = handler.by_clause.as_ref().filter(|b| b.is_sum()) {
                errors.push(
                    CompileError::new(
                        "bynk.http.form_on_sum_route",
                        by.span,
                        format!("a `@{kind}` route cannot take a sum of actors"),
                    )
                    .with_note(
                        "a multi-actor route reads and verifies the body as JSON itself; name one actor, or split the route",
                    ),
                );
            }
        }
        // #3380: a streamed `body`'s route cannot take a sum of actors either: its
        // wrapper reads and verifies the body itself.
        if is_body
            && is_byte_stream(&p.type_ref)
            && let Some(by) = handler.by_clause.as_ref().filter(|b| b.is_sum())
        {
            errors.push(
                CompileError::new(
                    "bynk.http.stream_body_on_sum_route",
                    by.span,
                    "a route with a streamed `body` cannot take a sum of actors",
                )
                .with_note(
                    "a multi-actor route reads and verifies the body itself before the handler runs; name one actor, or split the route",
                ),
            );
        }
        // #3752: a pass-through route's `request`, judged in `crate::http_pass_through`.
        let is_request = crate::http_pass_through::owns_param(method, p, types);
        if !refused
            && !is_path
            && !is_body
            && !is_headers
            && !is_query
            && !is_cookies
            && !is_cf
            && !is_request
        {
            errors.push(
                CompileError::new(
                    "bynk.http.extra_param",
                    p.span,
                    format!(
                        "handler parameter `{}` is not a path parameter and is not named `body`, `headers`, `query`, `cookies` or `cf`",
                        p.name.name
                    ),
                )
                .with_note(
                    "HTTP handler parameters must match a `:name` path segment, or be named `body`, `headers`, `query`, `cookies` or `cf`",
                ),
            );
        }
        // #3910: a path parameter reads as a typed `query` field does, minus `Option`
        // and `List` (a segment is always present and single-valued).
        // #3752: a rest's type is `bynk.http.rest_param_type`'s concern.
        let is_rest = is_path && crate::http_rest::rest_name(path) == Some(p.name.name.as_str());
        if is_path && !is_rest && !is_query_leaf(&p.type_ref, types) {
            errors.push(
                CompileError::new(
                    "bynk.http.path_param_not_stringy",
                    p.type_ref.span(),
                    format!(
                        "path parameter `{}` must be a `String`, `Int`, `Bool` or `Decimal`, an enum, or a refined or opaque type over one of those (got `{}`)",
                        p.name.name,
                        ts_type_ref_display(&p.type_ref),
                    ),
                )
                .with_note(
                    "a path segment is one value, so `Option` and `List` are refused; an enum is read by variant name",
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
                .with_note("GET and DELETE requests conventionally carry no body"),
            );
        }
    }
    // Validate return type shape. #3752: a pass-through route's is judged by
    // `crate::http_pass_through`.
    let return_ok =
        crate::http_pass_through::owns_return_check(method, &handler.return_type, types)
            || match &handler.return_type {
                TypeRef::Effect(inner, _) => matches!(inner.as_ref(), TypeRef::HttpResult(_, _, _)),
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

/// #2854: whether `r` reads off a query string as one value: a `String`, `Int`,
/// `Bool` or `Decimal`, a refined or opaque type over one of those, or an enum
/// (a sum none of whose variants carries a payload).
fn is_query_leaf(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    let stringy_base = |b: &BaseType| {
        matches!(
            b,
            BaseType::String | BaseType::Int | BaseType::Bool | BaseType::Decimal
        )
    };
    match r {
        TypeRef::Base(b, _) => stringy_base(b),
        TypeRef::Named(id) => match types.get(&id.name).map(|t| &t.body) {
            Some(TypeBody::Refined { base, .. }) | Some(TypeBody::Opaque { base, .. }) => {
                stringy_base(base)
            }
            Some(TypeBody::Sum(s)) => s.variants.iter().all(|v| v.payload.is_empty()),
            _ => false,
        },
        _ => false,
    }
}

/// #3513 C: whether this unit's `RequestCf` is the standard library's: consumed from
/// `bynk.cloudflare`, and not a type the unit declares itself. `cf` takes only that one,
/// so a look-alike cannot bypass the platform lock and the unstable gate, which are both
/// keyed to the import.
fn cf_type_is_std(cross_context: &resolver::CrossContextInfo, table: &UnitTable) -> bool {
    cross_context
        .consumed_types
        .get("bynk.cloudflare")
        .is_some_and(|t| t.contains_key("RequestCf"))
        && !table.types.contains_key("RequestCf")
}

/// #2854: whether `r` is a legal field type of a typed `query` record: a leaf,
/// an `Option` of one, or a `List` of one (a key repeated).
pub(crate) fn is_query_field_type(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    match r {
        TypeRef::Option(inner, _) | TypeRef::List(inner, _) => is_query_leaf(inner, types),
        other => is_query_leaf(other, types),
    }
}

/// Where a record is read off an HTTP request: the surface decides what a wire
/// name must look like and whether two names that differ only in case clash.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RequestSurface {
    Cookies,
    Query,
    Form,
    /// #3911: the `headers` record, whose names compare without case.
    Headers,
}

/// #3446: the records a handler of this unit decodes as its `cookies` parameter, in any file
/// of the unit. `Record.field.cookie(v)` is legal only on one of these: the decoder it must agree
/// with is then in the unit that writes the cookie.
pub(crate) fn cookie_records_of(table: &UnitTable) -> HashSet<String> {
    let mut out = HashSet::new();
    for service in table.services.values() {
        for h in &service.handlers {
            if !matches!(h.kind, HandlerKind::Http { .. }) {
                continue;
            }
            for p in &h.params {
                if p.name.name == "cookies"
                    && let TypeRef::Named(id) = &p.type_ref
                {
                    out.insert(id.name.clone());
                }
            }
        }
    }
    out
}

/// RFC 9110 `token`: a cookie name is one.
fn is_http_token(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_graphic() && !"()<>@,;:\\\"/[]?={}".contains(c))
}

/// #3343: what a request surface adds to a record field's `@name("…")`: a record used as the
/// `cookies`, `headers`, `query` or `@form`/`@multipart` `body` parameter of an `on http` handler
/// is read by name, so a cookie or header name must be an RFC 9110 token, and names of a cookies
/// or query record may not differ only in case. A wire name is legal on any record's field
/// (#3975); the rules every wire name follows are `wire_names::validate_record`.
pub(crate) fn validate_field_names(
    table: &UnitTable,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    let mut uses: HashMap<String, Vec<RequestSurface>> = HashMap::new();
    for service in table.services.values() {
        for h in &service.handlers {
            if !matches!(h.kind, HandlerKind::Http { .. }) {
                continue;
            }
            for p in &h.params {
                let surface = match p.name.name.as_str() {
                    "cookies" => Some(RequestSurface::Cookies),
                    "headers" => Some(RequestSurface::Headers),
                    "query" => Some(RequestSurface::Query),
                    "body"
                        if h.annotations
                            .iter()
                            .any(|a| matches!(a.name.name.as_str(), "form" | "multipart")) =>
                    {
                        Some(RequestSurface::Form)
                    }
                    _ => None,
                };
                if let (Some(s), TypeRef::Named(id)) = (surface, &p.type_ref) {
                    uses.entry(id.name.clone()).or_default().push(s);
                }
            }
        }
    }
    // #3975: the wire names themselves (the argument, an empty or repeated name) are checked on
    // the unit's own types (`wire_names::validate_record`); what is left here is what a request
    // surface adds: a cookie or header name must be a token, and names that differ only in case.
    let mut names: Vec<&String> = uses.keys().collect();
    names.sort();
    for name in names {
        let Some(TypeBody::Record(rec)) = types.get(name).map(|t| &t.body) else {
            continue;
        };
        let surfaces = &uses[name];
        let mut wire: Vec<(String, Span)> = Vec::new();
        for f in &rec.fields {
            let named = f
                .annotations
                .iter()
                .find(|a| a.name.name == "name")
                .and_then(|ann| Some((ann, f.wire_name()?)));
            if let Some((ann, literal)) = named {
                let arg = ann.args.first();
                let at = match arg {
                    Some(a) if a.from_const.is_some() => a.value.span,
                    _ => ann.span,
                };
                let bad = if surfaces.contains(&RequestSurface::Cookies) && !is_http_token(&literal)
                {
                    Some(
                        "a cookie name must be an RFC 9110 token (no spaces, quotes or separators)",
                    )
                } else if surfaces.contains(&RequestSurface::Headers) && !is_http_token(&literal) {
                    Some(
                        "a header name must be an RFC 9110 token (no spaces, quotes or separators)",
                    )
                } else {
                    None
                };
                if let Some(why) = bad {
                    errors.push(crate::wire_names::invalid_name(
                        "bynk.types.field_name_invalid",
                        "name",
                        &literal,
                        why,
                        at,
                        arg,
                    ));
                }
                wire.push((literal, at));
            } else {
                wire.push((f.name.name.clone(), f.name.span));
            }
        }
        let case_matters = surfaces
            .iter()
            .any(|s| matches!(s, RequestSurface::Cookies | RequestSurface::Query));
        let headers = surfaces.contains(&RequestSurface::Headers);
        for (i, (a, span)) in wire.iter().enumerate() {
            for (b, _) in &wire[..i] {
                // #3911: header names compare without case, so a difference of case is the same
                // name. An exact repeat is reported with the record's other wire-name checks.
                if a != b && headers && a.eq_ignore_ascii_case(b) {
                    errors.push(CompileError::new(
                        "bynk.types.field_name_duplicate",
                        *span,
                        format!("two fields of `{name}` read the same wire name `{a}`"),
                    ));
                } else if a != b && case_matters && a.eq_ignore_ascii_case(b) {
                    errors.push(
                        CompileError::new(
                            "bynk.types.field_name_case_conflict",
                            *span,
                            format!("wire names `{a}` and `{b}` of `{name}` differ only in case"),
                        )
                        .with_note("cookie and query names are case-sensitive but easily confused; rename one"),
                    );
                }
            }
        }
    }
}

/// #3911: an HTTP handler's `headers` parameter is `RequestHeaders` (#2341), or a record whose
/// fields are each a leaf a header can carry (the typed-`query` set: a `String`, `Int`, `Bool` or
/// `Decimal`, a refined or opaque type over one, an enum), an `Option` of one, or a `List` of one
/// (a comma-separated list).
fn validate_headers_param(
    p: &Param,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    if matches!(p.type_ref, TypeRef::RequestHeaders(_)) {
        return;
    }
    let record = match &p.type_ref {
        TypeRef::Named(id) => match types.get(&id.name).map(|t| &t.body) {
            Some(TypeBody::Record(r)) => Some(r),
            _ => None,
        },
        _ => None,
    };
    let Some(record) = record else {
        errors.push(
            CompileError::new(
                "bynk.http.headers_wrong_type",
                p.type_ref.span(),
                format!(
                    "an HTTP handler's `headers` parameter must be a record type or `RequestHeaders` (got `{}`)",
                    ts_type_ref_display(&p.type_ref),
                ),
            )
            .with_note(
                "`headers: UploadHeaders` decodes the request's headers into the record `UploadHeaders`, one field per header (named by the field, or by `@name(\"if-match\")`), and answers `400` when it does not fit; `headers: RequestHeaders` gives the handler the request's headers raw",
            ),
        );
        return;
    };
    for f in &record.fields {
        if !is_query_field_type(&f.type_ref, types) {
            errors.push(
                CompileError::new(
                    "bynk.http.headers_field_not_stringy",
                    f.type_ref.span(),
                    format!(
                        "field `{}` of the `headers` record has type `{}`, which a header cannot carry",
                        f.name.name,
                        ts_type_ref_display(&f.type_ref),
                    ),
                )
                .with_note(
                    "a header field is a `String`, `Int`, `Bool`, `Decimal`, an enum, or a refined or opaque type over one, optionally in an `Option` or a `List` (a comma-separated list)",
                ),
            );
        }
    }
}

/// #3277: an HTTP handler's `cookies` parameter is a record whose fields are each
/// a leaf a cookie can carry (the typed-`query` set: a `String`, `Int`, `Bool` or
/// `Decimal`, a refined or opaque type over one, an enum), an `Option` of one, or
/// a `List` of one (a name sent twice).
fn validate_cookies_param(
    p: &Param,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    // #3277: the raw view is the other legal form, for a name a field cannot spell.
    if matches!(p.type_ref, TypeRef::RequestCookies(_)) {
        return;
    }
    let record = match &p.type_ref {
        TypeRef::Named(id) => match types.get(&id.name).map(|t| &t.body) {
            Some(TypeBody::Record(r)) => Some(r),
            _ => None,
        },
        _ => None,
    };
    let Some(record) = record else {
        errors.push(
            CompileError::new(
                "bynk.http.cookies_wrong_type",
                p.type_ref.span(),
                format!(
                    "an HTTP handler's `cookies` parameter must be a record type or `RequestCookies` (got `{}`)",
                    ts_type_ref_display(&p.type_ref),
                ),
            )
            .with_note(
                "`cookies: Session` decodes the request's `Cookie` header into the record `Session`, one field per cookie name, and answers `400` when it does not fit; `cookies: RequestCookies` reads them raw, for a name a field cannot spell (`__Host-sid`)",
            ),
        );
        return;
    };
    for f in &record.fields {
        if !is_query_field_type(&f.type_ref, types) {
            errors.push(
                CompileError::new(
                    "bynk.http.cookies_field_not_stringy",
                    f.type_ref.span(),
                    format!(
                        "field `{}` of the `cookies` record has type `{}`, which a cookie cannot carry",
                        f.name.name,
                        ts_type_ref_display(&f.type_ref),
                    ),
                )
                .with_note(
                    "a cookie field is a `String`, `Int`, `Bool`, `Decimal`, an enum, or a refined or opaque type over one, optionally in an `Option` or a `List` (a name sent twice)",
                ),
            );
        }
    }
}

/// #2854: a HTTP handler's `query` parameter is `RequestQuery`, or a record
/// whose fields are each a query leaf, an `Option` of one or a `List` of one.
fn validate_query_param(
    p: &Param,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    if matches!(p.type_ref, TypeRef::RequestQuery(_)) {
        return;
    }
    let record = match &p.type_ref {
        TypeRef::Named(id) => match types.get(&id.name).map(|t| &t.body) {
            Some(TypeBody::Record(r)) => Some(r),
            _ => None,
        },
        _ => None,
    };
    let Some(record) = record else {
        errors.push(
            CompileError::new(
                "bynk.http.query_wrong_type",
                p.type_ref.span(),
                format!(
                    "an HTTP handler's `query` parameter must be a record type or `RequestQuery` (got `{}`)",
                    ts_type_ref_display(&p.type_ref),
                ),
            )
            .with_note(
                "`query: Filter` decodes the query string into the record `Filter` and answers `400` when it does not fit; `query: RequestQuery` reads it raw",
            ),
        );
        return;
    };
    for f in &record.fields {
        if !is_query_field_type(&f.type_ref, types) {
            errors.push(
                CompileError::new(
                    "bynk.http.query_field_not_stringy",
                    f.type_ref.span(),
                    format!(
                        "field `{}` of the `query` record has type `{}`, which a query string cannot carry",
                        f.name.name,
                        ts_type_ref_display(&f.type_ref),
                    ),
                )
                .with_note(
                    "a query field is a `String`, `Int`, `Bool`, `Decimal`, an enum, or a refined or opaque type over one, optionally in an `Option` or a `List` (a repeated key)",
                ),
            );
        }
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
/// #3922: whether `scope: public` on this route is a `bynk.http.cache_public_authenticated`,
/// and whether the route is a `SignedCookie` one. Neither is reported where another rule
/// already says it: a `SignedCookie` route has `bynk.actor.signed_cookie_public_cache`, and
/// in a context that declares `EdgeCache` the edge rule
/// `bynk.cloudflare.edge_cache_public_authenticated` covers every credentialed route.
fn route_actor_kinds(handler: &Handler, service: &ServiceDecl, table: &UnitTable) -> (bool, bool) {
    let edge = table.derived.values().any(|d| d.is_edge_cache());
    let by = handler.by_clause.as_ref().or(service.default_by.as_ref());
    by.map_or((false, false), |by| {
        (
            !edge && crate::actors::route_is_credentialed(by, &table.actors),
            crate::actors::by_clause_is_signed_cookie(by, &table.actors),
        )
    })
}

fn validate_handler_annotations(
    handler: &Handler,
    service_cap: Option<i64>,
    (credentialed, signed_cookie_route): (bool, bool),
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
    let mut seen_form = false;
    let mut seen_json = false;
    let mut seen_errors = false;
    // #3276: `@raw` and `@form` each fix the body's one wire format, so a handler
    // carries at most one of them.
    let mut body_kind: Option<&str> = None;
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
                // #3922: `noStore` is allowed on any HTTP route, since data that must never
                // be stored is not only a `GET`'s. Another protocol, or an agent, has no
                // response to stamp, so it stays `cache_on_non_get`.
                let is_http = matches!(handler.kind, HandlerKind::Http { .. });
                // #3752: an `ANY` route's `@cache` is `bynk.http.cache_on_pass_through`.
                let is_any = matches!(
                    handler.kind,
                    HandlerKind::Http {
                        method: HttpMethod::Any,
                        ..
                    }
                );
                if !(is_get || is_any || is_http && handler.cache_no_store()) {
                    errors.push(
                        CompileError::new(
                            "bynk.http.cache_on_non_get",
                            ann.span,
                            "`@cache` is only valid on an `on http GET` handler (`@cache(noStore: true)` is also valid on any other HTTP route)",
                        )
                        .with_note(
                            "conditional caching applies to safe, idempotent reads — a `GET` route",
                        ),
                    );
                    continue;
                }
                validate_cache_args(ann, errors);
                // #3277: a response that depends on the request's cookies is one
                // user's, so it must not be stored by a shared cache.
                let public = ann.args.iter().any(|a| {
                    a.label.as_ref().is_some_and(|n| n.name == "scope")
                        && matches!(&a.value.kind, ExprKind::Ident(id) if id.name == "public")
                });
                // #3922: a credentialed route's response is one caller's.
                // A `SignedCookie` route's own rule is `bynk.actor.signed_cookie_public_cache`.
                if public && credentialed && !signed_cookie_route {
                    errors.push(
                        CompileError::new(
                            "bynk.http.cache_public_authenticated",
                            ann.span,
                            "a `@cache(scope: public)` route is guarded by an actor that carries a credential, so a shared cache could replay one caller's response to another",
                        )
                        .with_note("use `scope: private`, or move a response that is the same for everyone to a `Visitor` route"),
                    );
                } else if public && handler.params.iter().any(|p| p.name.name == "cookies") {
                    errors.push(
                        CompileError::new(
                            "bynk.http.cache_public_reads_cookies",
                            ann.span,
                            "a `@cache(scope: public)` handler reads `cookies`, so a shared cache could replay one user's response to another",
                        )
                        .with_note("use `scope: private`, or drop the `cookies` parameter"),
                    );
                }
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
                validate_limit_args(
                    ann,
                    handler
                        .annotations
                        .iter()
                        .any(|a| a.name.name == "multipart"),
                    errors,
                );
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
                if let Some(other) = body_kind.filter(|k| *k != "raw") {
                    errors.push(body_kind_conflict(ann, "raw", other));
                    continue;
                }
                body_kind = Some("raw");
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
                if !matches!(body.type_ref, TypeRef::Base(BaseType::Bytes, _))
                    && !is_byte_stream(&body.type_ref)
                {
                    errors.push(
                        CompileError::new(
                            "bynk.http.raw_body_not_bytes",
                            body.type_ref.span(),
                            format!(
                                "the `body` of a `@raw` handler must be `Bytes` or `Stream[Bytes]`, but it is `{}`",
                                ts_type_ref_display(&body.type_ref),
                            ),
                        )
                        .with_note("a raw body is handed to the handler as its octets, undecoded: whole as `Bytes`, or chunk by chunk as a `Stream[Bytes]`"),
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
            kind @ ("form" | "multipart") => {
                // #3276: `@form` and `@multipart` each fix the wire format of the
                // one `body`: a urlencoded form, or a `multipart/form-data` one.
                if seen_form {
                    errors.push(CompileError::new(
                        "bynk.http.form_duplicate",
                        ann.span,
                        format!("a handler carries at most one `@{kind}` annotation"),
                    ));
                    continue;
                }
                seen_form = true;
                if let Some(other) = body_kind.filter(|k| *k != kind) {
                    errors.push(body_kind_conflict(ann, kind, other));
                    continue;
                }
                body_kind = Some(kind);
                let has_body = handler.params.iter().any(|p| p.name.name == "body");
                if !is_body_method || !has_body {
                    errors.push(
                        CompileError::new(
                            "bynk.http.form_on_bodyless",
                            ann.span,
                            format!(
                                "`@{kind}` is only valid on a POST/PUT/PATCH `on http` route with a `body` parameter"
                            ),
                        )
                        .with_note(format!(
                            "a {kind} body is the `body` parameter of a body-taking route"
                        )),
                    );
                    continue;
                }
                if let Some(arg) = ann.args.first() {
                    errors.push(
                        CompileError::new(
                            "bynk.http.form_unknown_arg",
                            arg.span,
                            format!("`@{kind}` takes no arguments"),
                        )
                        .with_note(format!(
                            "write `@{kind}`: the media type is fixed, and the caps are `@limit` and `limits {{ }}`"
                        )),
                    );
                }
                // A multipart body is held in memory beside its parsed copy, so a
                // large cap is flagged (every route has a cap: #3921 defaults it).
                if kind == "multipart"
                    && handler
                        .limit_max_body()
                        .or(service_cap)
                        .is_some_and(|cap| cap > MULTIPART_LARGE_CAP)
                {
                    errors.push(
                        CompileError::new(
                            "bynk.http.multipart_large_cap",
                            ann.span,
                            "this `@multipart` route's body cap is over 32 MiB",
                        )
                        .with_note(
                            "the body is buffered with a parsed copy beside it, and a Worker has 128 MB for everything; lower `maxBody`, or hold large uploads to a presigned or multipart-to-R2 flow",
                        ),
                    );
                }
            }
            "json" => {
                if seen_json {
                    errors.push(CompileError::new(
                        "bynk.http.json_duplicate",
                        ann.span,
                        "a handler carries at most one `@json` annotation",
                    ));
                    continue;
                }
                seen_json = true;
                if !matches!(handler.kind, HandlerKind::Http { .. }) {
                    errors.push(
                        CompileError::new(
                            "bynk.http.json_on_non_http",
                            ann.span,
                            "`@json` is only valid on an `on http` handler",
                        )
                        .with_note("it chooses how an HTTP response body writes `Option`"),
                    );
                    continue;
                }
                validate_json_args(ann, errors);
            }
            "errors" => {
                if !check_errors_annotation(ann, handler, &mut seen_errors, errors) {
                    continue;
                }
            }
            // #3926: validated by `http_rate_limit`, which sees the context's limiters.
            "rateLimit" => {}
            other => {
                errors.push(
                    CompileError::new(
                        "bynk.http.unknown_handler_annotation",
                        ann.name.span,
                        format!(
                            "unknown handler annotation `@{other}` — the handler annotations are `@cache`, `@limit`, `@raw`, `@form`, `@multipart`, `@json`, `@errors` and `@rateLimit`"
                        ),
                    )
                    .with_note("handler annotations are a closed set (ADR 0163, ADR 0165)"),
                );
            }
        }
    }
}

/// #3380: whether `r` is exactly `Stream[Bytes]`, the one stream type a `@raw`
/// handler's `body` may be.
pub(crate) fn is_byte_stream(r: &TypeRef) -> bool {
    matches!(r, TypeRef::Stream(inner, _) if matches!(**inner, TypeRef::Base(BaseType::Bytes, _)))
}

/// #3276: the body cap above which a `@multipart` route is warned about: a quarter
/// of a Worker's 128 MB isolate, since the body is buffered with a parsed copy.
const MULTIPART_LARGE_CAP: i64 = 32 * 1024 * 1024;

/// #3921 (DECISION D): one hint per service listing the body-taking routes that
/// now rely on the 1 MiB default cap, so a service that took larger bodies finds
/// out at build, not in production. A route with an explicit cap is not listed.
fn note_defaulted_body_caps(table: &UnitTable, errors: &mut Vec<CompileError>) {
    // In a stable order (the table is a hash map), so an incremental re-analysis
    // reports the same hints in the same order as a cold one.
    let mut services: Vec<&ServiceDecl> = table.services.values().collect();
    services.sort_by(|a, b| a.name.name.cmp(&b.name.name));
    for service in services {
        if service.limits.as_ref().and_then(|p| p.max_body()).is_none() {
            note_service_defaulted_body_caps(service, errors);
        }
    }
}

fn note_service_defaulted_body_caps(service: &ServiceDecl, errors: &mut Vec<CompileError>) {
    let routes: Vec<String> = service
        .handlers
        .iter()
        .filter(|h| h.limit_max_body().is_none() && h.raw_content_type().is_none())
        .filter(|h| h.effective_max_body(None).is_some())
        .filter_map(|h| match &h.kind {
            HandlerKind::Http { method, path } => Some(format!("{} {path}", method.as_str())),
            _ => None,
        })
        .collect();
    if routes.is_empty() {
        return;
    }
    errors.push(
        CompileError::new(
            "bynk.http.body_cap_defaulted",
            service.name.span,
            format!(
                "{} on `{}` {} on the default 1 MiB request-body cap",
                routes.join(", "),
                service.name.name,
                if routes.len() == 1 { "relies" } else { "rely" },
            ),
        )
        .with_note(
            "a larger body is answered `413`; raise it with `@limit(maxBody: …)` on the route or `limits { maxBody: … }` on the service",
        ),
    );
}

/// #3921: why a `maxBody` has an upper bound. There is no "unlimited": a larger
/// cap is a number, up to the largest platform plan's request-body ceiling.
const MAX_BODY_NOTE: &str = "a body cap is a number of bytes, at most 524_288_000 (500 MB, the request-body ceiling of the largest Cloudflare plan); there is no \"unlimited\" setting";

/// #3276: the diagnostic for a handler that declares two request-body wire formats
/// (`@raw` with `@form`): each fixes how the one `body` is read, so they exclude
/// each other.
fn body_kind_conflict(ann: &Annotation, this: &str, other: &str) -> CompileError {
    CompileError::new(
        "bynk.http.body_kind_conflict",
        ann.span,
        format!("a handler cannot carry both `@{other}` and `@{this}`"),
    )
    .with_note("`@raw`, `@form` and `@multipart` each fix the wire format of the one `body` parameter; use one")
}

/// #3276: whether `r` is a `Bool`, or a refined or opaque type over one (the
/// leaves a browser's unchecked checkbox leaves absent).
fn is_bool_leaf(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    match r {
        TypeRef::Base(b, _) => matches!(b, BaseType::Bool),
        TypeRef::Named(id) => matches!(
            types.get(&id.name).map(|t| &t.body),
            Some(TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. })
                if matches!(base, BaseType::Bool)
        ),
        _ => false,
    }
}

/// #3277: whether `decl` (the type table's entry for `name`) is a type the `bynk`
/// adapter exports, found by its first-party span.
pub fn is_std_bynk_type(
    name: &str,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    decl: &TypeDecl,
) -> bool {
    types.get(name).is_some_and(|d| d.span == decl.span)
        && crate::firstparty::unit(bynk_std::BYNK_UNIT)
            .is_some_and(|unit| decl.span.file == crate::project_model::firstparty_file_id(unit))
}

/// A `from websocket` service holds exactly one `on close` (#3902: it is required). `on open` always
/// hands its connection to an agent's state, and the runtime cannot tell which store entry holds it,
/// so nothing else removes a departed client's connection. A second `on close` is
/// `bynk.service.websocket_open_arity`. A missing one is only judged on a service with exactly one
/// `on open`: one with none or several already has that error.
fn check_ws_close_arity(
    service: &ServiceDecl,
    opens: usize,
    closes: &[&Handler],
    errors: &mut Vec<CompileError>,
) {
    if let Some(second) = closes.get(1) {
        errors.push(CompileError::new(
            "bynk.service.websocket_open_arity",
            second.span,
            format!(
                "the `from websocket` service `{}` has more than one `on close` handler — it needs at most one",
                service.name.name
            ),
        ));
    }
    if closes.is_empty() && opens == 1 {
        errors.push(
            CompileError::new(
                "bynk.ws.missing_close",
                service.name.span,
                format!(
                    "the `from websocket` service `{}` has no `on close` handler — it needs one, to release the connection `on open` handed over",
                    service.name.name
                ),
            )
            .with_note(
                "`on open` transfers the connection into an agent's state, so a departed client's connection stays held, and a later broadcast to it raises; add `on close (…) -> Effect[()]` that removes it (a trailing `close: CloseInfo` parameter carries the code and reason)",
            ),
        );
    }
}

/// #3902: whether `p` is the `close: CloseInfo` parameter of an `on close` handler: the `CloseInfo`
/// record `bynk` exports, found by its first-party declaration (as `is_std_file_type` finds `File`),
/// so a user type that happens to be called `CloseInfo` is a route value, not this.
pub fn is_close_info_param(
    p: &Param,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> bool {
    ts_type_ref_display(&p.type_ref) == "CloseInfo"
        && types.get("CloseInfo").is_some_and(|decl| {
            crate::firstparty::unit(bynk_std::BYNK_UNIT).is_some_and(|unit| {
                decl.span.file == crate::project_model::firstparty_file_id(unit)
            })
        })
}

/// #3902: the route parameters of an `on close`: its parameters less a trailing `close: CloseInfo`.
pub fn close_route_params<'a>(
    close: &'a Handler,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> &'a [Param] {
    match close.params.split_last() {
        Some((last, rest)) if is_close_info_param(last, types) => rest,
        _ => &close.params,
    }
}

/// #4111: `bynk.types.secret_at_boundary` for a `Secret` or `Authorization` (as `name` spells it) written at `span` in `what`.
fn secret_at_boundary_error(span: Span, name: &str, what: &str) -> CompileError {
    CompileError::new(
        "bynk.types.secret_at_boundary",
        span,
        format!(
            "a `{name}` cannot appear in {what} — it is never stored, sent, encoded or returned across a boundary"
        ),
    )
    .with_note(
        "read the text with `reveal()` where it is used, or keep it in `Secrets` and read it again where it is needed",
    )
}

/// #4111: which of `bynk`'s secret-carrying opaque types a declaration is (#3939 adds the third).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    /// `Secret`: a value from the platform's secret store, read with `reveal` and `matches`.
    Secret,
    /// `Authorization`: a header credential built from a `Secret` (`bearer`, `basic`), which
    /// `Fetch` reveals when it sends and which has no reader of its own.
    Authorization,
    /// #3939 (D2 D): `ApiKeyValue`, the key an `ApiKey` actor's `mint` answers. It has no reader:
    /// its one way out is a handler returning the `ApiKeySecret` that `mint` answered.
    ApiKeyValue,
}

/// #4111: the secret-carrying `bynk` type a declaration is, found by where it is declared and
/// what the text there says, and never by the name it is spelled here: `consumes bynk { Secret as
/// Token }` renames the declaration but not its source span.
pub fn secret_kind(decl: &TypeDecl) -> Option<SecretKind> {
    if !matches!(decl.body, bynk_syntax::ast::TypeBody::Opaque { .. }) {
        return None;
    }
    let unit = crate::firstparty::unit(bynk_std::BYNK_UNIT)?;
    if decl.span.file != crate::project_model::firstparty_file_id(unit) {
        return None;
    }
    let text = unit.source.get(decl.span.start..decl.span.end)?;
    if text.contains("type Secret =") {
        Some(SecretKind::Secret)
    } else if text.contains("type Authorization =") {
        Some(SecretKind::Authorization)
    } else if text.contains("type ApiKeyValue =") {
        Some(SecretKind::ApiKeyValue)
    } else {
        None
    }
}

/// #4111: whether `name`, in the project's type table, is a secret-carrying `bynk` type
/// (`Secret` or `Authorization`), whatever it is spelled here, so a user type that happens to be
/// called `Secret` is not mistaken for it.
pub fn is_secret_type(
    name: &str,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> bool {
    types.get(name).is_some_and(|decl| is_secret_carrier(decl))
}

/// #4111: whether the named type `name` is, or holds at any depth in a record field or variant
/// payload, a `Secret` or an `Authorization`. `bynk`'s own `Request` holds one, so a `Request` has
/// no codec, though the field itself is allowed there.
pub fn type_carries_secret(
    name: &str,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> bool {
    fn walk(
        name: &str,
        types: &std::collections::HashMap<String, Arc<TypeDecl>>,
        seen: &mut HashSet<String>,
    ) -> bool {
        let Some(decl) = types.get(name) else {
            return false;
        };
        if is_secret_carrier(decl) {
            return true;
        }
        if !seen.insert(name.to_string()) {
            return false;
        }
        let fields: Vec<&TypeRef> = match &decl.body {
            bynk_syntax::ast::TypeBody::Record(r) => r.fields.iter().map(|f| &f.type_ref).collect(),
            bynk_syntax::ast::TypeBody::Sum(s) => s
                .variants
                .iter()
                .flat_map(|v| v.payload.iter().map(|f| &f.type_ref))
                .collect(),
            _ => Vec::new(),
        };
        fields.into_iter().any(|f| {
            bynk_syntax::ast::type_ref_heads(f)
                .iter()
                .any(|h| walk(h, types, seen))
        })
    }
    walk(name, types, &mut HashSet::new())
}

/// #4111: the types the `bynk` adapter declares, by name, from its embedded source (built once).
fn bynk_types() -> &'static HashMap<String, Arc<TypeDecl>> {
    static TYPES: std::sync::OnceLock<HashMap<String, Arc<TypeDecl>>> = std::sync::OnceLock::new();
    TYPES.get_or_init(|| {
        crate::checker::std_type_decls()
            .iter()
            .map(|t| (t.name.name.clone(), Arc::clone(t)))
            .collect()
    })
}

/// #4111: whether `decl` is one the `bynk` adapter itself ships.
pub fn is_firstparty_decl(decl: &TypeDecl) -> bool {
    crate::firstparty::unit(bynk_std::BYNK_UNIT)
        .is_some_and(|unit| decl.span.file == crate::project_model::firstparty_file_id(unit))
}

/// #4111: whether `decl` is `Secret` or `Authorization`: the types that are never stored, sent,
/// encoded or compared, and that the emitter builds as a runtime `BynkSecret`.
pub fn is_secret_carrier(decl: &TypeDecl) -> bool {
    secret_kind(decl).is_some()
}

/// #4111: whether `decl` is `Secret` itself, the one whose `of` cannot fail.
pub fn is_secret_decl(decl: &TypeDecl) -> bool {
    secret_kind(decl) == Some(SecretKind::Secret)
}

/// #3276: whether `name` is the `File` record `bynk` exports, found by its
/// declaration's first-party span (as `is_crypto_key_type` finds a key), so a
/// user type that happens to be called `File` is not mistaken for it.
pub fn is_std_file_type(
    name: &str,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> bool {
    name == "File"
        && types.get(name).is_some_and(|decl| {
            crate::firstparty::unit(bynk_std::BYNK_UNIT).is_some_and(|unit| {
                decl.span.file == crate::project_model::firstparty_file_id(unit)
            })
        })
}

/// #3276: whether `r` is a `@multipart` file field: `File`, or an `Option` or a
/// `List` of it (one layer, as a form leaf is). `File` is the record `bynk`
/// exports (`consumes bynk { File }`).
fn is_file_field_type(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    let named = |t: &TypeRef| matches!(t, TypeRef::Named(id) if is_std_file_type(&id.name, types));
    match r {
        TypeRef::Option(inner, _) | TypeRef::List(inner, _) => named(inner),
        other => named(other),
    }
}

/// #3276: the field path by which `t` reaches a `File`, if it does: `Some(vec![])`
/// for a `File` itself, otherwise the record fields (or sum payloads) through
/// which a type carries one, as [`bynk_syntax::ast::stream_path`] does a stream.
fn file_path(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> Option<Vec<String>> {
    fn walk(
        t: &TypeRef,
        types: &HashMap<String, Arc<TypeDecl>>,
        visited: &mut std::collections::HashSet<String>,
    ) -> Option<Vec<String>> {
        match t {
            TypeRef::Named(id) if is_std_file_type(&id.name, types) => Some(Vec::new()),
            TypeRef::Named(id) => named(&id.name, types, visited),
            TypeRef::App { name, args, .. } => named(&name.name, types, visited)
                .or_else(|| args.iter().find_map(|a| walk(a, types, visited))),
            TypeRef::Option(a, _) | TypeRef::List(a, _) | TypeRef::Set(a, _) => {
                walk(a, types, visited)
            }
            TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
                walk(a, types, visited).or_else(|| walk(b, types, visited))
            }
            _ => None,
        }
    }
    fn named(
        name: &str,
        types: &HashMap<String, Arc<TypeDecl>>,
        visited: &mut std::collections::HashSet<String>,
    ) -> Option<Vec<String>> {
        if !visited.insert(name.to_string()) {
            return None;
        }
        let decl = types.get(name)?;
        let found = match &decl.body {
            TypeBody::Record(r) => r.fields.iter().find_map(|f| {
                walk(&f.type_ref, types, visited).map(|mut p| {
                    p.insert(0, f.name.name.clone());
                    p
                })
            }),
            TypeBody::Sum(s) => s.variants.iter().find_map(|v| {
                v.payload.iter().find_map(|f| {
                    walk(&f.type_ref, types, visited).map(|mut p| {
                        p.insert(0, f.name.name.clone());
                        p
                    })
                })
            }),
            _ => None,
        };
        visited.remove(name);
        found
    }
    walk(t, types, &mut std::collections::HashSet::new())
}

/// #3276: the `body` of a `@form` handler is a record whose fields are each a
/// form leaf: the typed `query` set (a `String`, `Int`, `Bool` or `Decimal`, a
/// refined or opaque type over one, an enum), optionally in an `Option` or a
/// `List` (a repeated key). A `Bool` that is neither optional, listed nor
/// defaulted is a warning: an unchecked checkbox sends nothing.
fn validate_form_body(
    p: &Param,
    kind: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    let multipart = kind == "multipart";
    let record = match &p.type_ref {
        TypeRef::Named(id) => match types.get(&id.name).map(|t| &t.body) {
            Some(TypeBody::Record(r)) => Some(r),
            _ => None,
        },
        _ => None,
    };
    let Some(record) = record else {
        errors.push(
            CompileError::new(
                "bynk.http.form_body_not_record",
                p.type_ref.span(),
                format!(
                    "the `body` of a `@{kind}` handler must be a record type (got `{}`)",
                    ts_type_ref_display(&p.type_ref),
                ),
            )
            .with_note(
                "a form body decodes into a record, one field per form key: `body: SignupForm`",
            ),
        );
        return;
    };
    for f in &record.fields {
        let is_file = multipart && is_file_field_type(&f.type_ref, types);
        if !is_file && !is_query_field_type(&f.type_ref, types) {
            errors.push(
                CompileError::new(
                    "bynk.http.form_field_not_stringy",
                    f.type_ref.span(),
                    format!(
                        "field `{}` of the `@{kind}` body record has type `{}`, which a {kind} body cannot carry",
                        f.name.name,
                        ts_type_ref_display(&f.type_ref),
                    ),
                )
                .with_note(if multipart {
                    "a multipart field is a `String`, `Int`, `Bool`, `Decimal`, an enum, or a refined or opaque type over one, or a `File`, optionally in an `Option` or a `List` (a repeated part); keys are flat, so a nested record is not allowed"
                } else {
                    "a form field is a `String`, `Int`, `Bool`, `Decimal`, an enum, or a refined or opaque type over one, optionally in an `Option` or a `List` (a repeated key); form keys are flat, so a nested record is not allowed"
                }),
            );
        } else if f.init.is_none() && is_bool_leaf(&f.type_ref, types) {
            errors.push(
                CompileError::new(
                    "bynk.http.form_bool_without_default",
                    f.type_ref.span(),
                    format!(
                        "`Bool` field `{}` of a `@{kind}` body has no default, so an unchecked checkbox is a `400`",
                        f.name.name,
                    ),
                )
                .with_note(
                    "a browser sends nothing for an unchecked checkbox; give the field `= false`, or make it an `Option[Bool]`",
                ),
            );
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
    let mut no_store: Option<&AnnotationArg> = None;
    for arg in &ann.args {
        match arg.label.as_ref().map(|l| l.name.as_str()) {
            Some("maxAge") => max_age = Some(arg),
            Some("scope") => scope = Some(arg),
            Some("immutable") => immutable = Some(arg),
            Some("noStore") => no_store = Some(arg),
            _ => {
                errors.push(
                    CompileError::new(
                        "bynk.http.cache_unknown_arg",
                        arg.span,
                        "`@cache` accepts only the `maxAge:`, `scope:`, `immutable:` and `noStore:` arguments",
                    )
                    .with_note("write `@cache(maxAge: 5.minutes, scope: private)`"),
                );
            }
        }
    }
    // #3922: `noStore: true` sends `no-store`, so no freshness argument may go with it.
    if let Some(arg) = no_store {
        match &arg.value.kind {
            ExprKind::BoolLit(true) => {
                for other in [max_age, scope, immutable].into_iter().flatten() {
                    errors.push(
                        CompileError::new(
                            "bynk.http.cache_no_store_conflict",
                            other.span,
                            "`@cache(noStore: true)` stores nothing, so it takes no `maxAge`, `scope` or `immutable`",
                        )
                        .with_label(arg.span, "`noStore: true` is declared here")
                        .with_note("drop `noStore: true` to cache, or drop the freshness arguments to forbid storing"),
                    );
                }
                return;
            }
            ExprKind::BoolLit(false) => {}
            _ => {
                errors.push(CompileError::new(
                    "bynk.http.cache_bad_no_store",
                    arg.span,
                    "`@cache` `noStore` must be `true` or `false`",
                ));
                return;
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
            let arg = max_age.unwrap();
            errors.push(crate::annotation_consts::with_const_origin(
                CompileError::new(
                    "bynk.http.cache_bad_max_age",
                    arg.span,
                    "`@cache` `maxAge` must be a positive `Duration` literal (e.g. `5.minutes`), or a `Duration` constant bound to one",
                ),
                arg,
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
fn validate_limit_args(ann: &Annotation, is_multipart: bool, errors: &mut Vec<CompileError>) {
    let mut max_body: Option<&AnnotationArg> = None;
    let mut multipart_args = 0;
    for arg in &ann.args {
        match arg.label.as_ref().map(|l| l.name.as_str()) {
            Some("maxBody") => max_body = Some(arg),
            // #3276: the per-file and part-count caps of a `@multipart` route.
            Some(name @ ("maxFile" | "maxParts")) => {
                multipart_args += 1;
                if !is_multipart {
                    errors.push(
                        CompileError::new(
                            "bynk.http.limit_on_non_multipart",
                            arg.span,
                            format!("`@limit` `{name}` is only valid on a `@multipart` route"),
                        )
                        .with_note(
                            "a per-file or part-count cap governs the parts of a multipart body",
                        ),
                    );
                } else if !matches!(&arg.value.kind, ExprKind::IntLit { value: n, .. } if *n > 0) {
                    errors.push(crate::annotation_consts::with_const_origin(
                        CompileError::new(
                            "bynk.http.limit_bad_max_file",
                            arg.span,
                            format!(
                                "`@limit` `{name}` must be a positive `Int` literal, or an `Int` constant bound to one"
                            ),
                        ),
                        arg,
                    ));
                }
            }
            _ => {
                errors.push(
                    CompileError::new(
                        "bynk.http.limit_unknown_arg",
                        arg.span,
                        "`@limit` accepts only the `maxBody:` argument (and, on a `@multipart` route, `maxFile:` and `maxParts:`)",
                    )
                    .with_note("write `@limit(maxBody: 26_214_400)`"),
                );
            }
        }
    }
    // A `@limit` that only tightens a multipart route's file or part caps may leave
    // `maxBody` to the service's `limits { }`.
    if max_body.is_none() && multipart_args > 0 {
        return;
    }
    // `maxBody` is required and must be a *positive* `Int` literal — a byte count.
    match max_body.map(|a| &a.value.kind) {
        Some(ExprKind::IntLit { value: n, .. }) if (1..=MAX_BODY_CEILING).contains(n) => {}
        Some(_) => {
            let arg = max_body.unwrap();
            errors.push(crate::annotation_consts::with_const_origin(
                CompileError::new(
                    "bynk.http.limit_bad_max_body",
                    arg.span,
                    "`@limit` `maxBody` must be a positive `Int` literal — a byte count (e.g. `26_214_400`), or an `Int` constant bound to one — no larger than 524_288_000",
                )
                .with_note(MAX_BODY_NOTE),
                arg,
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
        // #3912: a non-empty list of media types; `"*/*"` cannot be one of several.
        Some(ExprKind::ListLit(items, ..))
            if !items.is_empty()
                && items.iter().all(|e| {
                    matches!(&e.kind, ExprKind::StrLit(s)
                        if bynk_syntax::ast::is_media_type(s) && s != "*/*")
                }) => {}
        Some(_) => {
            let arg = content_type.unwrap();
            errors.push(crate::annotation_consts::with_const_origin(
                CompileError::new(
                    "bynk.http.raw_bad_content_type",
                    arg.span,
                    "`@raw` `contentType` must be a media type string literal (or a `String` constant bound to one) — `type/subtype`, or `\"*/*\"` to accept any — or a non-empty list of `type/subtype` literals",
                ),
                arg,
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

/// #3874 [DECISION A, B]: a cron handler takes up to two parameters, told apart by
/// type, in either order: an `Instant` (the scheduled time) and exactly the
/// prelude's `CronSchedule` (the schedule that fired the run). A second parameter
/// of the same type, a third, or any other type is `bynk.cron.bad_params`. A
/// `CronSchedule` the unit declares itself is not the prelude's, whatever it is
/// spelled like: the entry hands the parameter a schedule string, which only the
/// prelude type admits.
fn check_cron_params(handler: &Handler, table: &UnitTable, errors: &mut Vec<CompileError>) {
    use bynk_syntax::cron::{CronParam, cron_param};
    let own_schedule_type = table.types.contains_key("CronSchedule");
    let mut seen: Vec<CronParam> = Vec::new();
    for (i, p) in handler.params.iter().enumerate() {
        if i >= 2 {
            errors.push(
                CompileError::new(
                    "bynk.cron.bad_params",
                    p.span,
                    "an `on schedule` handler takes at most two parameters",
                )
                .with_note(
                    "a scheduled trigger carries two things: the time it fired (`Instant`) and the schedule that fired it (`CronSchedule`)",
                ),
            );
            continue;
        }
        let kind = cron_param(&p.type_ref);
        if kind == Some(CronParam::Schedule) && own_schedule_type {
            errors.push(
                CompileError::new(
                    "bynk.cron.bad_params",
                    p.type_ref.span(),
                    "this `CronSchedule` is a type this unit declares, not the prelude's",
                )
                .with_note(
                    "a schedule parameter must be the `CronSchedule` that `consumes bynk { CronSchedule }` provides: the entry hands it the schedule string that fired the run",
                ),
            );
            continue;
        }
        let Some(kind) = kind else {
            errors.push(
                CompileError::new(
                    "bynk.cron.bad_params",
                    p.type_ref.span(),
                    format!(
                        "an `on schedule` parameter must be `Instant` (the scheduled time) or `CronSchedule` (the schedule that fired), got `{}`",
                        ts_type_ref_display(&p.type_ref),
                    ),
                )
                .with_note(
                    "declare `at: Instant`; where the body needs the number, use `at.toEpochMillis()`. `schedule: CronSchedule` is the expression that fired this run",
                ),
            );
            continue;
        };
        if seen.contains(&kind) {
            let what = match kind {
                CronParam::Time => "an `Instant`",
                CronParam::Schedule => "a `CronSchedule`",
            };
            errors.push(
                CompileError::new(
                    "bynk.cron.bad_params",
                    p.span,
                    format!("an `on schedule` handler takes at most one parameter of each type, but this is a second {what}"),
                )
                .with_note(
                    "a scheduled trigger carries two things: the time it fired (`Instant`) and the schedule that fired it (`CronSchedule`)",
                ),
            );
        }
        seen.push(kind);
    }
}

/// Two cron handlers of a context may not declare the same schedule: the
/// generated `scheduled` dispatcher routes on `event.cron`. #3875: a schedule
/// constant that was refused has no expression to compare, and is already
/// reported.
fn check_duplicate_schedule(
    handler: &Handler,
    expr: &str,
    seen: &mut HashMap<String, Span>,
    errors: &mut Vec<CompileError>,
) {
    if handler
        .schedule_const
        .as_ref()
        .is_some_and(|s| s.refusal.is_some())
    {
        return;
    }
    if let Some(prev) = seen.get(expr).copied() {
        errors.push(
            CompileError::new(
                "bynk.cron.duplicate_schedule",
                handler.span,
                format!("duplicate cron schedule: another handler already declares `{expr}`"),
            )
            .with_label(prev, "previously declared here"),
        );
    } else {
        seen.insert(expr.to_string(), handler.span);
    }
}

/// #3875 [DECISION D]: a schedule constant that does not denote a schedule. An
/// unknown name is the ordinary unknown name (so a private constant of a used
/// commons reads as `bynk.visibility.private_item`); a constant of another type
/// is `bynk.cron.invalid_schedule` with a note naming the constant and its
/// declared type.
fn refused_schedule_const(
    sc: &bynk_syntax::ast::ScheduleConst,
    refusal: &bynk_syntax::ast::ScheduleRefusal,
) -> CompileError {
    use bynk_syntax::ast::ScheduleRefusal;
    let name = &sc.name.name;
    match refusal {
        ScheduleRefusal::Unknown => CompileError::new(
            "bynk.resolve.unknown_name",
            sc.name.span,
            format!("the schedule in `on schedule(…)` names an unknown constant `{name}`"),
        )
        .with_note("a schedule is a string literal, or a `String` or `CronSchedule` constant"),
        ScheduleRefusal::Declared(ty) => CompileError::new(
            "bynk.cron.invalid_schedule",
            sc.name.span,
            format!("`{name}` is not a cron schedule: it is declared `{ty}`"),
        )
        .with_note(format!(
            "`schedule(…)` takes a string literal, or a constant declared exactly `String` or `CronSchedule` and bound to a literal; `const {name}` is declared `{ty}`"
        )),
    }
}

/// #3873: the schedule must be valid Cloudflare cron syntax
/// ([`bynk_syntax::cron::is_cron_expression`]); the message names the failing
/// field. A valid one may draw the two advisory warnings (`numeric_weekday`,
/// `uneven_step`), both silenced by writing a name or an explicit list.
fn check_cron_schedule(handler: &Handler, expr: &str, errors: &mut Vec<CompileError>) {
    if let Some(sc) = &handler.schedule_const
        && let Some(refusal) = &sc.refusal
    {
        errors.push(refused_schedule_const(sc, refusal));
        return;
    }
    match bynk_syntax::cron::is_cron_expression(expr) {
        Err(fault) => {
            let mut err = CompileError::new(
                "bynk.cron.invalid_schedule",
                handler.span,
                format!("cron schedule `{expr}` is invalid: {}", fault.message()),
            )
            .with_note(
                "the fields are: minute hour day-of-month month day-of-week, separated by exactly one space",
            );
            let single_spaced = expr.split_whitespace().collect::<Vec<_>>().join(" ");
            if single_spaced != expr
                && bynk_syntax::cron::is_cron_expression(&single_spaced).is_ok()
            {
                err = err.with_note(format!(
                    "write it with single spaces: `{single_spaced}` (Cloudflare matches the schedule character for character)"
                ));
            }
            errors.push(err);
        }
        Ok(()) => {
            if bynk_syntax::cron::numeric_weekday(expr) {
                errors.push(
                    CompileError::new(
                        "bynk.cron.numeric_weekday",
                        handler.span,
                        format!("cron schedule `{expr}` names a day of the week by number"),
                    )
                    .with_note(
                        "on Cloudflare `1` is Sunday, so a number is a day earlier than standard cron's (`5` is Thursday, not Friday); write the name (`FRI`) to be explicit",
                    ),
                );
            }
            if let Some(when) = bynk_syntax::cron::uneven_step(expr) {
                errors.push(
                    CompileError::new(
                        "bynk.cron.uneven_step",
                        handler.span,
                        format!("cron schedule `{expr}` has an uneven step"),
                    )
                    .with_note(when)
                    .with_note(
                        "list the times you mean, or use a step that divides the field's range",
                    ),
                );
            }
        }
    }
}

/// Validate an `on schedule("expr") (at: Instant?) -> Effect[Result[(), E]]`
/// handler (v0.10a §4.1): at most one `Instant` parameter (the scheduled time), a structurally well-formed schedule, and the unit-Result
/// return shape. The service-only rule is enforced earlier, in the parser
/// (`bynk.parse.handler_in_agent`).
fn validate_cron_handler(
    handler: &Handler,
    expr: &str,
    table: &UnitTable,
    errors: &mut Vec<CompileError>,
) {
    check_cron_params(handler, table, errors);
    check_cron_schedule(handler, expr, errors);
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
fn validate_queue_handler(
    handler: &Handler,
    name: &str,
    table: &UnitTable,
    errors: &mut Vec<CompileError>,
) {
    if name.is_empty() {
        errors.push(CompileError::new(
            "bynk.queue.invalid_name",
            handler.span,
            "`from queue` requires a non-empty queue name",
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
    // #3880: the delivery parameter is the `QueueDelivery` of `bynk.cloudflare`. A type of the
    // same name that this context declares is the author's own, not the platform's, and is
    // refused rather than taken for it, since later stages recognise the parameter by its name.
    if table.types.contains_key("QueueDelivery")
        && let Some(p) = handler
            .params
            .iter()
            .find(|p| bynk_syntax::queue::is_queue_delivery(&p.type_ref))
    {
        errors.push(
            CompileError::new(
                "bynk.queue.bad_params",
                p.type_ref.span(),
                "this context declares its own `QueueDelivery`, which is not the delivery metadata of `bynk.cloudflare`",
            )
            .with_note(
                "rename your type, or `consumes bynk.cloudflare { QueueDelivery }` and use that one; the delivery parameter of an `on message` handler is the platform's",
            ),
        );
    }
    // The message, and optionally the delivery metadata (#3880): at most two
    // parameters, the second being exactly one `QueueDelivery`, in either order.
    let deliveries =
        bynk_syntax::queue::delivery_positions(handler.params.iter().map(|p| &p.type_ref));
    let messages = handler.params.len() - deliveries.len();
    if messages != 1 || deliveries.len() > 1 {
        let why = if deliveries.len() > 1 {
            format!(
                "`on message` handlers take one `QueueDelivery` at most, got {}",
                deliveries.len()
            )
        } else if messages == 0 {
            "`on message` handlers take the message as a parameter, and a `QueueDelivery` is not a message"
                .to_string()
        } else {
            format!(
                "`on message` handlers take the message and optionally a `QueueDelivery`, got {} parameters besides it",
                messages - 1
            )
        };
        errors.push(
            CompileError::new("bynk.queue.bad_params", handler.span, why).with_note(
                "a queue consumer processes one message per invocation; add `delivery: QueueDelivery` (`consumes bynk.cloudflare { QueueDelivery }`) to read its attempt count and send time",
            ),
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
pub(crate) fn is_string_constructible(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
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
/// #3343: the `cookies`, `query` or `@form` `body` record of an `on http` handler:
/// the one position where a record with `@name` fields is allowed. Every other
/// boundary check still applies to it.
pub const REQUEST_RECORD_PARAM: &str = "a request record parameter";

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
        // #2854 (decision B): `RequestQuery`, the same way — read in the
        // handler's `query` parameter, never stored or sent.
        TypeRef::RequestQuery(span) => {
            errors.push(
                CompileError::new(
                    "bynk.types.request_query_at_boundary",
                    *span,
                    format!(
                        "a `RequestQuery` type cannot appear in {what} — it is the request's query string, read in the handler that receives it, never persisted or sent"
                    ),
                )
                .with_note(
                    "read the value you need (`query.get(\"…\")`) and store or send that value instead",
                ),
            );
        }
        // #3277: `RequestCookies`, the same way — read in the handler's `cookies`
        // parameter, never stored or sent.
        TypeRef::RequestCookies(span) => {
            errors.push(
                CompileError::new(
                    "bynk.types.request_cookies_at_boundary",
                    *span,
                    format!(
                        "a `RequestCookies` type cannot appear in {what} — it is the request's cookies, read in the handler that receives it, never persisted or sent"
                    ),
                )
                .with_note(
                    "read the value you need (`cookies.get(\"…\")`) and store or send that value instead",
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
        | TypeRef::HttpResult(a, _, _)
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
        // #3192: a *recursive* generic (one that transitively contains itself)
        // has no codec yet, but declares and works in memory, so — as for a
        // stream (K-streams, #2623) — a record field, a sum payload and a
        // capability signature pass it, and it is refused where it is used at
        // a position that serialises, naming the field path when it is reached
        // through another type (the resolver's `recursive_record_field` guard
        // only catches a direct self-edge in a non-generic record).
        TypeRef::App { name, args, span } => {
            crate::app_boundary::reject_app(name, args, *span, what, types, errors);
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
            errors.push(statement_at_boundary(id.span, what, &id.name));
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
                    "keep the key bytes in `Secrets` and import the key again where it is used (`Crypto.importSigningKey`, `Crypto.importVerifyKey` or `Crypto.importCipherKey`)",
                ),
            );
        }
        // #4111: a `Secret` has no codec, so it is neither storable nor boundary-crossing:
        // it would leak the value into storage, a wire body or a log. A capability
        // operation is where one is returned (`Secrets.get`), in-process, so its signature
        // is exempt.
        // #3939 (D2 D): an `ApiKeyValue`, or a record or sum that holds one (the `ApiKeySecret` that
        // `mint` answers), would carry the key into storage, a wire body or a log. Its one exit is an
        // HTTP handler returning it (`api_key::reject_http_result`), which never reaches here.
        TypeRef::Named(id)
            if what != CAPABILITY_SIGNATURE
                && crate::api_key::carries_api_key_value(&id.name, types) =>
        {
            errors.push(crate::api_key::key_at_boundary_error(
                id.span, &id.name, what,
            ));
        }
        TypeRef::Named(id) if what != CAPABILITY_SIGNATURE && is_secret_type(&id.name, types) => {
            errors.push(secret_at_boundary_error(id.span, &id.name, what));
        }
        // #3752 (D11): an `InboundRequest` is the request a pass-through route is
        // answering, and a `Forwarded` an upstream's answer streaming through it:
        // each lives for that request, never stored or sent. `Upstream.forward`
        // takes and returns them, so a capability signature is exempt.
        // A record or sum may hold one, and is refused where it is used, as for a
        // stream.
        TypeRef::Named(id)
            if !stream_passes(what)
                && crate::http_pass_through::held_in(&id.name, types).is_some() =>
        {
            let held = crate::http_pass_through::held_in(&id.name, types).unwrap_or_default();
            errors.push(crate::http_pass_through::held_at_boundary(id, held, what));
        }
        // #3276: a `File` is a file part of a `@multipart` request, held in
        // memory for the handler that receives it. A record field or sum payload
        // may carry one (that is how a `@multipart` body names its parts), but it
        // is never persisted, sent across a boundary or returned.
        TypeRef::Named(id) if !stream_passes(what) && is_std_file_type(&id.name, types) => {
            errors.push(file_at_boundary(&id.name, &[], id.span, what));
        }
        // K-streams (#2623): a record or sum that carries a stream, at any
        // depth, is non-boundary like the stream.
        TypeRef::Named(id) if !stream_passes(what) => {
            if let Some(path) = file_path(r, types) {
                errors.push(file_at_boundary(&id.name, &path, id.span, what));
            }
            if let Some(path) = bynk_syntax::ast::stream_path(r, types) {
                errors.push(stream_carrying_at_boundary(&id.name, &path, id.span, what));
            }
            // #3192: a type that merely contains a recursive generic.
            if let Some(rg) = bynk_syntax::ast::recursive_generic_path(r, types) {
                errors.push(recursive_generic_at_boundary(&rg, id.span, what));
            }
        }
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::QueueResult(_)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_) => {}
    }
}

/// #3276: `bynk.types.file_at_boundary` for a `File`, or a type that carries one,
/// at a position that persists or sends it.
fn file_at_boundary(name: &str, path: &[String], span: Span, what: &str) -> CompileError {
    let reach = if path.is_empty() {
        String::new()
    } else {
        let chain = path
            .iter()
            .map(|f| format!("`{f}`"))
            .collect::<Vec<_>>()
            .join(", then ");
        format!(" (it is reached through {chain})")
    };
    CompileError::new(
        "bynk.types.file_at_boundary",
        span,
        format!(
            "`{name}` cannot appear in {what} — it holds a file part of a multipart request in memory, never persisted, sent across a boundary or returned{reach}"
        ),
    )
    .with_note(
        "take the part's `bytes` (and `filename`, `contentType`) in the `@multipart` handler and store them where they belong, as `R2.put(key, f.bytes)` does",
    )
}

/// K-streams (#2623): the positions a `Stream` may appear in. A capability
/// operation's provider runs in the caller's isolate, so nothing in its
/// signature is serialised; a record field or sum payload makes its type
/// stream-carrying, which is then checked where the type is used.
pub(crate) fn stream_passes(what: &str) -> bool {
    [CAPABILITY_SIGNATURE, RECORD_FIELD, SUM_PAYLOAD].contains(&what)
}

/// #3192: `bynk.generics.recursive_generic_at_boundary` for a recursive generic
/// type at a position that serialises. The noun comes from the declaration
/// (`record` or `sum`); a type that only contains the generic names the field
/// chain it is reached through.
pub(crate) fn recursive_generic_at_boundary(
    rg: &bynk_syntax::ast::RecursiveGenericPath,
    span: Span,
    what: &str,
) -> CompileError {
    let err = CompileError::new(
        "bynk.generics.recursive_generic_at_boundary",
        span,
        format!(
            "recursive generic {} `{}` cannot appear in {what} — it has no codec yet",
            rg.noun, rg.name
        ),
    );
    let chain = rg
        .path
        .iter()
        .map(|f| format!("`{f}`"))
        .collect::<Vec<_>>()
        .join(", then ");
    let reach = if rg.path.is_empty() {
        String::new()
    } else {
        format!(" (it is reached through {chain})")
    };
    err.with_note(format!(
        "a recursive generic type declares and works inside a context, but has no codec yet{reach}; \
         use a concrete recursive type, or keep it inside the context"
    ))
}

/// K-streams (#2623): `bynk.types.stream_at_boundary` for a type that carries
/// a stream in a field, naming the field chain it is reached through.
pub(crate) fn stream_carrying_at_boundary(
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
pub(crate) fn is_crypto_key_type(
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
            test_only: false,
            name: ctx.name,
            items: ctx.items,
            uses: ctx.uses,
            consumes: Vec::new(),
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
        // Mirrors `bynk-tests/tests/fixtures/positive/916_bytes_http_sum_body`:
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
  on call(ping: String) -> Effect[String] by Caller {
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
        // reads the same either way. Three handlers, only two with a
        // binder, pins both: each binder lands on its own handler's own
        // span, and the binder-less handler contributes no entry at all.
        // #3766: a service has one `on call`, so the three are http routes.
        let program = checked_context_program(
            r#"
context demo

type UserId = String

actor Buyer { auth = Bearer(secret = "AUTH_JWT_SECRET"), identity = UserId }

service Api from http {
  on GET("/a") () -> Effect[HttpResult[String]] by u: Buyer {
    Ok("a")
  }
  on GET("/b") () -> Effect[HttpResult[String]] by v: Buyer {
    Ok("b")
  }
  on GET("/c") () -> Effect[HttpResult[String]] by Visitor {
    Ok("c")
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
            "the third handler binds no actor (`by Visitor`, no binder)"
        );
        assert_eq!(
            program.program().actor_bindings.len(),
            2,
            "exactly the two `by`-bearing handlers, nothing extra persisted for the third"
        );
    }
}

#[cfg(test)]
mod secret_kind_tests {
    use super::*;

    /// #4111, #3939: `secret_kind` finds `Secret`, `Authorization` and `ApiKeyValue` by their declaration, so any other
    /// opaque type added to `bynk` would be neither restricted nor emitted as a secret. This pins
    /// the set: adding an opaque type to `bynk.bynk` fails here, pointing at `secret_kind`.
    /// #3752: `InboundRequest` and `Forwarded` are opaque and not secrets; their own rules are
    /// `crate::http_pass_through`'s (`held_in`).
    #[test]
    fn bynk_declares_exactly_five_opaque_types_and_secret_kind_names_each() {
        let opaque: Vec<(&str, Option<SecretKind>)> = crate::checker::std_type_decls()
            .iter()
            .filter(|t| matches!(t.body, bynk_syntax::ast::TypeBody::Opaque { .. }))
            .map(|t| (t.name.name.as_str(), secret_kind(t)))
            .collect();
        let mut names: Vec<&str> = opaque.iter().map(|(n, _)| *n).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["ApiKeyValue", "Authorization", "Forwarded", "InboundRequest", "Secret"],
            "`bynk` declares an opaque type that `secret_kind` does not know: decide whether it is \
             secret-carrying, and extend `secret_kind` and this test"
        );
        for (name, kind) in opaque {
            let want = match name {
                "Secret" => Some(SecretKind::Secret),
                "ApiKeyValue" => Some(SecretKind::ApiKeyValue),
                "Authorization" => Some(SecretKind::Authorization),
                _ => None,
            };
            assert_eq!(kind, want, "{name}");
        }
    }
}
