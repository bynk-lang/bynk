//! Expression and pattern checking.
//!
//! Split out of `checker.rs` (v0.29.10) verbatim; the parent module
//! re-exports these via `use expressions::*`.

use super::*;

/// v0.26 (ADR 0054): the deletion span for `given` entry `i`, list-aware so
/// the result never double-commas, leading-commas, or leaves `given ,`:
/// an entry with a successor deletes through the successor's start
/// (`C1, `); a final entry deletes from its predecessor's end (`, C2`); the
/// only entry deletes from the return type's end — the `given` keyword goes
/// with it (no dangling `given`).
pub(crate) fn given_removal_span(
    entries: &[(String, Span)],
    i: usize,
    return_ty_span: Span,
) -> Span {
    if entries.len() == 1 {
        Span::new(return_ty_span.end, entries[0].1.end)
    } else if i + 1 < entries.len() {
        Span::new(entries[i].1.start, entries[i + 1].1.start)
    } else {
        Span::new(entries[i - 1].1.end, entries[i].1.end)
    }
}

/// v0.26 (ADR 0054): the insertion edit that adds `name` to the `given`
/// clause — `, name` after the last entry, or ` given name` synthesised at
/// the anchor (the handler's return type) when the clause is absent. `None`
/// when there is no clause and no sound anchor (provider bodies — their
/// clause lives on the `provides` line, not at the op's return type).
pub(crate) fn given_insertion_edit(
    entries: &[(String, Span)],
    anchor: Option<Span>,
    name: &str,
) -> Option<(Span, String)> {
    if let Some((_, last)) = entries.last() {
        Some((Span::new(last.end, last.end), format!(", {name}")))
    } else {
        anchor.map(|a| (Span::new(a.end, a.end), format!(" given {name}")))
    }
}

// -- type-declaration validation --

pub(crate) fn check_ident(id: &Ident, expected: Option<TyId>, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    if let Some(ty) = ctx.lookup(id.name.as_str()) {
        // #1663: a name bound to the error type (a pattern that failed to
        // check) was already reported; typing its use as unknown (`None`)
        // lets every enclosing check absorb it instead of reporting again.
        if matches!(&*tys.get(ty), Ty::Error) {
            return None;
        }
        return Some(ty);
    }
    // #1883, slice 1a: an `asset` name is a value of type `Asset`. Locals
    // shadow it (checked above), as they shadow a function; an asset never
    // shares a name with a function or type in scope (`bynk.resolve.
    // name_conflict` at declaration).
    if ctx.input.assets.contains_key(&id.name) {
        // #1883: a use edge to the declaration, recorded here because this
        // one site sees both fn and handler bodies.
        ctx.refs.record(id.span, SymbolKind::Asset, &id.name);
        return Some(tys.intern(Ty::Asset));
    }
    // #2387, slice 1: an `embed` name is a `String`. Locals shadow it
    // (checked above); like an asset it never shares a name with a function or
    // type in scope (`bynk.resolve.name_conflict` at declaration).
    if ctx.input.embeds.contains_key(&id.name) {
        // #2387: a use edge to the declaration, as for an asset.
        ctx.refs.record(id.span, SymbolKind::Embed, &id.name);
        return Some(tys.intern(Ty::Base(BaseType::String)));
    }
    // v0.20a: a named function referenced as a *value* where a function type
    // is expected (the contextual relaxation of `bynk.resolve.fn_without_call`,
    // relocated here from the resolver). A Var-bearing expected (generic
    // instantiation, pass 1) counts as a function-type expectation.
    if let Some(fn_decl) = ctx.input.fns.get(&id.name).cloned() {
        let fn_expected = expected.is_some_and(|e| matches!(&*tys.get(e), Ty::Fn { .. }));
        if fn_expected {
            ctx.refs.record(id.span, SymbolKind::Fn, &id.name);
            // S3 (#1963): a function passed as a value is a use too.
            super::calls::refuse_experimental_import(id, ctx);
            if !fn_decl.type_params.is_empty() {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.generics.uninferable_type_arg",
                        id.span,
                        format!(
                            "generic function `{}` cannot be passed as a value in v0.20a — its type parameters cannot be instantiated here",
                            id.name
                        ),
                    )
                    .with_note("wrap it in a lambda, or call it directly"),
                );
                return None;
            }
            let params: Option<Vec<TyId>> = fn_decl
                .params
                .iter()
                .map(|p| resolve_type_ref(&p.type_ref, &ctx.input.types, tys))
                .collect();
            let ret = resolve_type_ref(&fn_decl.return_type, &ctx.input.types, tys)?;
            return Some(tys.intern(Ty::Fn {
                params: params?,
                ret,
            }));
        }
        // Bare reference outside a function-typed position: the original
        // rule, with the checker's type knowledge behind it.
        ctx.errors.push(
            CompileError::new(
                "bynk.resolve.fn_without_call",
                id.span,
                format!(
                    "`{}` is a function — call it (`{}(…)`), or pass it where a function type is expected",
                    id.name, id.name
                ),
            )
            .with_note(
                "a bare function reference is only a value in a function-typed position (v0.20a)",
            ),
        );
        return None;
    }
    // Bare variant of a unique-owner sum type (nullary variants).
    let owners: Vec<&TypeDecl> = ctx
        .input
        .types
        .values()
        .map(std::sync::Arc::as_ref)
        .filter(|t| matches!(&t.body, TypeBody::Sum(s) if s.variants.iter().any(|v| v.name.name == id.name)))
        .collect();
    if owners.len() == 1 {
        let owner = owners[0];
        if let TypeBody::Sum(s) = &owner.body
            && let Some(variant) = s.variants.iter().find(|v| v.name.name == id.name)
        {
            if !variant.payload.is_empty() {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.variant_missing_payload",
                        id.span,
                        format!(
                            "variant `{}` of `{}` has a payload — call it with arguments: `{}(...)`",
                            id.name, owner.name.name, id.name
                        ),
                    )
                    .with_label(variant.span, "variant declared here"),
                );
                return None;
            }
            return nullary_variant_ty(owner, id.name.as_str(), id.span, expected, ctx);
        }
    }
    // Nothing owns the name. The resolver's reference walk reports these in
    // `fn`/method bodies, but handler/service/agent bodies and test bodies
    // never pass through that walk — the checker is their only backstop, and
    // a silent `None` here admitted any unknown name and emitted it verbatim.
    // Mirror the resolver's ladder.
    //
    // #1708: test bodies were exempt, because a loosely typed service-call
    // result (`let r <- svc.call(…)`, v0.25) left `r` unbound and its uses
    // misfired as unknown. Since #1663 such a binding is bound error-typed and
    // its uses absorb, so the exemption only hid real unknown names — which
    // then reached test-module emission as `Ty::Error` and panicked.
    if owners.len() > 1 {
        ctx.errors.push(CompileError::new(
            "bynk.resolve.ambiguous_variant",
            id.span,
            format!(
                "the variant name `{}` is declared on multiple sum types — qualify it as `TypeName.{}`",
                id.name, id.name
            ),
        ));
        return None;
    }
    // #1925: the local declaration of another kind (a type, event, service,
    // agent or provider) is no value here, and it hides the import.
    if let Some(err) = ctx.input.shadowed_import_error(&id.name, id.span) {
        ctx.errors.push(err);
        return None;
    }
    if ctx.input.types.contains_key(&id.name) {
        ctx.errors.push(
            CompileError::new(
                "bynk.resolve.type_in_expr",
                id.span,
                format!("`{}` is a type, not a value", id.name),
            )
            .with_note(
                "types cannot appear in expression position; \
                 use `TypeName.of(value)` or `TypeName { ... }` to construct values",
            ),
        );
        return None;
    }
    ctx.errors.push(
        CompileError::new(
            "bynk.resolve.unknown_name",
            id.span,
            format!("unknown name `{}`", id.name),
        )
        .with_note(
            "only parameters, `let` bindings, and functions declared \
             in this commons are in scope",
        ),
    );
    None
}

/// v0.9.4 Part B (slice 1): `Val[T]` / `Val[T](literal)` for refined types,
/// valid only in test bodies. Sum/record/opaque types are not yet supported.
pub(crate) fn check_val(
    type_ref: &TypeRef,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if !ctx.in_test_body {
        ctx.errors.push(
            CompileError::new(
                "bynk.val.outside_test",
                span,
                "`Val[T]` is only valid inside a test case body",
            )
            .with_note(
                "fabricated values are test-time construction; use them only inside `case \"...\" { ... }` blocks",
            ),
        );
    }
    let ty = match resolve_type_ref(type_ref, &ctx.input.types, tys) {
        Some(t) => {
            // v0.25: `Val[T]` names the type.
            record_type_refs(type_ref, &ctx.input.types, &HashSet::new(), ctx.refs);
            t
        }
        None => {
            ctx.errors.push(CompileError::new(
                "bynk.val.unknown_type",
                span,
                "`Val[T]` refers to a type that does not resolve",
            ));
            return None;
        }
    };
    // v0.157 (ADR 0183): fabricating a value of a generic type would need
    // per-instantiation field generation, which this increment does not wire.
    // Keyed off the *declaration* (not the resolved `args`), so a bare
    // `Val[Wrapper]` on a generic type is caught as well as `Val[Wrapper[T]]`
    // — the resolver does not walk a `Val` type ref, so it never fires its own
    // arity check for the bare form.
    if let Ty::Named { name, .. } = &*tys.get(ty)
        && ctx
            .input
            .types
            .get(name)
            .is_some_and(|d| !d.type_params.is_empty())
    {
        ctx.errors.push(
            CompileError::new(
                "bynk.generics.generic_record_at_boundary",
                span,
                format!("`Val[{name}[…]]` cannot fabricate a value of a generic type in v0.157"),
            )
            .with_note("construct the value explicitly with a record literal instead"),
        );
        return None;
    }
    match &*tys.get(ty) {
        // Refined types: bare (generate a default) or a single literal pin.
        Ty::Named {
            name,
            kind: NamedKind::Refined(base),
            ..
        } => {
            let name = name.clone();
            let base = *base;
            let decl = match ctx.input.types.get(&name) {
                Some(d) => d.clone(),
                // Unreachable: the type already resolved above.
                None => return None,
            };
            let refinement = type_decl_refinement(&decl);
            match args {
                [] => {
                    if refinement.is_some_and(refinement_needs_pin) {
                        ctx.errors.push(
                            CompileError::new(
                                "bynk.val.needs_pin",
                                span,
                                format!(
                                    "bare `Val[{name}]` cannot generate a value for a `Matches` refinement"
                                ),
                            )
                            .with_note("provide an explicit value, e.g. `Val[T](\"...\")`"),
                        );
                    }
                }
                [arg] => {
                    type_of(arg, Some(tys.intern(Ty::Base(base))), ctx);
                    match const_literal(arg) {
                        Some(lit) if literal_matches_base(&lit, base) => {
                            if let Some(r) = refinement
                                && let Some(failed) = first_failed_predicate(r, &lit)
                            {
                                ctx.errors.push(CompileError::new(
                                    "bynk.val.literal_violates",
                                    arg.span,
                                    format!(
                                        "literal {} does not satisfy `{}` required by type `{}`",
                                        lit.display(),
                                        failed.name(),
                                        name
                                    ),
                                ));
                            }
                        }
                        _ => {
                            ctx.errors.push(CompileError::new(
                                "bynk.val.pin_not_literal",
                                arg.span,
                                format!(
                                    "`Val[{name}](...)` requires a literal `{}` value",
                                    base.name()
                                ),
                            ));
                        }
                    }
                }
                _ => {
                    ctx.errors.push(CompileError::new(
                        "bynk.val.arity",
                        span,
                        format!(
                            "`Val[{name}]` takes at most one pin argument, but {} were given",
                            args.len()
                        ),
                    ));
                }
            }
        }
        // v0.9.4 slice 2: opaque / sum / record — bare generation only. Pins for
        // these kinds (variant pins, record overrides) are a later increment.
        Ty::Named {
            name,
            kind: NamedKind::Opaque(_) | NamedKind::Sum | NamedKind::Record,
            ..
        } => {
            let name = name.clone();
            if !args.is_empty() {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.val.pin_unsupported",
                        span,
                        format!(
                            "pinned `Val[{name}](...)` is not yet supported for this kind of type — use bare `Val[{name}]`"
                        ),
                    )
                    .with_note("literal pins are currently supported for refined types only"),
                );
            } else if !can_mock_bare(ty, &ctx.input.types, MOCK_DEPTH, tys) {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.val.needs_pin",
                        span,
                        format!(
                            "bare `Val[{name}]` cannot generate a value — it (transitively) needs a `Matches` refinement or is recursively unbounded"
                        ),
                    )
                    .with_note("provide an explicit value in the test instead"),
                );
            }
        }
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.val.unsupported_kind",
                span,
                format!("`Val` is not a value type: `{}`", ty.display(tys)),
            ));
        }
    }
    Some(ty)
}

/// v0.9.4 slice 2 recursion depth cap for bare `Val` generation — guards
/// against recursively-unbounded types (a sum whose first variant re-enters the
/// type). Beyond it, bare generation is refused.
const MOCK_DEPTH: u32 = 12;

/// Whether a bare `Val[T]` can generate a value for `ty`: refined types must
/// not carry a `Matches` predicate (no default), and sums/records must have
/// every (first-variant / field) component recursively mockable within the
/// depth cap.
fn can_mock_bare(
    ty: TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    depth: u32,
    tys: &Types,
) -> bool {
    if depth == 0 {
        return false;
    }
    match &*tys.get(ty) {
        Ty::Base(_) => true,
        Ty::Named { name, .. } => {
            let Some(decl) = types.get(name.as_str()) else {
                return false;
            };
            match &decl.body {
                TypeBody::Refined { refinement, .. } => {
                    !refinement.as_ref().is_some_and(refinement_needs_pin)
                }
                TypeBody::Opaque { .. } => true,
                TypeBody::Sum(s) => s.variants.first().is_some_and(|v| {
                    v.payload.iter().all(|f| {
                        resolve_type_ref(&f.type_ref, types, tys)
                            .is_some_and(|t| can_mock_bare(t, types, depth - 1, tys))
                    })
                }),
                TypeBody::Record(r) => r.fields.iter().all(|f| {
                    resolve_type_ref(&f.type_ref, types, tys)
                        .is_some_and(|t| can_mock_bare(t, types, depth - 1, tys))
                }),
            }
        }
        _ => false,
    }
}

pub(crate) fn check_expect(inner: &Expr, span: Span, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    if !ctx.in_test_body {
        ctx.errors.push(
            CompileError::new(
                "bynk.expect.outside_case",
                span,
                "`expect` is only valid inside a `case` body",
            )
            .with_note(
                "expectations verify predicates at test runtime; use them only inside `case \"...\" { ... }` blocks",
            ),
        );
    }
    let val_ty = type_of(inner, Some(tys.intern(Ty::Base(BaseType::Bool))), ctx);
    if let Some(actual) = val_ty
        && !compatible(actual, tys.intern(Ty::Base(BaseType::Bool)), tys)
    {
        ctx.errors.push(CompileError::new(
            "bynk.expect.not_bool",
            inner.span,
            format!(
                "`expect` predicate has type `{}`, but a `Bool` is required",
                actual.display(tys),
            ),
        ));
    }
    Some(tys.intern(Ty::Unit))
}

/// Resolve an observation seam `Cap.op` (v0.117) against the capabilities the
/// unit under test consumes. Returns the operation's signature on success; on
/// failure it pushes `bynk.observe.not_a_seam` / `bynk.observe.unknown_op` and
/// returns `None`.
fn resolve_observation_seam(cap: &Ident, op: &Ident, ctx: &mut Ctx) -> Option<CapabilityOpInfo> {
    let Some(cap_info) = ctx.caps.capabilities.get(&cap.name).cloned() else {
        ctx.errors.push(
            CompileError::new(
                "bynk.observe.not_a_seam",
                cap.span,
                format!(
                    "`{}` is not a capability the unit under test consumes; only a consumed \
                     capability's calls can be observed",
                    cap.name
                ),
            )
            .with_note("observe a capability the target `consumes` / has in scope via `given`"),
        );
        return None;
    };
    let Some(op_info) = cap_info.ops.iter().find(|o| o.name == op.name).cloned() else {
        ctx.errors.push(CompileError::new(
            "bynk.observe.unknown_op",
            op.span,
            format!(
                "capability `{}` has no operation named `{}`",
                cap.name, op.name
            ),
        ));
        return None;
    };
    ctx.refs.record(
        op.span,
        SymbolKind::CapabilityOp,
        &format!("{}.{}", cap.name, op.name),
    );
    Some(op_info)
}

/// Type-check an observation (v0.117, testing track slice 5). The subject
/// `Cap.op` must be a consumed capability operation; `with <pred>` is the pure
/// invariant predicate over the operation's parameters (in scope by name); a
/// count must be a non-negative literal; `before Cap.op` resolves a second seam.
/// The observation itself is a `Bool` claim about the recorded trace.
pub(crate) fn check_observation(o: &ObservationExpr, _span: Span, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    // An observation parses only as the subject of an `expect`, and an
    // `expect` outside a test body has already been reported
    // (`bynk.expect.outside_case`) — but the walk continues into the subject,
    // so stop here rather than resolve a recording seam that only a test body
    // has (#1662 review).
    if !ctx.in_test_body {
        return None;
    }
    let op_info = resolve_observation_seam(&o.cap, &o.op, ctx);
    match &o.matcher {
        ObservationMatcher::Called { count, with_pred } => {
            // The parser accepts only `once` or a bare integer-literal token as a
            // count, and a literal token is never negative.
            debug_assert!(
                count.as_ref().is_none_or(
                    |c| matches!(&c.kind, ExprKind::IntLit { value: n, .. } if *n >= 0)
                ),
                "an observation count is always a non-negative literal"
            );
            if let Some(p) = with_pred {
                if let Some(impure) = predicate_impure_construct(p) {
                    ctx.errors.push(
                        CompileError::new(
                            "bynk.observe.impure_with",
                            impure,
                            "a `with` predicate uses an effectful or test-only construct; it must be pure",
                        )
                        .with_note(
                            "a `with` predicate may read the operation's arguments and call pure value methods only",
                        ),
                    );
                }
                // Scope the predicate over the operation's parameters by name.
                let mut scope: HashMap<String, TyId> = HashMap::new();
                if let Some(info) = &op_info {
                    for (name, ty) in info.param_names.iter().zip(info.params.iter()) {
                        scope.insert(name.clone(), *ty);
                    }
                }
                ctx.push_scope_with(scope);
                let pred_ty = type_of(p, Some(tys.intern(Ty::Base(BaseType::Bool))), ctx);
                ctx.pop_scope();
                if let Some(t) = pred_ty
                    && !compatible(t, tys.intern(Ty::Base(BaseType::Bool)), tys)
                {
                    ctx.errors.push(CompileError::new(
                        "bynk.observe.with_not_bool",
                        p.span,
                        format!(
                            "a `with` predicate has type `{}`, but a `Bool` is required",
                            t.display(tys)
                        ),
                    ));
                }
            }
        }
        ObservationMatcher::NeverCalled => {}
        ObservationMatcher::Before { cap, op } => {
            let _ = resolve_observation_seam(cap, op, ctx);
        }
    }
    Some(tys.intern(Ty::Base(BaseType::Bool)))
}

/// Type-check a fault claim, `expect <call> faults` (#1706). The subject must
/// be an `Effect[_]` — the call the claim awaits — and the claim itself is a
/// `Bool`: whether awaiting it threw. Like the observation surface it parses
/// only as an `expect`'s subject, so outside a test body
/// `bynk.expect.outside_case` has already been reported and the subject is not
/// re-diagnosed here.
///
/// Awaiting is an effect, so the claim needs an effectful body, as `<-` does
/// (`bynk.effect.bind_in_pure_context`). The claim's call-site principal
/// (#1812, `expect <call> by User("bob") faults`) is validated against the
/// addressed handler exactly as an effect-let's is
/// (`check_effect_let_principal`) — an absent `by` on an identity-carrying
/// handler included, rather than silently driven with no identity.
///
/// `by Nobody` is the exception: it drives the `system`-tier auth seam, where
/// a fault claim cannot hold, so the tier pass reports it as
/// `bynk.test.faults_needs_in_process` and it is not validated here — the
/// route-shape diagnostics would only bury that one.
pub(crate) fn check_faults(faults: &FaultsExpr, span: Span, ctx: &mut Ctx) -> Option<TyId> {
    let FaultsExpr { call, principal } = faults;
    let tys = ctx.tys;
    let bool_ty = tys.intern(Ty::Base(BaseType::Bool));
    if !ctx.in_test_body {
        return Some(bool_ty);
    }
    if !ctx.effectful {
        ctx.errors.push(
            CompileError::new(
                "bynk.effect.bind_in_pure_context",
                span,
                "`faults` awaits its call, so it can only be used inside an effectful body",
            )
            .with_note("a fault claim belongs in a `case` body, not inside a pure lambda"),
        );
    }
    let call_ty = type_of(call, None, ctx);
    match principal {
        Some(p) if p.actor.name == "Nobody" => {
            if let Some(identity) = &p.identity {
                let _ = type_of(identity, None, ctx);
                // #1878 review: the route-shape checks are skipped (the tier
                // error is the one that matters), but `Nobody` taking no
                // identity is said now, not after the rewrite the tier error
                // suggests.
                ctx.errors.push(CompileError::new(
                    "bynk.test.actor_no_identity",
                    p.span,
                    "actor `Nobody` has no identity, so write `by Nobody` with no argument",
                ));
            }
        }
        _ => super::calls::check_effect_let_principal(call, principal.as_ref(), ctx),
    }
    let note = "only an effectful call can fault; give `faults` the call itself (`expect svc.call(x) faults`), not a value already bound from it";
    // The subject must be a call *syntactically* as well as an `Effect` by
    // type: a case-body binding from an address call is deliberately untyped
    // (`let r <- svc.call()`), so the type test alone would let
    // `expect r faults` through to await a settled value that never throws.
    if !matches!(
        call.kind,
        ExprKind::Call { .. } | ExprKind::MethodCall { .. }
    ) {
        ctx.errors.push(
            CompileError::new(
                "bynk.expect.faults_not_effect",
                call.span,
                "`faults` claims that an effectful call throws, but this is not a call",
            )
            .with_note(note),
        );
        return Some(bool_ty);
    }
    match call_ty.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Effect(_)) | None => {}
        Some(other) => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.expect.faults_not_effect",
                    call.span,
                    format!(
                        "`faults` claims that an effectful call throws, but this call has type `{}`, not `Effect[_]`",
                        other.display(tys)
                    ),
                )
                .with_note(note),
            );
        }
    }
    Some(bool_ty)
}

/// Type-check `trace(Cap.op)` (v0.117). Resolves the seam and yields
/// `List[<CallRecord>]`, where `<CallRecord>` is the synthetic per-operation
/// record (registered in the test-body type table) whose fields are the
/// operation's parameters.
pub(crate) fn check_trace(cap: &Ident, op: &Ident, span: Span, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    if !ctx.in_test_body {
        ctx.errors.push(
            CompileError::new(
                "bynk.observe.trace_outside_test",
                span,
                "`trace` is only valid inside a `case` body",
            )
            .with_note("`trace(Cap.op)` reads the calls recorded during a `case`"),
        );
    }
    resolve_observation_seam(cap, op, ctx)?;
    Some(tys.intern(Ty::List(tys.intern(Ty::Named {
        name: call_record_type_name(&cap.name, &op.name),
        kind: NamedKind::Record,
        args: Vec::new(),
    }))))
}

pub(crate) fn check_unary(op: UnaryOp, inner: &Expr, op_span: Span, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    let t = type_of(inner, None, ctx)?;
    match op {
        UnaryOp::Neg => {
            if t.base(tys) == Some(BaseType::Int) {
                Some(tys.intern(Ty::Base(BaseType::Int)))
            } else {
                ctx.errors.push(CompileError::new(
                    "bynk.types.type_mismatch",
                    op_span,
                    format!(
                        "unary `-` requires `Int`, but the operand has type `{}`",
                        t.display(tys)
                    ),
                ));
                None
            }
        }
        UnaryOp::Not => {
            if t.base(tys) == Some(BaseType::Bool) {
                Some(tys.intern(Ty::Base(BaseType::Bool)))
            } else {
                ctx.errors.push(CompileError::new(
                    "bynk.types.type_mismatch",
                    op_span,
                    format!(
                        "unary `!` requires `Bool`, but the operand has type `{}`",
                        t.display(tys)
                    ),
                ));
                None
            }
        }
    }
}

/// v0.21: whether one operand is `Int` and the other `Float` — the mix the
/// no-coercion rule (ADR 0041) rejects with its own diagnostic.
pub(crate) fn numeric_mix(a: Option<BaseType>, b: Option<BaseType>) -> bool {
    matches!(
        (a, b),
        (Some(BaseType::Int), Some(BaseType::Float)) | (Some(BaseType::Float), Some(BaseType::Int))
    )
}

/// v0.86 (ADR 0112): a `Duration` operand misused. Shares the
/// `no_numeric_coercion` code (D4) but with a `Duration`-specific message,
/// since the `.toFloat()` advice is wrong here.
fn push_duration_op_error(op: BinOp, span: Span, lt: TyId, rt: TyId, ctx: &mut Ctx) {
    let tys = ctx.tys;
    ctx.errors.push(
        CompileError::new(
            "bynk.types.no_numeric_coercion",
            span,
            format!(
                "operator `{}` is not defined for operands `{}` and `{}`",
                op.name(),
                lt.display(tys),
                rt.display(tys)
            ),
        )
        .with_note(
            "`Duration` supports `+`/`-` with another `Duration`, `*` by an `Int`, \
             comparison, and instant math with an `Instant` (`now + 5.minutes`); use \
             `.toMillis()` to compute in raw milliseconds",
        ),
    );
}

/// v0.90 (ADR 0114): an `Instant` operand misused. Shares the
/// `no_numeric_coercion` code with an `Instant`-specific message.
fn push_instant_op_error(op: BinOp, span: Span, lt: TyId, rt: TyId, ctx: &mut Ctx) {
    let tys = ctx.tys;
    ctx.errors.push(
        CompileError::new(
            "bynk.types.no_numeric_coercion",
            span,
            format!(
                "operator `{}` is not defined for operands `{}` and `{}`",
                op.name(),
                lt.display(tys),
                rt.display(tys)
            ),
        )
        .with_note(
            "`Instant` supports `+`/`-` with a `Duration` (yielding an `Instant`), \
             `Instant - Instant` (yielding a `Duration`), and comparison; it has no \
             arithmetic with `Int` — use `.toEpochMillis()` for a raw millis count",
        ),
    );
}

fn push_no_numeric_coercion(op: BinOp, span: Span, lt: TyId, rt: TyId, ctx: &mut Ctx) {
    let tys = ctx.tys;
    ctx.errors.push(
        CompileError::new(
            "bynk.types.no_numeric_coercion",
            span,
            format!(
                "operator `{}` cannot mix `Int` and `Float` operands; got `{}` and `{}`",
                op.name(),
                lt.display(tys),
                rt.display(tys)
            ),
        )
        .with_note(
            "there is no implicit numeric coercion — convert explicitly with \
             `.toFloat()` on the `Int`, or `.round()`/`.floor()`/`.ceil()`/`.truncate()` \
             on the `Float`",
        ),
    );
}

pub(crate) fn check_binop(op: BinOp, lhs: &Expr, rhs: &Expr, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    // For `&&` (and v0.80 `implies`), if the lhs is or contains an `is` test,
    // propagate the bindings into the rhs scope (so `r is Ok(n) && n > 0`, and
    // `r is Ok(n) implies n > 0`, both work). `implies` is `!P || Q`, so the rhs
    // is only reached when the lhs holds — the same binding scope as `&&`.
    if matches!(op, BinOp::And | BinOp::Implies) {
        let lt = type_of(lhs, Some(tys.intern(Ty::Base(BaseType::Bool))), ctx);
        // Must run *after* `type_of(lhs)`: `collect_is_bindings` reads (and
        // memoises by span) the lhs's `expr_types`, so type-checking has to
        // have populated them first — collecting earlier would cache empties.
        let bindings = collect_is_bindings(lhs, ctx);
        ctx.push_scope();
        for (name, ty) in bindings {
            ctx.bind(name, ty);
        }
        let rt = type_of(rhs, Some(tys.intern(Ty::Base(BaseType::Bool))), ctx);
        ctx.pop_scope();
        let (lt, rt) = (lt?, rt?);
        if lt.base(tys) != Some(BaseType::Bool) {
            ctx.errors.push(CompileError::new(
                "bynk.types.type_mismatch",
                lhs.span,
                format!(
                    "operator `{}` requires `Bool` operands; left operand has type `{}`",
                    op.name(),
                    lt.display(tys)
                ),
            ));
            return None;
        }
        if rt.base(tys) != Some(BaseType::Bool) {
            ctx.errors.push(CompileError::new(
                "bynk.types.type_mismatch",
                rhs.span,
                format!(
                    "operator `{}` requires `Bool` operands; right operand has type `{}`",
                    op.name(),
                    rt.display(tys)
                ),
            ));
            return None;
        }
        return Some(tys.intern(Ty::Base(BaseType::Bool)));
    }

    // #1659 (Decision A): `==`/`!=` operands are same-typed (§5.2), so one
    // side's type is the expected type for the other. That lets a bare
    // `None`/`Ok(…)`/`Err(…)`/`[]` take its type from the other operand
    // (`o == None`, `None == o`, `r == Ok(7)`) instead of failing to infer, or
    // reading an unrelated surrounding type. Type the determined side first.
    //
    // Only an operand that cannot type itself is given the other's type. A
    // self-typing operand is typed on its own, as before: passing a literal
    // the other side's refined type would run refinement admission on it
    // (`q == 0` against `Int where Positive` would become a
    // `literal_violates` error), a new rejection this does not intend (review
    // of #1696).
    let eq = matches!(op, BinOp::Eq | BinOp::NotEq);
    let (lt, rt) = if eq && needs_expected_type(lhs) && !needs_expected_type(rhs) {
        let rt = type_of(rhs, None, ctx);
        let lt = type_of(lhs, rt, ctx);
        (lt, rt)
    } else if eq && needs_expected_type(rhs) {
        let lt = type_of(lhs, None, ctx);
        let rt = type_of(rhs, lt, ctx);
        (lt, rt)
    } else {
        (type_of(lhs, None, ctx), type_of(rhs, None, ctx))
    };
    let (lt, rt) = (lt?, rt?);
    let span = lhs.span.merge(rhs.span);
    let lt_base = lt.base(tys);
    let rt_base = rt.base(tys);
    match op {
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div => {
            // v0.90 (ADR 0114): `Instant` arithmetic. Handled first so an
            // `Instant ± Duration` (either order) routes here, not to the
            // `Duration` block. D3: advance/retreat an instant by a span; the
            // span between two instants.
            use BaseType::{Duration, Instant, Int};
            if lt_base == Some(Instant) || rt_base == Some(Instant) {
                return match (op, lt_base, rt_base) {
                    (BinOp::Add, Some(Instant), Some(Duration))
                    | (BinOp::Add, Some(Duration), Some(Instant))
                    | (BinOp::Sub, Some(Instant), Some(Duration)) => {
                        Some(tys.intern(Ty::Base(Instant)))
                    }
                    (BinOp::Sub, Some(Instant), Some(Instant)) => {
                        Some(tys.intern(Ty::Base(Duration)))
                    }
                    // Every other `Instant` combination is rejected (e.g.
                    // `Instant + Instant`, `Instant * Int`, `Instant ± Int`).
                    _ => {
                        push_instant_op_error(op, span, lt, rt, ctx);
                        None
                    }
                };
            }
            // v0.86 (ADR 0112): `Duration` arithmetic. Handled before the
            // Int/Float rules so the `Duration`-closed ops (D3) are explicit;
            // anything else involving a `Duration` is a coercion error. The
            // former `Int ± Duration` clock-math mix (0112 D4) is **withdrawn**
            // (ADR 0114 D5): timestamp math now goes through `Instant`.
            if lt_base == Some(Duration) || rt_base == Some(Duration) {
                return match (op, lt_base, rt_base) {
                    // D3: span ± span; span scaled by an integer scalar.
                    (BinOp::Add | BinOp::Sub, Some(Duration), Some(Duration)) => {
                        Some(tys.intern(Ty::Base(Duration)))
                    }
                    (BinOp::Mul, Some(Duration), Some(Int))
                    | (BinOp::Mul, Some(Int), Some(Duration)) => {
                        Some(tys.intern(Ty::Base(Duration)))
                    }
                    // Every other `Duration` combination is rejected (e.g.
                    // `Duration + Int`, `Int + Duration`, `Duration * Duration`).
                    _ => {
                        push_duration_op_error(op, span, lt, rt, ctx);
                        None
                    }
                };
            }
            // v0.21: arithmetic is defined on `Int` and `Float`, never mixed
            // — there is no implicit numeric coercion (ADR 0041).
            match (lt_base, rt_base) {
                (Some(BaseType::Int), Some(BaseType::Int)) => {
                    Some(tys.intern(Ty::Base(BaseType::Int)))
                }
                (Some(BaseType::Float), Some(BaseType::Float)) => {
                    Some(tys.intern(Ty::Base(BaseType::Float)))
                }
                (Some(BaseType::Int), Some(BaseType::Float))
                | (Some(BaseType::Float), Some(BaseType::Int)) => {
                    push_no_numeric_coercion(op, span, lt, rt, ctx);
                    None
                }
                _ => {
                    let (side, side_span, ty) =
                        if !matches!(lt_base, Some(BaseType::Int) | Some(BaseType::Float)) {
                            ("left", lhs.span, &lt)
                        } else {
                            ("right", rhs.span, &rt)
                        };
                    ctx.errors.push(CompileError::new(
                        "bynk.types.type_mismatch",
                        side_span,
                        format!(
                            "operator `{}` requires `Int` or `Float` operands; {side} operand has type `{}`",
                            op.name(),
                            ty.display(tys)
                        ),
                    ));
                    None
                }
            }
        }
        BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq => {
            if lt_base != rt_base || lt_base.is_none() {
                if numeric_mix(lt_base, rt_base) {
                    push_no_numeric_coercion(op, span, lt, rt, ctx);
                    return None;
                }
                ctx.errors.push(CompileError::new(
                    "bynk.types.type_mismatch",
                    span,
                    format!(
                        "operator `{}` requires both operands to have the same base type; got `{}` and `{}`",
                        op.name(),
                        lt.display(tys),
                        rt.display(tys)
                    ),
                ));
                return None;
            }
            if !matches!(
                lt_base,
                Some(BaseType::Int)
                    | Some(BaseType::String)
                    | Some(BaseType::Float)
                    | Some(BaseType::Duration)
                    | Some(BaseType::Instant)
            ) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.type_mismatch",
                    span,
                    format!(
                        "operator `{}` is only defined on `Int`, `Float`, `Duration`, `Instant`, and `String`, not `{}`",
                        op.name(),
                        lt.display(tys)
                    ),
                ));
                return None;
            }
            Some(tys.intern(Ty::Base(BaseType::Bool)))
        }
        BinOp::Eq | BinOp::NotEq => {
            // #1652 (runtime-semantics track §3.1): `==` is structural, so it is
            // only defined on equality-supporting types, recursively
            // (type-system §2.3.5). A `Stream` (v0.100), a held `Connection`
            // (v0.102, §2.9.3), or a function/`Effect`/`Query` anywhere inside
            // the operand's type makes it non-comparable. Before #1652 only the
            // top-level type was checked, so `Option[Connection[F]]` and
            // `List[(Int) -> Int]` slipped through to a reference `===`.
            // #1688: inside a generic body, note which type variables are
            // compared, so a call site can check what they are instantiated
            // with.
            super::equality::record_compared(lt, tys);
            super::equality::record_compared(rt, tys);
            for operand in [lt, rt] {
                let Some(blocker) = super::equality::not_comparable(operand, ctx) else {
                    continue;
                };
                let shown = tys.display(operand);
                // The operand may itself be the offending part (`s == s` on a
                // `Stream`), not merely contain it.
                let top = matches!(
                    &*tys.get(operand),
                    Ty::Stream(_)
                        | Ty::Connection(_)
                        | Ty::Fn { .. }
                        | Ty::Effect(_)
                        | Ty::Query(_)
                );
                let it = if top { "it is" } else { "it contains" };
                let (code, message) = match blocker {
                    super::equality::NotComparable::Stream => (
                        "bynk.types.stream_not_comparable",
                        format!(
                            "operator `{}` cannot compare `{shown}` — {it} a `Stream`, a live value-over-time source, not a comparable value",
                            op.name()
                        ),
                    ),
                    super::equality::NotComparable::Held(held) => (
                        "bynk.types.held_not_comparable",
                        format!(
                            "operator `{}` cannot compare `{shown}` — {it} a held `{held}`, which has identity, not value-equality",
                            op.name()
                        ),
                    ),
                    super::equality::NotComparable::Computation(part) => (
                        "bynk.types.not_comparable",
                        if top {
                            format!(
                                "operator `{}` cannot compare `{shown}` — a function, `Effect` or `Query` is a computation, which has no value equality",
                                op.name()
                            )
                        } else {
                            format!(
                                "operator `{}` cannot compare `{shown}` — it contains `{part}`, which has no value equality",
                                op.name()
                            )
                        },
                    ),
                };
                ctx.errors.push(CompileError::new(code, span, message));
                return None;
            }
            if lt_base.is_some() && rt_base.is_some() {
                if lt_base != rt_base {
                    if numeric_mix(lt_base, rt_base) {
                        push_no_numeric_coercion(op, span, lt, rt, ctx);
                        return None;
                    }
                    ctx.errors.push(CompileError::new(
                        "bynk.types.type_mismatch",
                        span,
                        format!(
                            "operator `{}` requires both operands to have the same base type; got `{}` and `{}`",
                            op.name(),
                            lt.display(tys),
                            rt.display(tys)
                        ),
                    ));
                    return None;
                }
            } else if !compatible(lt, rt, tys) && !compatible(rt, lt, tys) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.type_mismatch",
                    span,
                    format!(
                        "operator `{}` requires both operands to have the same type; got `{}` and `{}`",
                        op.name(),
                        lt.display(tys),
                        rt.display(tys)
                    ),
                ));
                return None;
            }
            Some(tys.intern(Ty::Base(BaseType::Bool)))
        }
        // `And` and `Implies` are handled in the early-return block above (for
        // is-binding propagation); listed here only for match exhaustiveness.
        BinOp::And | BinOp::Or | BinOp::Implies => {
            if lt.base(tys) != Some(BaseType::Bool) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.type_mismatch",
                    lhs.span,
                    format!(
                        "operator `{}` requires `Bool` operands; left operand has type `{}`",
                        op.name(),
                        lt.display(tys)
                    ),
                ));
                return None;
            }
            if rt.base(tys) != Some(BaseType::Bool) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.type_mismatch",
                    rhs.span,
                    format!(
                        "operator `{}` requires `Bool` operands; right operand has type `{}`",
                        op.name(),
                        rt.display(tys)
                    ),
                ));
                return None;
            }
            Some(tys.intern(Ty::Base(BaseType::Bool)))
        }
    }
}

/// v0.20a: type-check the application of a function-typed value (`f(x)`
/// where `f` is a param or local of type `A -> B`). Reuses the ordinary
/// argument rules; an effectful result (`ret` is `Effect[_]`) is an effect
/// operation, legal only in an effectful context — the same confinement a
/// capability call obeys.
/// v0.20a: type-check a lambda (`(params) => body`). Two paths:
///
/// - **Expected function type** (ground — guaranteed by the generic
///   instantiation order): params type contextually (an annotation must be
///   compatible with the expected param), the body checks against the
///   expected return with `effectful` derived from it, and the result is the
///   expected type (checking-mode bidirectionality).
/// - **Unconstrained**: every param must be annotated
///   (`bynk.lambda.unannotated_param`); effectfulness is decided by a
///   syntactic pre-scan of the body (`<-`, capability calls, effectful named
///   or value calls), and the result type wraps in `Effect` when it fired.
///
/// The enclosing handler's capability map and `given` tracking stay shared —
/// a lambda may close over and call a `given` capability (ADR 0033). The
/// frame swap forbids `commit` inside a lambda (`agent_state_ty = None` →
/// the existing `bynk.commit.outside_agent`).
pub(crate) fn check_lambda(
    lambda: &LambdaExpr,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let expected_fn = match expected.map(|e| tys.get(e)).as_deref() {
        Some(Ty::Fn { params, ret }) => Some((params.clone(), *ret)),
        _ => None,
    };

    // Establish param types.
    let mut param_tys: Vec<TyId> = Vec::new();
    let mut scope: HashMap<String, TyId> = HashMap::new();
    if let Some((eps, _)) = &expected_fn {
        if eps.len() != lambda.params.len() {
            ctx.errors.push(CompileError::new(
                "bynk.types.lambda_mismatch",
                lambda.span,
                format!(
                    "this lambda takes {} parameter(s), but a function of {} parameter(s) is expected",
                    lambda.params.len(),
                    eps.len()
                ),
            ));
            return None;
        }
        for (p, ep) in lambda.params.iter().zip(eps) {
            let ty = match &p.type_ref {
                Some(tr) => {
                    // #712: report an unknown annotation type instead of the
                    // bare `?` silently dropping the lambda in a handler body.
                    let annotated = resolve_expr_type_ref(tr, ctx)?;
                    if !compatible(*ep, annotated, tys) {
                        ctx.errors.push(CompileError::new(
                            "bynk.types.lambda_mismatch",
                            p.span,
                            format!(
                                "lambda parameter `{}` is annotated `{}`, but `{}` is expected here",
                                p.name.name,
                                annotated.display(tys),
                                ep.display(tys)
                            ),
                        ));
                    }
                    annotated
                }
                None => {
                    // v0.27 (ADR 0056): a param typed from the expected fn
                    // type gets an inferred-type inlay hint at its name.
                    if p.name.name != "_" {
                        ctx.hints
                            .record(p.name.span, format!(": {}", ep.display(tys)));
                    }
                    *ep
                }
            };
            scope.insert(p.name.name.clone(), ty);
            param_tys.push(ty);
        }
    } else {
        let mut missing = false;
        for p in &lambda.params {
            match &p.type_ref {
                Some(tr) => {
                    // #712: report an unknown annotation type instead of the
                    // bare `?` silently dropping the lambda in a handler body.
                    let ty = resolve_expr_type_ref(tr, ctx)?;
                    scope.insert(p.name.name.clone(), ty);
                    param_tys.push(ty);
                }
                None => {
                    ctx.errors.push(
                        CompileError::new(
                            "bynk.lambda.unannotated_param",
                            p.span,
                            format!(
                                "lambda parameter `{}` needs a type annotation — no function type is expected here to infer it from",
                                p.name.name
                            ),
                        )
                        .with_note("annotate the parameter (e.g. `(x: Int) => …`), or pass the lambda where a function type is expected"),
                    );
                    missing = true;
                }
            }
        }
        if missing {
            return None;
        }
    }

    // v0.31: lambda parameters are in scope over the lambda body.
    for (p, ty) in lambda.params.iter().zip(&param_tys) {
        if p.name.name != "_" {
            ctx.locals.record(
                p.name.name.clone(),
                p.name.span,
                crate::locals::LocalKind::Param,
                ty.display(tys),
                lambda.body.span,
            );
        }
    }

    ctx.push_scope_with(scope);

    // v0.20a generics: an expected return that still carries a *flexible*
    // type variable (pass 2 of inference — the lambda's result is what binds
    // it) is treated as unconstrained: the body types bottom-up and the
    // caller's unify captures the variable. v0.20b: the enclosing generic
    // fn's own type parameters are *rigid* — an expected return of
    // `Option[A]` inside `find[A]`'s body is fully constrained, and the
    // body's `None`/`[]`/`Map.empty()` may infer from it.
    let ret_constrained = expected_fn
        .as_ref()
        .is_some_and(|(_, er)| !contains_flexible_var(*er, &ctx.type_vars, tys));

    // Decide the body's effectfulness BEFORE typing it: the effect gates
    // (`bind_in_pure_context`, `capability_in_pure_context`, the fn-value
    // gate) fire during typing off `ctx.effectful`.
    let body_effectful = match &expected_fn {
        Some((_, er)) if ret_constrained => er.is_effect(tys),
        _ => body_performs_effects(&lambda.body, ctx),
    };

    // Frame swap (save/restore — the capability map and given-tracking stay
    // shared so closures over capabilities work and count as uses).
    let saved_effectful = ctx.effectful;
    let saved_return_ty = ctx.return_ty;
    let saved_return_ty_span = ctx.return_ty_span;
    let saved_agent_state_ty = ctx.agent_state_ty.take();
    let saved_commit_seen = ctx.commit_seen;
    ctx.effectful = body_effectful;
    ctx.return_ty = match &expected_fn {
        Some((_, er)) if ret_constrained => *er,
        // Placeholder: no diagnostic path can consult it — the pre-scan sets
        // `effectful` whenever a `<-` exists, so `bind_in_pure_context`'s
        // return-type label is unreachable here.
        _ => tys.intern(Ty::Unit),
    };
    ctx.return_ty_span = lambda.span;
    ctx.commit_seen = false;

    let body_expected = match &expected_fn {
        Some((_, er)) if ret_constrained => Some(*er),
        _ => None,
    };
    let body_ty = type_of(&lambda.body, body_expected, ctx);

    ctx.effectful = saved_effectful;
    ctx.return_ty = saved_return_ty;
    ctx.return_ty_span = saved_return_ty_span;
    ctx.agent_state_ty = saved_agent_state_ty;
    ctx.commit_seen = saved_commit_seen;
    ctx.pop_scope();

    match expected_fn {
        // Var-bearing expected return: report the actual function type and
        // let the caller's unify bind the variable.
        Some((eps, _)) if !ret_constrained => {
            let bt = body_ty?;
            let ret = if body_effectful && !bt.is_effect(tys) {
                tys.intern(Ty::Effect(bt))
            } else {
                bt
            };
            Some(tys.intern(Ty::Fn { params: eps, ret }))
        }
        Some((eps, er)) => {
            if let Some(bt) = body_ty.as_ref() {
                // A pure body against an effectful expected return auto-lifts
                // (the emitter's async arrow realises the lifted Promise).
                let lifted = maybe_auto_lift(Some(*bt), Some(er), tys).unwrap_or(*bt);
                if !compatible(lifted, er, tys) {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.lambda_mismatch",
                        lambda.body.span,
                        format!(
                            "lambda body has type `{}`, but `{}` is expected",
                            bt.display(tys),
                            er.display(tys)
                        ),
                    ));
                    return None;
                }
            }
            Some(tys.intern(Ty::Fn {
                params: eps,
                ret: er,
            }))
        }
        None => {
            let bt = body_ty?;
            let ret = if body_effectful && !bt.is_effect(tys) {
                tys.intern(Ty::Effect(bt))
            } else {
                bt
            };
            Some(tys.intern(Ty::Fn {
                params: param_tys,
                ret,
            }))
        }
    }
}

/// v0.20a: the syntactic pre-scan deciding a lambda's effectfulness in an
/// unconstrained position, run after the lambda's params are in scope and
/// before typing. True on: an `<-` bind; a capability static-call; a call on
/// a scope binding or named function whose type/signature returns `Effect`;
/// `Effect.pure`. Does **not** descend into nested lambdas — an inner
/// lambda's effects are its own.
fn body_performs_effects(e: &Expr, ctx: &Ctx) -> bool {
    fn block_performs(b: &Block, ctx: &Ctx) -> bool {
        for s in &b.statements {
            match s {
                Statement::EffectLet(_) => return true,
                Statement::Let(l) => {
                    if body_performs_effects(&l.value, ctx) {
                        return true;
                    }
                }
                Statement::Expect(a) => {
                    if body_performs_effects(&a.value, ctx) {
                        return true;
                    }
                }
                Statement::Send(_) => return true,
                Statement::Do(_) => return true,
                Statement::Assign(a) => {
                    if body_performs_effects(&a.value, ctx) {
                        return true;
                    }
                }
                // #2137: moving the harness clock is an effect on the case.
                Statement::Advance(_) => return true,
                Statement::Par(_) => return true,
            }
        }
        body_performs_effects(&b.tail, ctx)
    }
    match &e.kind {
        ExprKind::Lambda(_) => false,
        // Slice C: `Wire(<String>)` wraps a pure string literal — never effectful.
        ExprKind::Wire(inner) => body_performs_effects(inner, ctx),
        // v0.43: an interpolated string is effectful iff one of its holes is.
        ExprKind::InterpStr(parts) => parts
            .iter()
            .any(|part| matches!(part, InterpPart::Hole(hole) if body_performs_effects(hole, ctx))),
        ExprKind::Block(b) => block_performs(b, ctx),
        ExprKind::EffectPure(_) => true,
        // A capability operation call (`Cap.op(…)`) or `Effect.pure` shape.
        ExprKind::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            if let ExprKind::Ident(id) = &receiver.kind
                && ctx.caps.capabilities.contains_key(&id.name)
            {
                return true;
            }
            // v0.20b: the effectful kernel fold returns `Effect[Acc]`.
            // Detected by name (the pre-scan is syntactic); a false positive
            // only *permits* effect syntax — a pure body still types pure.
            // v0.146 (ADR 0170): `forEach` (List/Query) is likewise an effect
            // terminal returning `Effect[()]`; v0.147 (ADR 0171): so is the
            // parallel `parTraverse`; v0.148 (ADR 0172): so are the collect-all
            // `traverseAll`/`parTraverseAll`.
            if matches!(
                method.name.as_str(),
                FOLD_EFF
                    | FOR_EACH
                    | PAR_TRAVERSE
                    | TRAVERSE_ALL
                    | PAR_TRAVERSE_ALL
                    | TRAVERSE_TRY
                    | PAR_TRAVERSE_TRY
            ) {
                return true;
            }
            body_performs_effects(receiver, ctx)
                || args.iter().any(|a| body_performs_effects(a, ctx))
        }
        ExprKind::Call { name, args, .. } => {
            if let Some(f) = ctx.lookup(&name.name)
                && let Ty::Fn { ret, .. } = &*ctx.tys.get(f)
                && ret.is_effect(ctx.tys)
            {
                return true;
            }
            if let Some(f) = ctx.input.fns.get(&name.name)
                && matches!(f.return_type, TypeRef::Effect(..))
            {
                return true;
            }
            args.iter().any(|a| body_performs_effects(a, ctx))
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            body_performs_effects(cond, ctx)
                || block_performs(then_block, ctx)
                || block_performs(else_block, ctx)
        }
        // #1800: a guard runs like any arm body, so an effect in one (`if x > 0
        // { let t <- Clock.now(); true } …`) makes the lambda effectful too.
        ExprKind::Match { discriminant, arms } => {
            body_performs_effects(discriminant, ctx)
                || arms.iter().any(|a| {
                    a.guard
                        .as_ref()
                        .is_some_and(|g| body_performs_effects(g, ctx))
                        || match &a.body {
                            MatchBody::Expr(e) => body_performs_effects(e, ctx),
                            MatchBody::Block(b) => block_performs(b, ctx),
                        }
                })
        }
        ExprKind::BinOp(_, l, r) => body_performs_effects(l, ctx) || body_performs_effects(r, ctx),
        ExprKind::UnaryOp(_, i)
        | ExprKind::Paren(i)
        | ExprKind::Ok(i)
        | ExprKind::Err(i)
        | ExprKind::Some(i)
        | ExprKind::Question(i)
        | ExprKind::Expect(i) => body_performs_effects(i, ctx),
        // #1706: a fault claim awaits its call — always an effect.
        ExprKind::Faults(_) => true,
        ExprKind::RecordConstruction { fields, .. } => fields.iter().any(|f| {
            f.value
                .as_ref()
                .is_some_and(|v| body_performs_effects(v, ctx))
        }),
        ExprKind::RecordSpread {
            base, overrides, ..
        } => {
            body_performs_effects(base, ctx)
                || overrides.iter().any(|f| {
                    f.value
                        .as_ref()
                        .is_some_and(|v| body_performs_effects(v, ctx))
                })
        }
        ExprKind::FieldAccess { receiver, .. } => body_performs_effects(receiver, ctx),
        ExprKind::Is { value, .. } => body_performs_effects(value, ctx),
        ExprKind::Val { args, .. } => args.iter().any(|a| body_performs_effects(a, ctx)),
        ExprKind::ListLit(elems) => elems.iter().any(|e| body_performs_effects(e, ctx)),
        // v0.117: observations and `trace` read the recorded call log — the
        // recording rides the *real* capability calls elsewhere in the body; the
        // observation expression itself performs no effect.
        ExprKind::Observation(_) | ExprKind::Trace { .. } => false,
        ExprKind::Ident(_)
        | ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::DurationLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::None
        | ExprKind::UnitLit => false,
    }
}

/// #593: the type of a payload-less variant (`Nil`, or `Opt.Nil`). For a
/// non-generic sum this is just the named type. For a generic sum the variant
/// carries no arguments to infer from, so the binding's expected type is the
/// only ground — `let o: Opt[Int] = Nil`. With no such expectation the type
/// arguments are uninferable.
pub(crate) fn nullary_variant_ty(
    owner: &TypeDecl,
    variant_name: &str,
    span: Span,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if reject_rebrand_construction(&owner.name.name, span, ctx) {
        return None;
    }
    if owner.type_params.is_empty() {
        return Some(named_ty(owner, tys));
    }
    if let Some(exp) = expected
        && let Ty::Named { name, args, .. } = &*tys.get(exp)
        && *name == owner.name.name
        && args.len() == owner.type_params.len()
        && !args.iter().any(|a| contains_var(*a, tys))
    {
        return Some(named_ty_with_args(owner, args.clone(), tys));
    }
    ctx.errors.push(
        CompileError::new(
            "bynk.generics.uninferable_type_arg",
            span,
            format!(
                "cannot infer the type argument(s) of generic type `{}` from payload-less variant `{}`",
                owner.name.name, variant_name
            ),
        )
        .with_note("annotate the binding so the type argument is known — `let x: Name[T] = …`"),
    );
    None
}

pub(crate) fn check_variant_construction(
    owner: &TypeDecl,
    variant_name: &str,
    args: &[Expr],
    span: Span,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let TypeBody::Sum(s) = &owner.body else {
        return None;
    };
    if reject_rebrand_construction(&owner.name.name, span, ctx) {
        return None;
    }
    let variant = s.variants.iter().find(|v| v.name.name == variant_name)?;
    if variant.payload.len() != args.len() {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.variant_arity",
                span,
                format!(
                    "variant `{}` of `{}` expects {} argument(s), but {} were given",
                    variant_name,
                    owner.name.name,
                    variant.payload.len(),
                    args.len()
                ),
            )
            .with_label(variant.span, "variant declared here"),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }

    // #593: a generic sum infers its type arguments from the variant's payload
    // arguments, argument-directed exactly as a generic record infers from its
    // field values (`check_record_construction`). A payload-less or otherwise
    // under-determined variant leans on the binding's expected type — the same
    // pressure valve records use. Non-generic sums take the simple path below.
    if !owner.type_params.is_empty() {
        let vars: HashSet<String> = owner
            .type_params
            .iter()
            .map(|p| p.name.name.clone())
            .collect();
        let mut subst: HashMap<String, TyId> = HashMap::new();
        // Seed from an expected `Name[…]` of this same sum, so `let o: Opt[Int]
        // = Nil` (or an under-determined variant) grounds the arguments.
        if let Some(exp) = expected
            && let Ty::Named {
                name, args: eargs, ..
            } = &*tys.get(exp)
            && *name == owner.name.name
            && eargs.len() == owner.type_params.len()
        {
            subst = type_param_subst(owner, eargs);
        }
        // Pass 1: type each argument once, binding parameters only from fully
        // ground actuals (a `None`/empty list adopting `Var(T)` from the
        // expected type carries no information — mirrors the record rule).
        let mut typed: Vec<(&VariantField, Option<TyId>, Option<TyId>, Span)> = Vec::new();
        for (field, arg) in variant.payload.iter().zip(args.iter()) {
            let pattern = resolve_type_ref_in(&field.type_ref, &ctx.input.types, &vars, tys);
            let expected_now = pattern.map(|p| substitute(p, &subst, tys));
            let actual = type_of(arg, expected_now, ctx);
            if let (Some(pat), Some(act)) = (pattern, actual)
                && !contains_var(act, tys)
            {
                unify(pat, act, &mut subst, tys);
            }
            typed.push((field, pattern, actual, arg.span));
        }
        // Ground the type arguments; an unbound (or still-var-bearing) parameter
        // is uninferable — the binding's type is the pressure valve.
        let mut inferred = Vec::new();
        let mut uninferable = Vec::new();
        for p in &owner.type_params {
            match subst.get(&p.name.name) {
                Some(t) if !contains_var(*t, tys) => inferred.push(*t),
                _ => uninferable.push(p.name.name.clone()),
            }
        }
        if !uninferable.is_empty() {
            ctx.errors.push(
                CompileError::new(
                    "bynk.generics.uninferable_type_arg",
                    span,
                    format!(
                        "cannot infer type parameter{} {} of generic type `{}` from variant `{}`",
                        if uninferable.len() == 1 { "" } else { "s" },
                        uninferable
                            .iter()
                            .map(|n| format!("`{n}`"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        owner.name.name,
                        variant_name,
                    ),
                )
                .with_note(
                    "annotate the binding so the type argument is known — `let x: Name[T] = …`",
                ),
            );
            return None;
        }
        // Pass 2: compatibility per payload field against the substituted type.
        let mut ok = true;
        for (i, (field, pattern, actual, aspan)) in typed.into_iter().enumerate() {
            if let (Some(pat), Some(act)) = (pattern, actual) {
                let expected_ty = substitute(pat, &subst, tys);
                let actual_ty = substitute(act, &subst, tys);
                if !contains_var(expected_ty, tys)
                    && !contains_var(actual_ty, tys)
                    && !compatible(actual_ty, expected_ty, tys)
                {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.variant_payload_mismatch",
                        aspan,
                        format!(
                            "argument {} to variant `{}` has type `{}`, but field `{}` expects `{}`",
                            i + 1,
                            variant_name,
                            actual_ty.display(tys),
                            field.name.name,
                            expected_ty.display(tys)
                        ),
                    ));
                    ok = false;
                }
            } else {
                ok = false;
            }
        }
        if !ok {
            return None;
        }
        return Some(named_ty_with_args(owner, inferred, tys));
    }

    let mut ok = true;
    for (i, (field, arg)) in variant.payload.iter().zip(args.iter()).enumerate() {
        let expected = resolve_type_ref(&field.type_ref, &ctx.input.types, tys);
        let actual = type_of(arg, expected, ctx);
        let (Some(actual), Some(expected)) = (actual, expected) else {
            ok = false;
            continue;
        };
        if !compatible(actual, expected, tys) {
            ctx.errors.push(CompileError::new(
                "bynk.types.variant_payload_mismatch",
                arg.span,
                format!(
                    "argument {} to variant `{}` has type `{}`, but field `{}` expects `{}`",
                    i + 1,
                    variant_name,
                    actual.display(tys),
                    field.name.name,
                    expected.display(tys)
                ),
            ));
            ok = false;
        }
    }
    if !ok {
        return None;
    }
    Some(named_ty(owner, tys))
}

pub(crate) fn check_if(
    cond: &Expr,
    then_block: &Block,
    else_block: &Block,
    if_span: Span,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let cond_ty = type_of(cond, Some(tys.intern(Ty::Base(BaseType::Bool))), ctx);
    if let Some(c) = &cond_ty
        && c.base(tys) != Some(BaseType::Bool)
    {
        ctx.errors.push(CompileError::new(
            "bynk.types.if_non_bool_cond",
            cond.span,
            format!(
                "`if` condition must have type `Bool`, but has type `{}`",
                c.display(tys)
            ),
        ));
    }
    // `is` bindings in the condition flow into the then-branch. This runs
    // after `type_of(cond)` above so the memoised collector sees populated
    // `expr_types`; collecting before type-checking would cache empties.
    let bindings = collect_is_bindings(cond, ctx);
    ctx.push_scope();
    for (name, ty) in bindings {
        ctx.bind(name, ty);
    }
    let then_ty = type_of_block(then_block, expected, ctx);
    ctx.pop_scope();
    // v0.146 (ADR 0170): an `if` with no `else` carries a synthesised `{ () }`
    // else-branch. It is legal only when the then-branch is unit (`()` or
    // `Effect[()]`) — the missing else defaults to `()`, so a valued branch
    // would have no matching value. The synthesised block is typed against the
    // then-branch's type so its `()` lifts to `Effect[()]` when needed.
    if else_block.is_synth_unit() {
        if let Some(t) = then_ty {
            let is_unit = matches!(&*tys.get(t), Ty::Unit)
                || matches!(&*tys.get(t), Ty::Effect(inner) if matches!(&*tys.get(*inner), Ty::Unit));
            if !is_unit {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.if_without_else_requires_unit",
                        if_span,
                        format!(
                            "an `if` with no `else` must produce `()` or `Effect[()]`, but its branch has type `{}`",
                            t.display(tys)
                        ),
                    )
                    .with_label(
                        then_block.tail.span,
                        format!("this branch has type `{}`", t.display(tys)),
                    )
                    .with_note(
                        "add an `else` branch to produce a value, or make the branch a unit effect",
                    ),
                );
                return None;
            }
        }
        let else_ty = type_of_block(else_block, then_ty, ctx);
        return match (then_ty, else_ty) {
            (Some(t), Some(_)) => Some(t),
            _ => None,
        };
    }
    // #1654: the bindings the condition's *falsity* proves are in scope in the
    // else-branch (`if !(o is Some(v)) { 0 } else { v }`, type-system §2.3.6).
    let else_bindings = collect_is_bindings_when(cond, false, ctx);
    ctx.push_scope();
    for (name, ty) in else_bindings {
        ctx.bind(name, ty);
    }
    let else_ty = type_of_block(else_block, expected, ctx);
    ctx.pop_scope();
    match (then_ty, else_ty) {
        (Some(t), Some(e)) => match join_ty(t, e, tys) {
            Some(joined) => Some(joined),
            None => {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.if_branch_mismatch",
                        if_span,
                        format!(
                            "`if` branches produce different types: `{}` and `{}`",
                            t.display(tys),
                            e.display(tys)
                        ),
                    )
                    .with_label(
                        then_block.tail.span,
                        format!("then-branch has type `{}`", t.display(tys)),
                    )
                    .with_label(
                        else_block.tail.span,
                        format!("else-branch has type `{}`", e.display(tys)),
                    )
                    .with_note(
                        "both branches of an `if` must join to a common type — a refined type and its base agree at the base",
                    ),
                );
                None
            }
        },
        _ => None,
    }
}

/// #1659: an expression whose type comes only from its context: a bare
/// `None`, an `Ok(…)`/`Err(…)` (the other type parameter is unknown), an
/// empty list literal, or a `Some(…)`/list whose payload/first element is one
/// of these. `Some(1)` and `[1]` type themselves (review of #1696), so
/// `None == Some(1)` and `[] == [1]` take their type from that side.
fn needs_expected_type(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::None | ExprKind::Ok(_) | ExprKind::Err(_) => true,
        ExprKind::Some(inner) => needs_expected_type(inner),
        ExprKind::ListLit(items) => items.first().is_none_or(needs_expected_type),
        ExprKind::Paren(inner) => needs_expected_type(inner),
        _ => false,
    }
}

pub(crate) fn check_ok(
    inner: &Expr,
    span: Span,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // v0.9: `Ok` is now overloaded between `Result.Ok` and `HttpResult.Ok`.
    // First consult the expected type (propagated from let-annotations, match
    // arms, and the enclosing return type via tail-position auto-lift).
    // #1659: an expected type that settles the question wins outright. A
    // `let r: Result[Int, String] = Ok(1)` in a handler returning
    // `HttpResult[_]` read both the annotation (`Result`) and the return type
    // (`HttpResult`) and reported `ambiguous_constructor`; the enclosing return
    // type is only the fallback when the expected type says neither.
    //
    // One type decides, so `Ok` is never ambiguous: a `Result` builds
    // `Result.Ok`, an `HttpResult` builds `HttpResult.Ok`. The former
    // `bynk.types.ambiguous_constructor` (annotation `Result`, return type
    // `HttpResult`) is withdrawn with it (review of #1696).
    let settles =
        |t: TyId| peel_to_result(t, tys).is_some() || peel_to_http_result(t, tys).is_some();
    let context = expected.filter(|&t| settles(t)).unwrap_or(ctx.return_ty);
    let in_result = peel_to_result(context, tys);
    let in_http = peel_to_http_result(context, tys);
    match (in_result, in_http) {
        (None, Some(t_ty)) => {
            let inner_ty = type_of(inner, Some(t_ty), ctx)?;
            if !compatible(inner_ty, t_ty, tys) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.ok_value_mismatch",
                    inner.span,
                    format!(
                        "`Ok(...)` value has type `{}`, but the surrounding context expects `HttpResult[{}]`",
                        inner_ty.display(tys),
                        t_ty.display(tys),
                    ),
                ));
                return None;
            }
            Some(tys.intern(Ty::HttpResult(t_ty)))
        }
        (Some((t_ty, e_ty)), _) => {
            let inner_ty = type_of(inner, Some(t_ty), ctx)?;
            if !compatible(inner_ty, t_ty, tys) {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.ok_value_mismatch",
                        inner.span,
                        format!(
                            "`Ok(...)` value has type `{}`, but the surrounding context expects `Result[{}, {}]`",
                            inner_ty.display(tys),
                            t_ty.display(tys),
                            e_ty.display(tys)
                        ),
                    )
                    .with_label(ctx.return_ty_span, "context's expected `Result` type"),
                );
                return None;
            }
            Some(tys.intern(Ty::Result(t_ty, e_ty)))
        }
        (None, None) => {
            let _ = type_of(inner, None, ctx);
            ctx.errors.push(
                CompileError::new(
                    "bynk.types.cannot_infer_result_type_params",
                    span,
                    "cannot infer the type parameter of `Ok(...)`",
                )
                .with_note(
                    "add a `let` annotation (`let x: Result[T, E] = Ok(...)`) \
                     or declare the enclosing function's return type as `Result[T, E]` or `HttpResult[T]`",
                ),
            );
            None
        }
    }
}

/// v0.44: type-check a `QueueResult` variant reference or construction. `Ack`
/// is nullary; `Retry` carries one `String` reason. Non-generic.
pub(crate) fn check_queue_variant(
    span: Span,
    variant: QueueVariant,
    args: &[Expr],
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    match variant.payload {
        QueueVariantPayload::None => {
            if !args.is_empty() {
                ctx.errors.push(CompileError::new(
                    "bynk.types.variant_arity",
                    span,
                    format!(
                        "`QueueResult.{}` takes no arguments, but {} were given",
                        variant.name,
                        args.len(),
                    ),
                ));
                return None;
            }
            Some(tys.intern(Ty::QueueResult))
        }
        QueueVariantPayload::Message => {
            if args.len() != 1 {
                ctx.errors.push(CompileError::new(
                    "bynk.types.variant_arity",
                    span,
                    format!(
                        "`QueueResult.{}` expects 1 `String` argument, but {} were given",
                        variant.name,
                        args.len(),
                    ),
                ));
                return None;
            }
            let arg_ty = type_of(&args[0], Some(tys.intern(Ty::Base(BaseType::String))), ctx)?;
            if !compatible(arg_ty, tys.intern(Ty::Base(BaseType::String)), tys) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.argument_mismatch",
                    args[0].span,
                    format!(
                        "`QueueResult.{}` expects a `String` reason, but got `{}`",
                        variant.name,
                        arg_ty.display(tys),
                    ),
                ));
                return None;
            }
            Some(tys.intern(Ty::QueueResult))
        }
    }
}

/// Type-check construction of an `HttpResult[T]` variant (v0.9 §4.3).
///
/// Variants come in six payload shapes:
/// - `Value` (`Ok`, `Created`, `Accepted`) — argument's type is `T`. `T` is
///   taken from the expected type if available; otherwise reported as ambiguous.
/// - `Message` (`BadRequest`, `Conflict`, `TooManyRequests`, `ServerError`, …)
///   — argument must be `String`.
/// - `Location` (`Found`, `SeeOther`, `PermanentRedirect`, …) — argument must
///   be `String`: the redirect target URL, emitted as a `Location` header.
/// - `Streamed` (`Streaming`) — argument must be `Stream[String]`, SSE-framed.
/// - `Raw` (`Raw`) — two arguments, a `Bytes` body then a `String` content-type;
///   the only two-argument shape.
/// - `None` (`NoContent`, `NotFound`, `MethodNotAllowed`, …) — no argument
///   permitted; `T` is taken from the expected type or left inferred.
/// - `RawError` (`RawError`) — three arguments: an `ErrorStatus` name
///   (`Forbidden`, or `ErrorStatus.Forbidden`), a `Bytes` body and a `String`
///   content-type (#2341, L4).
pub(crate) fn check_http_variant(
    span: Span,
    variant: HttpVariant,
    args: &[Expr],
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let expected_t = expected
        .and_then(|t| peel_to_http_result(t, tys))
        .or_else(|| peel_to_http_result(ctx.return_ty, tys));
    match variant.payload {
        HttpVariantPayload::Value => {
            if args.len() != 1 {
                ctx.errors.push(CompileError::new(
                    "bynk.types.variant_arity",
                    span,
                    format!(
                        "`HttpResult.{}` expects 1 argument, but {} were given",
                        variant.name,
                        args.len(),
                    ),
                ));
                return None;
            }
            let arg_ty = type_of(&args[0], expected_t, ctx)?;
            let t_ty = match (expected_t, arg_ty) {
                (Some(t), _) => {
                    if !compatible(arg_ty, t, tys) {
                        ctx.errors.push(CompileError::new(
                            "bynk.types.ok_value_mismatch",
                            args[0].span,
                            format!(
                                "`HttpResult.{}` value has type `{}`, but the surrounding context expects `HttpResult[{}]`",
                                variant.name,
                                arg_ty.display(tys),
                                t.display(tys),
                            ),
                        ));
                        return None;
                    }
                    t
                }
                (None, t) => t,
            };
            Some(tys.intern(Ty::HttpResult(t_ty)))
        }
        HttpVariantPayload::Message => {
            if args.len() != 1 {
                ctx.errors.push(CompileError::new(
                    "bynk.types.variant_arity",
                    span,
                    format!(
                        "`HttpResult.{}` expects 1 `String` argument, but {} were given",
                        variant.name,
                        args.len(),
                    ),
                ));
                return None;
            }
            let arg_ty = type_of(&args[0], Some(tys.intern(Ty::Base(BaseType::String))), ctx)?;
            if !compatible(arg_ty, tys.intern(Ty::Base(BaseType::String)), tys) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.argument_mismatch",
                    args[0].span,
                    format!(
                        "`HttpResult.{}` expects a `String` message, but got `{}`",
                        variant.name,
                        arg_ty.display(tys),
                    ),
                ));
                return None;
            }
            // Inner T is irrelevant for message variants but the type needs
            // a concrete payload. Pick `()` when nothing is known; otherwise
            // use the propagated expected type.
            let t_ty = expected_t.unwrap_or(tys.intern(Ty::Unit));
            Some(tys.intern(Ty::HttpResult(t_ty)))
        }
        HttpVariantPayload::Location => {
            if args.len() != 1 {
                ctx.errors.push(CompileError::new(
                    "bynk.types.variant_arity",
                    span,
                    format!(
                        "`HttpResult.{}` expects 1 `String` location argument, but {} were given",
                        variant.name,
                        args.len(),
                    ),
                ));
                return None;
            }
            let arg_ty = type_of(&args[0], Some(tys.intern(Ty::Base(BaseType::String))), ctx)?;
            if !compatible(arg_ty, tys.intern(Ty::Base(BaseType::String)), tys) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.argument_mismatch",
                    args[0].span,
                    format!(
                        "`HttpResult.{}` expects a `String` location URL, but got `{}`",
                        variant.name,
                        arg_ty.display(tys),
                    ),
                ));
                return None;
            }
            // A redirect carries no body; inner T is irrelevant, so fall back
            // to the propagated expected type or `()`.
            let t_ty = expected_t.unwrap_or(tys.intern(Ty::Unit));
            Some(tys.intern(Ty::HttpResult(t_ty)))
        }
        // v0.101 (real-time track slice 1): `Streaming(s)` carries a
        // `Stream[String]` body, SSE-framed at the boundary. Like the redirect
        // and message shapes, the JSON body parameter `T` is irrelevant.
        HttpVariantPayload::Streamed => {
            let stream_str = Ty::Stream(tys.intern(Ty::Base(BaseType::String)));
            if args.len() != 1 {
                ctx.errors.push(CompileError::new(
                    "bynk.types.variant_arity",
                    span,
                    format!(
                        "`HttpResult.{}` expects 1 `Stream[String]` argument, but {} were given",
                        variant.name,
                        args.len(),
                    ),
                ));
                return None;
            }
            let arg_ty = type_of(&args[0], Some(tys.intern(stream_str.clone())), ctx)?;
            if !compatible(arg_ty, tys.intern(stream_str), tys) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.argument_mismatch",
                    args[0].span,
                    format!(
                        "`HttpResult.{}` expects a `Stream[String]` body, but got `{}`",
                        variant.name,
                        arg_ty.display(tys),
                    ),
                ));
                return None;
            }
            let t_ty = expected_t.unwrap_or(tys.intern(Ty::Unit));
            Some(tys.intern(Ty::HttpResult(t_ty)))
        }
        // v0.111: `Raw(body, contentType)` — the first two-argument shape. The
        // body is a `Bytes` written straight into the response; the content-type
        // is any `String`, unvalidated (opaque, ADR 0143 D3). The JSON body
        // parameter `T` is irrelevant, as for the redirect/message/stream shapes.
        // `RawCreated(body, contentType)` (#2403, L6) shares the shape at 201.
        // `RawTagged(body, contentType, etag)` (caching raw responses) adds the
        // handler's entity tag, a `String` validated at run time, not here: it is
        // usually built from a value (a hash, an object's `etag`).
        HttpVariantPayload::Raw | HttpVariantPayload::RawTagged => {
            let tagged = variant.payload == HttpVariantPayload::RawTagged;
            let (arity, shape) = if tagged {
                (
                    3,
                    "3 arguments (a `Bytes` body, a `String` content-type and a `String` entity tag)",
                )
            } else {
                (
                    2,
                    "2 arguments (a `Bytes` body and a `String` content-type)",
                )
            };
            if args.len() != arity {
                ctx.errors.push(CompileError::new(
                    "bynk.types.variant_arity",
                    span,
                    format!(
                        "`HttpResult.{}` expects {shape}, but {} were given",
                        variant.name,
                        args.len(),
                    ),
                ));
                return None;
            }
            let bytes = tys.intern(Ty::Base(BaseType::Bytes));
            let string = tys.intern(Ty::Base(BaseType::String));
            let expected = [
                (bytes, "a `Bytes` body"),
                (string, "a `String` content-type"),
                (string, "a `String` entity tag"),
            ];
            for (arg, (want, what)) in args.iter().zip(expected) {
                let got = type_of(arg, Some(want), ctx)?;
                if !compatible(got, want, tys) {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.argument_mismatch",
                        arg.span,
                        format!(
                            "`HttpResult.{}` expects {what}, but got `{}`",
                            variant.name,
                            got.display(tys),
                        ),
                    ));
                    return None;
                }
            }
            let t_ty = expected_t.unwrap_or(tys.intern(Ty::Unit));
            Some(tys.intern(Ty::HttpResult(t_ty)))
        }
        // K-streams (#2623): `RawStreamed(body, contentType)` — `Raw`'s shape
        // with a `Stream[Bytes]` body, streamed into the response.
        HttpVariantPayload::RawStreamed => {
            if args.len() != 2 {
                ctx.errors.push(CompileError::new(
                    "bynk.types.variant_arity",
                    span,
                    format!(
                        "`HttpResult.{}` expects 2 arguments (a `Stream[Bytes]` body and a `String` content-type), but {} were given",
                        variant.name,
                        args.len(),
                    ),
                ));
                return None;
            }
            let bytes = tys.intern(Ty::Base(BaseType::Bytes));
            let stream = tys.intern(Ty::Stream(bytes));
            let string = tys.intern(Ty::Base(BaseType::String));
            for (arg, (want, what)) in args.iter().zip([
                (stream, "a `Stream[Bytes]` body"),
                (string, "a `String` content-type"),
            ]) {
                let got = type_of(arg, Some(want), ctx)?;
                if !compatible(got, want, tys) {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.argument_mismatch",
                        arg.span,
                        format!(
                            "`HttpResult.{}` expects {what}, but got `{}`",
                            variant.name,
                            got.display(tys),
                        ),
                    ));
                    return None;
                }
            }
            let t_ty = expected_t.unwrap_or(tys.intern(Ty::Unit));
            Some(tys.intern(Ty::HttpResult(t_ty)))
        }
        // #2341 (L4): `RawError(status, body, contentType)`. The status is an
        // `ErrorStatus` name — one of the vocabulary's own 4xx/5xx variant
        // names, bare or qualified — resolved here to its number, so it is read
        // syntactically and never typed as an expression (a bare `Forbidden`
        // would otherwise be the `HttpResult` value).
        HttpVariantPayload::RawError => {
            if args.len() != 3 {
                ctx.errors.push(CompileError::new(
                    "bynk.types.variant_arity",
                    span,
                    format!(
                        "`HttpResult.{}` expects 3 arguments (an `ErrorStatus`, a `Bytes` body and a `String` content-type), but {} were given",
                        variant.name,
                        args.len(),
                    ),
                ));
                // A status name in first place is read, not typed: typing it
                // would add a misleading `error_status_position`.
                for (i, a) in args.iter().enumerate() {
                    if i == 0 && bynk_syntax::ast::raw_error_status(a).is_some() {
                        continue;
                    }
                    let _ = type_of(a, None, ctx);
                }
                return None;
            }
            if bynk_syntax::ast::raw_error_status(&args[0]).is_none() {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.http.raw_error_status",
                        args[0].span,
                        "`RawError`'s status must be an `ErrorStatus` name, such as `Forbidden` or `ErrorStatus.Conflict`".to_string(),
                    )
                    .with_note(format!(
                        "the error statuses are the 4xx and 5xx `HttpResult` variants: {}",
                        HTTP_VARIANTS
                            .iter()
                            .filter(|v| bynk_syntax::ast::error_status(v.name).is_some())
                            .map(|v| format!("`{}` ({})", v.name, v.status))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )),
                );
            }
            let mut ok = true;
            for (a, want, what) in [
                (&args[1], Ty::Base(BaseType::Bytes), "a `Bytes` body"),
                (
                    &args[2],
                    Ty::Base(BaseType::String),
                    "a `String` content-type",
                ),
            ] {
                let want = tys.intern(want);
                let Some(got) = type_of(a, Some(want), ctx) else {
                    ok = false;
                    continue;
                };
                if !compatible(got, want, tys) {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.argument_mismatch",
                        a.span,
                        format!(
                            "`HttpResult.{}` expects {what}, but got `{}`",
                            variant.name,
                            got.display(tys),
                        ),
                    ));
                    ok = false;
                }
            }
            if !ok {
                return None;
            }
            let t_ty = expected_t.unwrap_or(tys.intern(Ty::Unit));
            Some(tys.intern(Ty::HttpResult(t_ty)))
        }
        HttpVariantPayload::None => {
            if !args.is_empty() {
                ctx.errors.push(CompileError::new(
                    "bynk.types.variant_arity",
                    span,
                    format!(
                        "`HttpResult.{}` takes no arguments, but {} were given",
                        variant.name,
                        args.len(),
                    ),
                ));
                return None;
            }
            let t_ty = expected_t.unwrap_or(tys.intern(Ty::Unit));
            Some(tys.intern(Ty::HttpResult(t_ty)))
        }
    }
}

pub(crate) fn check_err(
    inner: &Expr,
    span: Span,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // Unlike `Ok`, `Err` has no `HttpResult` overload, so it only asks for a
    // surrounding `Result`: the expected type's if it is one, else the return
    // type's (`surrounding_result`). The same preference as `check_ok`'s, without
    // the `HttpResult` arm.
    let surrounding = surrounding_result(expected, ctx.return_ty, tys);
    let expected_e = surrounding.as_ref().map(|(_, e)| *e);
    let inner_ty = type_of(inner, expected_e, ctx)?;
    match surrounding {
        Some((t_ty, e_ty)) => {
            if !compatible(inner_ty, e_ty, tys) {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.err_value_mismatch",
                        inner.span,
                        format!(
                            "`Err(...)` value has type `{}`, but the surrounding context expects `Result[{}, {}]`",
                            inner_ty.display(tys),
                            t_ty.display(tys),
                            e_ty.display(tys)
                        ),
                    )
                    .with_label(ctx.return_ty_span, "context's expected `Result` type"),
                );
                return None;
            }
            Some(tys.intern(Ty::Result(t_ty, e_ty)))
        }
        None => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.types.cannot_infer_result_type_params",
                    span,
                    "cannot infer the value type parameter of `Err(...)`",
                )
                .with_note(
                    "add a `let` annotation or declare the enclosing function's return type as `Result[T, E]`",
                ),
            );
            None
        }
    }
}

pub(crate) fn check_some(
    inner: &Expr,
    _span: Span,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let expected_inner = expected
        .and_then(|t| peel_to_option(t, tys))
        .or_else(|| peel_to_option(ctx.return_ty, tys));
    let inner_ty = type_of(inner, expected_inner, ctx)?;
    if let Some(exp) = &expected_inner
        && !compatible(inner_ty, *exp, tys)
    {
        ctx.errors.push(CompileError::new(
            "bynk.types.some_value_mismatch",
            inner.span,
            format!(
                "`Some(...)` value has type `{}`, but the surrounding context expects `Option[{}]`",
                inner_ty.display(tys),
                exp.display(tys)
            ),
        ));
        return None;
    }
    Some(tys.intern(Ty::Option(inner_ty)))
}

pub(crate) fn check_none(span: Span, expected: Option<TyId>, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    if let Some(t) = expected.and_then(|t| peel_to_option(t, tys)) {
        return Some(tys.intern(Ty::Option(t)));
    }
    if let Some(t) = peel_to_option(ctx.return_ty, tys) {
        return Some(tys.intern(Ty::Option(t)));
    }
    ctx.errors.push(
        CompileError::new(
            "bynk.types.cannot_infer_option_type_param",
            span,
            "cannot infer the value type of `None`",
        )
        .with_note(
            "add an annotation (`let x: Option[T] = None`) or use `None` where the context expects an `Option`",
        ),
    );
    None
}

pub(crate) fn check_question(inner: &Expr, span: Span, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    let inner_ty = type_of(inner, None, ctx)?;
    // v0.153 (ADR 0177): `Option[T]?` lifts into an HttpResult-returning
    // handler — `Some(v)` yields `v`, `None` early-returns `NotFound` (404).
    // Allowed ONLY where the enclosing return peels to `HttpResult[_]` (a bare
    // `HttpResult[T]` handler or `Effect[HttpResult[T]]`); anywhere else an
    // `Option` has no error channel to propagate into.
    if let Ty::Option(t) = &*tys.get(inner_ty) {
        if peel_to_http_result(ctx.return_ty, tys).is_some() {
            return Some(*t);
        }
        ctx.errors.push(
            CompileError::new(
                "bynk.types.question_option_outside_http",
                span,
                "the `?` operator lifts an `Option` only inside a handler returning `HttpResult` (where `None` becomes `NotFound`)",
            )
            .with_label(
                ctx.return_ty_span,
                format!("function returns `{}`", ctx.return_ty.display(tys)),
            )
            .with_note("to turn an `Option` into a `Result` elsewhere, use `.okOr(err)`"),
        );
        return None;
    }
    let Ty::Result(t, e) = &*tys.get(inner_ty) else {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.question_on_non_result",
                inner.span,
                format!(
                    "the `?` operator requires a `Result[T, E]` value (or an `Option[T]` in an HttpResult handler), but got `{}`",
                    inner_ty.display(tys)
                ),
            )
            .with_label(span, "this `?` requires a Result"),
        );
        return None;
    };
    // v0.5: `?` is also valid inside `Effect[Result[T, E]]` — the `Err` is
    // propagated as `Effect.pure(Err(e))`.
    let effect_result = if let Ty::Effect(inner_eff) = &*tys.get(ctx.return_ty)
        && let Ty::Result(_, eff_e) = &*tys.get(*inner_eff)
    {
        Some(*eff_e)
    } else {
        None
    };
    let Ty::Result(_ret_t, ret_e) = &*tys.get(ctx.return_ty) else {
        if let Some(eff_e) = effect_result {
            // v0.154 (ADR 0178): the error propagates as-is when compatible, or
            // via a declared embedding (`eff_e embeds e as V`) that `?` auto-wraps.
            if !compatible(*e, eff_e, tys) {
                match embedding_for(eff_e, *e, &ctx.input.types, tys) {
                    Some((sum_name, _)) => {
                        // #907: the emitter lowers this auto-wrap to a real
                        // runtime `SumType.Variant(...)` reference (`embed_conversion`
                        // in `bynk-emit/src/emitter/lower.rs`) — the same rebrand
                        // hazard as an explicit variant construction, just with no
                        // source-level construction site for the checks above to
                        // catch. Gate it here too.
                        if reject_rebrand_construction(&sum_name, span, ctx) {
                            return None;
                        }
                    }
                    None => {
                        ctx.errors.push(CompileError::new(
                            "bynk.types.question_error_mismatch",
                            span,
                            format!(
                                "the `?` operator propagates an error of type `{}`, but the enclosing function returns `Effect[Result[_, {}]]` (and no `embeds` clause converts it)",
                                e.display(tys),
                                eff_e.display(tys)
                            ),
                        ));
                        return None;
                    }
                }
            }
            return Some(*t);
        }
        ctx.errors.push(
            CompileError::new(
                "bynk.types.question_outside_result",
                span,
                "the `?` operator can only be used inside a function returning `Result`",
            )
            .with_label(
                ctx.return_ty_span,
                format!("function returns `{}`", ctx.return_ty.display(tys)),
            ),
        );
        return None;
    };
    if !compatible(*e, *ret_e, tys) {
        match embedding_for(*ret_e, *e, &ctx.input.types, tys) {
            Some((sum_name, _)) => {
                if reject_rebrand_construction(&sum_name, span, ctx) {
                    return None;
                }
            }
            None => {
                ctx.errors.push(CompileError::new(
                    "bynk.types.question_error_mismatch",
                    span,
                    format!(
                        "the `?` operator propagates an error of type `{}`, but the enclosing function returns `Result[_, {}]` (and no `embeds` clause converts it)",
                        e.display(tys),
                        ret_e.display(tys)
                    ),
                ));
                return None;
            }
        }
    }
    Some(*t)
}

pub(crate) fn check_record_spread(
    type_name: Option<&Ident>,
    base: &Expr,
    overrides: &[FieldInit],
    span: Span,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // 1) Determine the record type.
    let base_ty = type_of(base, expected, ctx)?;
    let (record_name, base_args) = match &*tys.get(base_ty) {
        Ty::Named {
            name,
            kind: NamedKind::Record,
            args,
        } => (name.clone(), args.clone()),
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.record_spread.non_record_base",
                base.span,
                format!(
                    "record spread requires a record-typed base, but got `{}`",
                    base_ty.display(tys)
                ),
            ));
            return None;
        }
    };
    if let Some(tn) = type_name
        && tn.name != record_name
    {
        ctx.errors.push(CompileError::new(
            "bynk.record_spread.type_mismatch",
            tn.span,
            format!(
                "spread type prefix `{}` does not match the base's type `{}`",
                tn.name, record_name
            ),
        ));
    }
    // #1948: the base may be a consumed context's type the caller does not
    // name, whose fields the projection declares.
    let decl = lookup_type_decl(&record_name, ctx)?;
    let TypeBody::Record(r) = &decl.body else {
        return None;
    };
    let declared: HashMap<&str, &RecordField> =
        r.fields.iter().map(|f| (f.name.name.as_str(), f)).collect();
    let _ = span;
    for f in overrides {
        let Some(declared_field) = declared.get(f.name.name.as_str()) else {
            ctx.errors.push(CompileError::new(
                "bynk.record_spread.unknown_field",
                f.name.span,
                format!(
                    "record type `{}` has no field `{}`",
                    record_name, f.name.name
                ),
            ));
            continue;
        };
        // v0.36 (ADR 0069, slice 2): a spread override label references the field.
        ctx.refs.record(
            f.name.span,
            SymbolKind::Field,
            &format!("{}.{}", record_name, f.name.name),
        );
        // v0.157 (ADR 0183): substitute the base's applied type arguments into
        // the declared field type, so an override on a generic record
        // (`{ ...p, items: [...] }` where `p: Paginated[String]`) is checked
        // against `List[String]`, not a `None`-resolving type variable.
        let expected_ty = instantiate_field_ty(
            &decl,
            &base_args,
            &declared_field.type_ref,
            &ctx.input.types,
            tys,
        );
        let value_ty = match &f.value {
            Some(v) => type_of(v, expected_ty, ctx),
            None => ctx.lookup(&f.name.name),
        };
        if let (Some(actual), Some(expected_ty)) = (value_ty, expected_ty)
            && !compatible(actual, expected_ty, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.record_spread.field_type_mismatch",
                f.value.as_ref().map(|v| v.span).unwrap_or(f.name.span),
                format!(
                    "spread override of field `{}` has type `{}`, but the declared type is `{}`",
                    f.name.name,
                    actual.display(tys),
                    expected_ty.display(tys)
                ),
            ));
        }
    }
    Some(base_ty)
}

/// #907: a context that `uses` a commons sum type only ever sees that type
/// rebranded on emission (`emit_context_rebrands` in `bynk-emit`) — the
/// rebrand re-exports the public name as a *type* alias, never a value, so
/// the type's own variant constructors are not in scope there. Checking this
/// in `bynk-check` catches what would otherwise be a clean `bynkc check`
/// followed by a `tsc` failure (TS2693) on the emitted output. Called from
/// every site that resolves to a runtime `SumType.Variant(...)` reference in
/// the emitted output: explicit construction (a bare `Text(...)` or
/// qualified `MessageArg.Text(...)`) *and* a `?`-operator declared-embedding
/// auto-wrap (ADR 0178's `embeds E as V`, lowered by `embed_conversion` in
/// `bynk-emit/src/emitter/lower.rs`) — the latter has no source-level
/// construction expression at all, so it needs its own call site rather than
/// falling out of `check_variant_construction`/`nullary_variant_ty`.
///
/// Record-literal construction of a `uses`-sourced commons record type is
/// *not* gated here: `lower_record_construction`
/// (`bynk-emit/src/emitter/lower.rs`) never emits the type name at all — a
/// record literal lowers to a plain structural `{ field: value, ... }`
/// object, and since #1704 the rebrand's brand is optional, so that
/// unbranded object fits the context's branded type.
/// Refined/opaque types are likewise unaffected by the sum-specific gate
/// below — the emitter re-exports a real value (`.of`/`.unsafe`) for those.
fn reject_rebrand_construction(type_name: &str, span: Span, ctx: &mut Ctx) -> bool {
    if !ctx.input.is_context || !ctx.input.uses_commons_type_names.contains(type_name) {
        return false;
    }
    ctx.errors.push(
        CompileError::new(
            "bynk.context.rebrand_construction",
            span,
            format!(
                "cannot construct `{type_name}` here — its constructors are not in scope in this context"
            ),
        )
        .with_note(
            "this type is `uses`-sourced from a commons and is rebranded per-context on \
             emission; construct it through that commons's own builder function(s) instead",
        ),
    );
    true
}

pub(crate) fn check_record_construction(
    type_name: &Ident,
    fields: &[FieldInit],
    expected: Option<TyId>,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let decl = ctx.input.types.get(&type_name.name)?.clone();
    ctx.refs
        .record(type_name.span, SymbolKind::Type, &type_name.name);
    // An opaque type built with record-literal syntax is reported by the
    // resolver (`bynk.resolve.opaque_record_construction`), which runs first;
    // here it only stops the record checks from cascading (#1662).
    if matches!(decl.body, TypeBody::Opaque { .. }) {
        return None;
    }
    let TypeBody::Record(r) = &decl.body else {
        return None;
    };
    // Collect declared fields.
    let declared: HashMap<&str, &RecordField> =
        r.fields.iter().map(|f| (f.name.name.as_str(), f)).collect();

    // Field-set validation — missing required, unknown extra, duplicate init,
    // and shorthand-in-scope. Shared verbatim with the resolver's reference
    // walk (#711): the resolver runs it for `fn`/method bodies but skips
    // `service`/`agent`/`actor` items, and `check` only runs once `resolve`
    // passed clean, so for `fn`/method bodies this is a no-op (already gated,
    // no double-report) and for handler bodies the checker is the only
    // backstop. A single implementation keeps a service handler and an `fn`
    // rejecting the identical literal identically. The shorthand predicate is
    // the checker's lexical scope (mirrors the resolver's `name_in_scope`).
    let scopes = &ctx.scopes;
    crate::resolver::check_record_field_set(
        type_name,
        fields,
        r,
        span,
        |n| scopes.iter().rev().any(|s| s.contains_key(n)),
        ctx.errors,
    );

    // Non-generic records are the common case: resolve each field type directly
    // and compare — no type variables, no substitution churn (ADR 0183 #10).
    if decl.type_params.is_empty() {
        for f in fields {
            if let Some(declared_field) = declared.get(f.name.name.as_str()) {
                ctx.refs.record(
                    f.name.span,
                    SymbolKind::Field,
                    &format!("{}.{}", type_name.name, f.name.name),
                );
                let expected = resolve_type_ref(&declared_field.type_ref, &ctx.input.types, tys);
                let value_ty = match &f.value {
                    Some(v) => type_of(v, expected, ctx),
                    None => ctx.lookup(&f.name.name),
                };
                if let (Some(actual), Some(expected)) = (value_ty, expected)
                    && !compatible(actual, expected, tys)
                {
                    ctx.errors.push(
                        CompileError::new(
                            "bynk.types.field_value_mismatch",
                            f.value.as_ref().map(|v| v.span).unwrap_or(f.name.span),
                            format!(
                                "field `{}` expects `{}`, but the value has type `{}`",
                                f.name.name,
                                expected.display(tys),
                                actual.display(tys)
                            ),
                        )
                        .with_label(declared_field.name.span, "field declared here"),
                    );
                }
            }
        }
        return Some(named_ty(&decl, tys));
    }

    // v0.157 (ADR 0183): a generic record infers its type arguments from the
    // field values (argument-directed, like a generic call — records have no
    // function-typed fields, so there is no lambda-ordering subtlety). The
    // declared type parameters are the flexible vars; unification binds them.
    let vars: HashSet<String> = decl
        .type_params
        .iter()
        .map(|p| p.name.name.clone())
        .collect();
    let mut subst: HashMap<String, TyId> = HashMap::new();
    // Seed the substitution from an expected `Name[…]` of this same type — this
    // lets `let p: Paginated[String] = Paginated { … }` ground the arguments
    // even when a field (e.g. an empty list) cannot determine them.
    if let Some(exp) = expected
        && let Ty::Named { name, args, .. } = &*tys.get(exp)
        && *name == decl.name.name
        && args.len() == decl.type_params.len()
    {
        subst = type_param_subst(&decl, args);
    }

    // Pass 1: type each provided field value once, binding type parameters only
    // from **fully ground** actuals. A field whose value type still carries a
    // var — an empty list adopts `List[Var(T)]` from the expected type, a `None`
    // adopts `Option[Var(T)]` — carries no information about `T` and must not
    // bind it (that made inference order-dependent — ADR 0183 #5).
    let mut typed: Vec<(&RecordField, Option<TyId>, Option<TyId>, Span)> = Vec::new();
    for f in fields {
        if let Some(declared_field) = declared.get(f.name.name.as_str()) {
            // v0.36 (ADR 0069, slice 2): a construction field label is a
            // reference to the record field.
            ctx.refs.record(
                f.name.span,
                SymbolKind::Field,
                &format!("{}.{}", type_name.name, f.name.name),
            );
            let pattern =
                resolve_type_ref_in(&declared_field.type_ref, &ctx.input.types, &vars, tys);
            let expected_now = pattern.map(|p| substitute(p, &subst, tys));
            let value_ty = match &f.value {
                Some(v) => type_of(v, expected_now, ctx),
                None => ctx.lookup(&f.name.name),
            };
            // #594: bind type parameters from an actual that is ground *up to the
            // enclosing generic method's rigid vars* — inside `fn Box.map[U](self,
            // …) -> Box[U]`, constructing `Box { value: <U> }` legitimately infers
            // `Box[U]`. A still-*flexible* var (an empty list adopting `List[Var(T)]`
            // from the expected type, ADR 0183 #5) carries no information and is
            // still skipped. Outside a generic body `ctx.type_vars` is empty, so this
            // is exactly `!contains_var(actual, tys)`.
            if let (Some(pat), Some(actual)) = (pattern, value_ty)
                && !contains_flexible_var(actual, &ctx.type_vars, tys)
            {
                unify(pat, actual, &mut subst, tys);
            }
            let fspan = f.value.as_ref().map(|v| v.span).unwrap_or(f.name.span);
            typed.push((declared_field, pattern, value_ty, fspan));
        }
    }

    // Ground the type arguments; an unbound (or still-var-bearing) parameter is
    // uninferable — the surrounding binding's type is the pressure valve.
    let mut args = Vec::new();
    let mut uninferable = Vec::new();
    for p in &decl.type_params {
        match subst.get(&p.name.name) {
            // #594: a rigid var (the enclosing method's own parameter) is a fully
            // determined argument here — `Box[U]` is ground inside `Box.map[U]`.
            Some(t) if !contains_flexible_var(*t, &ctx.type_vars, tys) => args.push(*t),
            _ => uninferable.push(p.name.name.clone()),
        }
    }
    if !uninferable.is_empty() {
        ctx.errors.push(
            CompileError::new(
                "bynk.generics.uninferable_type_arg",
                type_name.span,
                format!(
                    "cannot infer type parameter{} {} of generic type `{}` from the given fields",
                    if uninferable.len() == 1 { "" } else { "s" },
                    uninferable
                        .iter()
                        .map(|n| format!("`{n}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    decl.name.name
                ),
            )
            .with_note("annotate the binding so the element type is known — `let x: Name[T] = …`"),
        );
        return None;
    }

    // Pass 2: compatibility per field against the substituted field type. Both
    // sides are grounded through `subst` — a field that adopted `Var(T)` from
    // the expected type (an empty list) resolves to the inferred argument on
    // both sides, so it neither errors nor is skipped spuriously.
    for (declared_field, pattern, value_ty, fspan) in typed {
        if let (Some(pat), Some(actual)) = (pattern, value_ty) {
            let expected_ty = substitute(pat, &subst, tys);
            let actual_ty = substitute(actual, &subst, tys);
            if !contains_var(expected_ty, tys)
                && !contains_var(actual_ty, tys)
                && !compatible(actual_ty, expected_ty, tys)
            {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.field_value_mismatch",
                        fspan,
                        format!(
                            "field `{}` expects `{}`, but the value has type `{}`",
                            declared_field.name.name,
                            expected_ty.display(tys),
                            actual_ty.display(tys)
                        ),
                    )
                    .with_label(declared_field.name.span, "field declared here"),
                );
            }
        }
    }

    Some(named_ty_with_args(&decl, args, tys))
}

pub(crate) fn check_field_access(
    receiver: &Expr,
    field: &Ident,
    // #593: grounds a qualified payload-less generic variant (`Opt.Nil`).
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // #1925: `thing.url` where a local function `thing` hides a `uses`-imported
    // asset — a field read means the asset, which is out of scope. Only as a
    // receiver: a bare `thing` elsewhere is the local function, and its own
    // rule (`fn_without_call`) gives the useful advice.
    if let ExprKind::Ident(id) = &receiver.kind
        && ctx.lookup(id.name.as_str()).is_none()
        && ctx.input.fns.contains_key(&id.name)
        && let Some(err) = ctx.input.shadowed_import_error(&id.name, id.span)
    {
        ctx.errors.push(err);
        return None;
    }
    // v0.158 (ADR 0184): `<map>.entries` / `.keys` / `.values` on a `store
    // Map[K, V]` field — the key-exposing lazy queries. Dispatched by receiver
    // provenance (a bare ident naming a store map, not shadowed by a local),
    // mirroring the query-op method dispatch. `.entries` lifts the map into a
    // `Query[MapEntry[K, V]]` (each entry a nominal `{ key, value }` record);
    // `.keys`/`.values` into `Query[K]`/`Query[V]`.
    if let ExprKind::Ident(id) = &receiver.kind
        && ctx.lookup(id.name.as_str()).is_none()
        && let Some(StoreField::Map(k, v)) = ctx.store_fields.get(&id.name).cloned()
    {
        // The key-aware accessors are not offered on a held `Map[K, Connection]`
        // (v0.158, ADR 0184): a held resource has its own single-owner discipline
        // (§2.9) and a dedicated broadcast surface (`forEach`/`parTraverse`/…),
        // so the connections are iterated there, never lifted into a key query or
        // a `MapEntry` record.
        if v.is_held(tys)
            && matches!(
                field.name.as_str(),
                map_query::ENTRIES | map_query::KEYS | map_query::VALUES
            )
        {
            ctx.errors.push(CompileError::new(
                "bynk.held.query_accessor_on_held_map",
                field.span,
                format!(
                    "`.{}` is not available on a held `Map[K, {}]` — a held resource is iterated with the broadcast ops (`forEach`/`parTraverse`/`traverseAll`), not a key query",
                    field.name,
                    v.display(tys)
                ),
            ));
            return None;
        }
        let elem = match field.name.as_str() {
            map_query::ENTRIES => map_entry_ty(k, v, tys),
            map_query::KEYS => k,
            map_query::VALUES => v,
            other => {
                ctx.errors.push(CompileError::new(
                    "bynk.store.unknown_map_accessor",
                    field.span,
                    format!(
                        "a `store Map` exposes `.entries`, `.keys`, and `.values` as lazy \
                         queries, not `.{other}` — its effectful entry ops \
                         (`put`/`get`/`update`/`upsert`/`remove`/`contains`/`size`) are method calls",
                    ),
                ));
                return None;
            }
        };
        let q = Ty::Query(elem);
        // Record the receiver's lifted query type (the dispatch keys off the
        // store field, not a typed receiver) so hover/linearity see it.
        ctx.expr_types.insert(
            receiver.id,
            TypedExpr {
                span: receiver.span,
                ty: tys.intern(q.clone()),
            },
        );
        return Some(tys.intern(q));
    }
    // Qualified nullary variant: `TypeName.Variant` where TypeName is a
    // declared sum type and Variant is one of its payload-less variants.
    if let ExprKind::Ident(id) = &receiver.kind
        && ctx.lookup(id.name.as_str()).is_none()
        && let Some(decl) = ctx.input.types.get(&id.name)
        && let TypeBody::Sum(s) = &decl.body
        && let Some(variant) = s.variants.iter().find(|v| v.name.name == field.name)
    {
        if !variant.payload.is_empty() {
            ctx.errors.push(
                CompileError::new(
                    "bynk.types.variant_missing_payload",
                    field.span,
                    format!(
                        "variant `{}.{}` has a payload — call it with arguments",
                        id.name, field.name
                    ),
                )
                .with_label(variant.span, "variant declared here"),
            );
            return None;
        }
        return nullary_variant_ty(decl, field.name.as_str(), field.span, expected, ctx);
    }
    let recv_ty = type_of(receiver, None, ctx)?;
    // v0.45: a verified actor binding exposes exactly `.identity` — the sealed,
    // boundary-minted identity value. No other member is valid.
    if let Ty::Actor(identity) = &*tys.get(recv_ty) {
        if field.name == "identity" {
            return Some(*identity);
        }
        // #2380 (L5): an `Oidc` binder also exposes its token's verified claims.
        let oidc = matches!(&receiver.kind, ExprKind::Ident(id) if ctx.oidc_binder.as_deref() == Some(id.name.as_str()));
        if field.name == "claims" && oidc {
            return Some(tys.intern(Ty::TokenClaims));
        }
        ctx.errors.push(
            CompileError::new(
                "bynk.types.unknown_field",
                field.span,
                format!(
                    "a verified actor exposes `.identity`{}, not `.{}`",
                    if oidc { " and `.claims`" } else { "" },
                    field.name
                ),
            )
            .with_note(if field.name == "claims" {
                "`.claims` is on an `Oidc` actor's binder: the verified token's claims"
            } else {
                "`.identity` is the identity the actor's seam verified"
            }),
        );
        return None;
    }
    // `.raw` on an opaque value: only available within the defining commons.
    // Returns the base type. The emitter compiles this to a `value as base`
    // type assertion (see emitter::lower_expr for FieldAccess).
    if field.name == RAW
        && let Ty::Named {
            kind: NamedKind::Opaque(base),
            name,
            ..
        } = &*tys.get(recv_ty)
    {
        if !ctx.input.is_local_type(name) {
            ctx.errors.push(
                CompileError::new(
                    "bynk.types.opaque_raw_outside",
                    field.span,
                    format!(
                        "`.raw` on opaque type `{}` is only available within its defining commons",
                        name
                    ),
                )
                .with_note(
                    "the base representation of an opaque type is hidden from importers; \
                     define a method on the type or use a public accessor",
                ),
            );
            return None;
        }
        return Some(tys.intern(Ty::Base(*base)));
    }
    // #1883, slice 1a ([DECISION H]): `Asset` is compiler-known, with one
    // field, `.url` — the content-hashed URL the platform serves the file at.
    if recv_ty == tys.intern(Ty::Asset) {
        return match field.name.as_str() {
            "url" => Some(tys.intern(Ty::Base(BaseType::String))),
            other => {
                ctx.errors.push(CompileError::new(
                    "bynk.types.unknown_field",
                    field.span,
                    format!("`Asset` has no field `{other}` — its one field is `url`"),
                ));
                None
            }
        };
    }
    // v0.22b: `JsonError` is a compiler-known record (ADR 0047) — uniform
    // `String` fields so a decode failure is inspectable in Bynk.
    if recv_ty == tys.intern(Ty::JsonError) {
        return match field.name.as_str() {
            "kind" | "path" | "message" => Some(tys.intern(Ty::Base(BaseType::String))),
            other => {
                ctx.errors.push(CompileError::new(
                    "bynk.types.unknown_field",
                    field.span,
                    format!(
                        "`JsonError` has no field `{other}` — its fields are `kind`, `path`, `message`"
                    ),
                ));
                None
            }
        };
    }
    // v0.158 (ADR 0184): `MapEntry[K, V]` is a compiler-known generic record
    // (the element of a map's `.entries` query) with exactly `.key`/`.value` —
    // it has no user `TypeDecl`, so its fields resolve here (like `JsonError`),
    // reading the two type arguments.
    if let Ty::Named {
        name,
        kind: NamedKind::Record,
        args,
    } = &*tys.get(recv_ty)
        && name == MAP_ENTRY
        && args.len() == 2
        && !ctx.input.types.contains_key(name.as_str())
    {
        return match field.name.as_str() {
            map_query::KEY => Some(args[0]),
            map_query::VALUE => Some(args[1]),
            other => {
                ctx.errors.push(CompileError::new(
                    "bynk.types.unknown_field",
                    field.span,
                    format!(
                        "`MapEntry` has no field `{other}` — a map entry exposes `key` and `value`"
                    ),
                ));
                None
            }
        };
    }
    let recv_node = tys.get(recv_ty);
    let Ty::Named {
        name,
        kind: NamedKind::Record,
        args,
    } = &*recv_node
    else {
        let mut err = CompileError::new(
            "bynk.types.field_access_on_non_record",
            field.span,
            format!(
                "field access requires a record type, but the receiver has type `{}`",
                recv_ty.display(tys)
            ),
        );
        // #48: a `.raw` (or any field) on a *refined* value is a common
        // mistake — refined values widen to their base type, so there's
        // nothing to unwrap. Say what's right, and offer the mechanical fix
        // (drop `.raw`) when that's what was written.
        if let Ty::Named {
            kind: NamedKind::Refined(_),
            ..
        } = &*tys.get(recv_ty)
        {
            err = err.with_note(
                "a refined value is usable wherever its base type is expected — \
                 pass it directly (`.raw` is for opaque types)",
            );
            if field.name == RAW {
                err = err.with_suggestion(
                    "remove `.raw` — a refined value is already its base type",
                    vec![(
                        bynk_syntax::span::Span::new(receiver.span.end, field.span.end),
                        String::new(),
                    )],
                    Applicability::MachineApplicable,
                );
            }
        }
        ctx.errors.push(err);
        return None;
    };
    // #1948: a value of a consumed context's type the caller does not name
    // (an unexported return or field type) is read through the projection,
    // so its fields are checked like any other record's. `projection` is
    // kept to resolve the field's own type, which the caller may not name
    // either.
    let mut projection = None;
    let decl = match ctx.input.types.get(name) {
        Some(decl) => decl.clone(),
        None => {
            let (table, ambiguous) = consumer_projection(ctx)?;
            let Some(decl) = table.get(name).cloned() else {
                if ambiguous.contains(name) {
                    let message = format!("cannot read field `{}` of a `{name}`", field.name);
                    ctx.errors
                        .push(ambiguous_type_error(field.span, message, name));
                }
                return None;
            };
            projection = Some((table, ambiguous));
            decl
        }
    };
    let TypeBody::Record(r) = &decl.body else {
        return None;
    };
    let Some(field_decl) = r.fields.iter().find(|f| f.name.name == field.name) else {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.unknown_field",
                field.span,
                format!("record type `{}` has no field `{}`", name, field.name),
            )
            // Finding #46: `decl` comes from the combined cross-file symbol
            // table — see resolver.rs:1029 for the full rationale.
            .with_note("type declared here"),
        );
        return None;
    };
    // v0.36 (ADR 0069, slice 2): the field is an index symbol, keyed by the
    // compound `"Type.field"` name (read access is a reference site).
    ctx.refs.record(
        field.span,
        SymbolKind::Field,
        &format!("{name}.{}", field.name),
    );
    // v0.157 (ADR 0183): resolve the field type at this instantiation —
    // substituting the record's type parameters by the receiver's `args`.
    // #1948: a field type the caller's table does not resolve may still be
    // one the projection does (a transparent export's unexported field).
    let field_ref = &field_decl.type_ref;
    if projection.is_none() {
        if let Some(ty) = instantiate_field_ty(&decl, args, field_ref, &ctx.input.types, tys) {
            return Some(ty);
        }
        projection = consumer_projection(ctx);
    }
    let (table, ambiguous) = projection.as_ref()?;
    let ty = instantiate_field_ty(&decl, args, field_ref, table, tys);
    // A field type the projection leaves ambiguous untypes the read just as
    // an ambiguous receiver does, so it is reported the same way.
    if ty.is_none() {
        let vars: HashSet<&str> = decl
            .type_params
            .iter()
            .map(|p| p.name.name.as_str())
            .collect();
        let mut names = Vec::new();
        crate::project_model::type_ref_names(field_ref, &vars, &mut names);
        if let Some(amb) = names.iter().find(|n| ambiguous.contains(*n)) {
            let message = format!(
                "cannot read field `{}` of a `{name}`: its type names `{amb}`",
                field.name
            );
            ctx.errors
                .push(ambiguous_type_error(field.span, message, amb));
        }
    }
    ty
}

/// #1948: a field read untyped because a type it reaches, `name`, is declared
/// with different shapes by more than one consumed context.
fn ambiguous_type_error(span: Span, message: String, name: &str) -> CompileError {
    CompileError::new("bynk.boundary.ambiguous_type", span, message).with_note(format!(
        "more than one consumed context declares a type `{name}`, with different shapes, so \
         a value of it cannot be traced to one of them; rename one"
    ))
}

/// What a `match` ranges over (v0.130, DECISION D). A discriminant is either
/// variant-kind (a sum / `Result` / `Option`, carrying its variants) or
/// literal-kind (a primitive `Int`/`String`/`Bool`, or a refinement over one).
enum MatchKind {
    Variant,
    Literal(BaseType),
}

/// The literal-matchable base of a scrutinee type, if any: a bare `Int`/
/// `String`/`Bool`, or a refined type over one of those bases (which widens to
/// its base — ADR 0001 / `NamedKind::Refined`). Opaque types do *not* widen, so
/// they are not literal-matchable.
fn literal_base_of(ty: TyId, tys: &Types) -> Option<BaseType> {
    let base = match &*tys.get(ty) {
        Ty::Base(b) => *b,
        Ty::Named {
            kind: NamedKind::Refined(b),
            ..
        } => *b,
        _ => return None,
    };
    matches!(base, BaseType::Int | BaseType::String | BaseType::Bool).then_some(base)
}

/// The base type a literal pattern matches against.
fn literal_value_base(value: &LiteralValue) -> BaseType {
    match value {
        LiteralValue::Int(_) => BaseType::Int,
        LiteralValue::Str(_) => BaseType::String,
        LiteralValue::Bool(_) => BaseType::Bool,
    }
}

/// A refined type widened to its base (`Email` → `String`); any other type is
/// returned unchanged. Mirrors the ADR 0001 widening used by `literal_base_of`.
fn widen_refined(t: TyId, tys: &Types) -> TyId {
    match &*tys.get(t) {
        Ty::Named {
            kind: NamedKind::Refined(base),
            ..
        } => tys.intern(Ty::Base(*base)),
        _ => t,
    }
}

/// The join (least upper bound) of two `match`-arm / `if`-branch body types,
/// along the refined→base widening and structural-covariance lattice that
/// `compatible` already encodes. `None` when the two have no common supertype.
///
/// #726: branches must *agree*, but exact `Ty` equality wrongly rejected valid
/// mixes such as a refined `Email` in one arm and its base `String` in another
/// (both usable at `String`). We pick the wider type when one branch is usable
/// where the other is expected, then fall back to widening both refined types
/// to their shared base — so the fold is independent of arm order.
fn join_ty(a: TyId, b: TyId, tys: &Types) -> Option<TyId> {
    if a == b {
        return Some(a);
    }
    // One branch is a subtype of the other → the supertype is the join.
    if compatible(a, b, tys) {
        return Some(b);
    }
    if compatible(b, a, tys) {
        return Some(a);
    }
    // Two distinct refined types over the same base (e.g. `Email` and
    // `Username`, both `String`): widen each to its base and retry.
    let (wa, wb) = (widen_refined(a, tys), widen_refined(b, tys));
    if wa != a || wb != b {
        return join_ty(wa, wb, tys);
    }
    None
}

pub(crate) fn check_match(
    discriminant: &Expr,
    arms: &[MatchArm],
    span: Span,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let disc_ty = type_of(discriminant, None, ctx)?;
    // v0.130 (DECISION D): a match is *either* variant-kind (sum/`Result`/
    // `Option`) *or* literal-kind (a primitive `Int`/`String`/`Bool` scrutinee,
    // including a refined type over one of those bases). Anything else — a
    // record, `Float`, a function — cannot be matched.
    let kind = if variants_of(disc_ty, &ctx.input.types, tys).is_some() {
        MatchKind::Variant
    } else if let Some(base) = literal_base_of(disc_ty, tys) {
        MatchKind::Literal(base)
    } else {
        ctx.errors.push(CompileError::new(
            "bynk.types.match_non_sum_discriminant",
            discriminant.span,
            format!(
                "cannot match on a value of type `{}` — `match` requires a sum, `Result`, `Option`, or a primitive `Int`/`String`/`Bool`",
                disc_ty.display(tys)
            ),
        ));
        return None;
    };
    // #2010: a `match` the parser built from `let P <- e else …` scrutinises
    // its hidden binder. Its arms are the pattern's (the rest of the block)
    // and the `else`'s, so its diagnostics are worded in those terms.
    let let_else = matches!(
        &discriminant.kind,
        ExprKind::Ident(i) if i.name.starts_with(LET_ELSE_BINDER_PREFIX)
    );
    let mut arm_types: Vec<(TyId, Span)> = Vec::new();
    let mut saw_wildcard = false;
    // Unguarded, refutable patterns already seen — structural duplicate
    // detection (ADR 0169: `Err(A)` and `Err(B)` are distinct, so a duplicate
    // keys on pattern *shape*, not the outer variant name).
    let mut seen: Vec<&Pattern> = Vec::new();
    // #1656: every earlier unguarded arm's pattern, for the usefulness check.
    // A guarded arm never covers (its guard may fail), so it is not recorded.
    let mut covering: Vec<&Pattern> = Vec::new();
    for (index, arm) in arms.iter().enumerate() {
        ctx.push_scope();
        // Validate the pattern against the scrutinee and bind its names,
        // recursing through nested payload patterns (ADR 0169).
        let errors_before = ctx.errors.len();
        check_pattern(&arm.pattern, disc_ty, ctx);
        let pattern_ok = ctx.errors.len() == errors_before;
        // #2010: a `let … else` pattern's names are the author's `let`
        // bindings, in scope for the rest of the block: they get the inlay
        // hints and locals entries a `let` gets (a `match` arm's do not).
        if let_else && index == 0 && pattern_ok {
            record_let_else_bindings(arm, ctx);
        }
        let mut duplicate_reported = false;
        // Structural duplicate detection over unguarded, refutable patterns —
        // `Err(A)` and `Err(B)` are distinct, `Ok(_)` twice is a duplicate.
        // #474: an or-pattern is checked alternative-by-alternative, so
        // `1 | 2 => …` followed by a later `2 => …` is caught too.
        if arm.guard.is_none() && !arm.pattern.is_irrefutable() {
            for cand in pattern_alternatives(&arm.pattern) {
                if cand.is_irrefutable() {
                    continue;
                }
                if seen.iter().any(|p| patterns_equal(p, cand)) {
                    let (code, msg) = match cand {
                        Pattern::Literal { value, .. } => (
                            "bynk.types.duplicate_literal_arm",
                            format!("literal `{}` is matched more than once", value.describe()),
                        ),
                        _ => (
                            "bynk.types.duplicate_variant_arm",
                            format!("`{}` is matched more than once", describe_pattern(cand)),
                        ),
                    };
                    ctx.errors.push(CompileError::new(code, cand.span(), msg));
                    duplicate_reported = true;
                } else {
                    seen.push(cand);
                }
            }
        }
        // #1656 (static-semantics: arms MUST NOT be unreachable): an arm is
        // unreachable when every value its pattern matches is already matched
        // by an earlier unguarded arm. A duplicate is the simplest case and
        // already has its own report; an ill-typed pattern has one too.
        if pattern_ok
            && !duplicate_reported
            && pattern_covered(&covering, &arm.pattern, disc_ty, ctx)
        {
            ctx.errors.push(if let_else {
                let_else_unreachable(arms, index, disc_ty, ctx)
            } else if saw_wildcard {
                CompileError::new(
                    "bynk.types.unreachable_arm",
                    arm.span,
                    "this match arm is unreachable because a wildcard arm precedes it",
                )
            } else {
                CompileError::new(
                    "bynk.types.unreachable_arm",
                    arm.span,
                    if arm.pattern.is_irrefutable() {
                        "this match arm is unreachable — the earlier arms already match every value"
                            .to_string()
                    } else {
                        format!(
                            "this match arm is unreachable — every value `{}` matches is already matched by an earlier arm",
                            describe_pattern(&arm.pattern)
                        )
                    },
                )
                .with_note("remove the arm, or move it above the arm that covers it")
            });
        }
        if arm.guard.is_none() {
            covering.push(&arm.pattern);
        }
        // Guard (ADR 0169): must be `Bool`; a guarded arm never covers.
        if let Some(guard) = &arm.guard
            && let Some(gt) = type_of(guard, Some(tys.intern(Ty::Base(BaseType::Bool))), ctx)
            && !compatible(gt, tys.intern(Ty::Base(BaseType::Bool)), tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.types.guard_not_bool",
                guard.span,
                format!(
                    "a match-arm `if` guard must be `Bool`, but this guard is `{}`",
                    gt.display(tys)
                ),
            ));
        }
        let body_ty = match &arm.body {
            MatchBody::Expr(e) => maybe_auto_lift(type_of(e, expected, ctx), expected, tys),
            MatchBody::Block(b) => type_of_block(b, expected, ctx),
        };
        ctx.pop_scope();
        if let Some(t) = body_ty {
            arm_types.push((t, arm.body.span()));
        }
        // An unguarded irrefutable pattern (`_` or a bare name) is a catch-all.
        if arm.guard.is_none() && arm.pattern.is_irrefutable() {
            saw_wildcard = true;
        }
    }
    // #2100 slice 3: each missing witness carries the arm that covers it, as
    // a `HasPlaceholders` suggestion (never a one-click fix, never taken by
    // fix-all) whose body is a typed default of the arm result type. The
    // language server's fill-match-arms action collects them. No default for
    // the result type (or a `let … else match`, or a witness that isn't a
    // writable pattern) means no suggestion, and so no action.
    let fill_body = if let_else {
        None
    } else {
        fill_arm_default(expected, arm_types.first().map(|(t, _)| *t), tys)
    };
    let fill_anchor = arms.last().map(|a| a.span.end);
    let with_fill = |err: CompileError, witness: &str| -> CompileError {
        match (&fill_body, fill_anchor) {
            (Some(body), Some(at)) if !witness.contains('…') => err.with_suggestion(
                format!("add a match arm for `{witness}`"),
                vec![(Span::new(at, at), format!("\n{witness} => {body}"))],
                Applicability::HasPlaceholders,
            ),
            _ => err,
        }
    };
    // Exhaustiveness (ADR 0169: bounded structural coverage; guarded arms are
    // excluded because a guard may fail at runtime).
    if !saw_wildcard {
        let unguarded: Vec<&Pattern> = arms
            .iter()
            .filter(|a| a.guard.is_none())
            .map(|a| &a.pattern)
            .collect();
        match &kind {
            MatchKind::Variant => {
                for witness in missing_patterns(disc_ty, &unguarded, ctx) {
                    if let Some((name, shown)) = future_variant_witness(&witness) {
                        // #2100 slice 3: the wildcard arm it asks for is
                        // fillable like any other missing arm.
                        ctx.errors.push(with_fill(
                            CompileError::new(
                                "bynk.types.extensible_match_needs_wildcard",
                                span,
                                format!(
                                    "`{name}` may gain variants in a minor release, so this \
                                     `match` must cover them — `{shown}` is not covered"
                                ),
                            )
                            .with_note(format!(
                                "add a wildcard arm for `{shown}`: listing every variant `{name}` has today is not enough"
                            )),
                            &shown,
                        ));
                        continue;
                    }
                    if let_else {
                        ctx.errors.push(
                            CompileError::new(
                                "bynk.types.non_exhaustive_match",
                                let_else_arms_span(arms).unwrap_or(span),
                                format!(
                                    "this `else match` does not cover `{}` — the `let` pattern does not match it either",
                                    witness
                                ),
                            )
                            .with_note("add an arm for it to the `else match`, or end it with a wildcard `_` arm"),
                        );
                        continue;
                    }
                    ctx.errors.push(with_fill(
                        CompileError::new(
                            "bynk.types.non_exhaustive_match",
                            span,
                            format!(
                                "non-exhaustive `match` — variant `{}` of `{}` is not covered",
                                witness,
                                disc_ty.display(tys)
                            ),
                        )
                        .with_note("add a match arm for this variant, or use a wildcard `_` arm"),
                        &witness,
                    ));
                }
            }
            // v0.130 (DECISION B): `Bool` is exhausted by `true` + `false`;
            // `Int`/`String` are unbounded and always need a wildcard.
            MatchKind::Literal(BaseType::Bool) => {
                for witness in missing_patterns(disc_ty, &unguarded, ctx) {
                    ctx.errors.push(with_fill(
                        CompileError::new(
                            "bynk.types.non_exhaustive_match",
                            span,
                            format!(
                                "non-exhaustive `match` on `Bool` — `{witness}` is not covered"
                            ),
                        )
                        .with_note("add the missing arm, or use a wildcard `_` arm"),
                        &witness,
                    ));
                }
            }
            MatchKind::Literal(_) => {
                ctx.errors.push(with_fill(
                    CompileError::new(
                        "bynk.types.non_exhaustive_match",
                        span,
                        format!(
                            "non-exhaustive `match` on `{}` — a literal match over an unbounded type can never be complete",
                            disc_ty.display(tys)
                        ),
                    )
                    .with_note("add a wildcard `_` arm to cover the remaining values"),
                    "_",
                ));
            }
        }
    }
    // All arm bodies must agree.
    if arm_types.is_empty() {
        return None;
    }
    let mut acc = arm_types[0].0;
    for (t, span) in arm_types.iter().skip(1) {
        match join_ty(acc, *t, tys) {
            Some(joined) => acc = joined,
            None if let_else => {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.match_arm_mismatch",
                        *span,
                        format!(
                            "this `else` produces `{}`, but the rest of the block after the `let` produces `{}`",
                            t.display(tys),
                            acc.display(tys)
                        ),
                    )
                    .with_note(
                        "the `else` stands in for the rest of its block when the pattern does not match, so both must have the same type",
                    ),
                );
                return None;
            }
            None => {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.match_arm_mismatch",
                        *span,
                        format!(
                            "match-arm body has type `{}`, but earlier arms have type `{}`",
                            t.display(tys),
                            acc.display(tys)
                        ),
                    )
                    .with_note(
                    "every arm of a `match` must join to a common type — a refined type and its base agree at the base",
                ),
                );
                return None;
            }
        }
    }
    Some(acc)
}

/// #2010: the unreachable-arm report for a `match` desugared from
/// `let P <- e else …`. Arm 0 is the pattern's; a later arm is the `else`'s.
fn let_else_unreachable(arms: &[MatchArm], index: usize, disc_ty: TyId, ctx: &Ctx) -> CompileError {
    let pattern = describe_pattern(&arms[0].pattern);
    let arm = &arms[index];
    if arms.len() == 2 && matches!(arm.pattern, Pattern::Wildcard(_)) {
        CompileError::new(
            "bynk.types.unreachable_arm",
            arm.span,
            format!(
                "this `else` can never run — `{pattern}` matches every `{}`",
                disc_ty.display(ctx.tys)
            ),
        )
        .with_note(
            "bind the value with a plain `let` instead, or check the pattern is the one you meant",
        )
    } else {
        CompileError::new(
            "bynk.types.unreachable_arm",
            arm.span,
            format!(
                "this `else match` arm is unreachable — every value `{}` matches is already matched by the `let` pattern `{pattern}` or by an earlier arm",
                describe_pattern(&arm.pattern)
            ),
        )
        .with_note("remove the arm: the `let` pattern runs the rest of the block for these values")
    }
}

/// #2010: hints and locals for the names a `let … else` pattern binds, which
/// [`check_pattern`] has just put in scope with their types.
fn record_let_else_bindings(arm: &MatchArm, ctx: &mut Ctx) {
    let tys = ctx.tys;
    for id in arm.pattern.bound_names() {
        let Some(ty) = ctx.lookup(&id.name) else {
            continue;
        };
        let shown = ty.display(tys);
        ctx.hints.record(id.span, format!(": {shown}"));
        ctx.locals.record(
            id.name.clone(),
            id.span,
            crate::locals::LocalKind::Let,
            shown,
            arm.body.span(),
        );
    }
}

/// #2010: the span of a desugared `let … else`'s own arms (all but the
/// pattern's, which holds the rest of the block).
fn let_else_arms_span(arms: &[MatchArm]) -> Option<Span> {
    let first = arms.get(1)?;
    Some(
        arms[1..]
            .iter()
            .fold(first.span, |acc, a| acc.merge(a.span)),
    )
}

/// Validate `pat` against scrutinee type `ty`, binding names into the current
/// scope and emitting diagnostics. Recurses through nested payload patterns
/// (ADR 0169). Duplicate-arm and exhaustiveness are computed separately over
/// the whole arm set; this is per-pattern validation + binding only.
/// #1663: bind every name `pat` introduces to the error type. A pattern that
/// fails to check (wrong arity, an unknown variant or field, a non-variant
/// scrutinee) is already reported; without its bindings, each use in the arm
/// would echo as `bynk.resolve.unknown_name`. The error type is absorbing, so
/// those uses raise nothing further.
fn bind_as_error(pat: &Pattern, ctx: &mut Ctx) {
    let error = ctx.tys.intern(Ty::Error);
    for id in pat.bound_names() {
        ctx.bind(id.name.clone(), error);
    }
}

fn check_pattern(pat: &Pattern, ty: TyId, ctx: &mut Ctx) {
    let tys = ctx.tys;
    match pat {
        Pattern::Wildcard(_) => {}
        Pattern::Binding(id) => {
            // Record the binding's resolved type at its ident span so later
            // span-keyed passes can read it. The linearity pass (v0.193) relies
            // on this to learn whether an arm binding names a held resource
            // (`Some(conn)` over `Option[Connection]`) — patterns are not
            // expressions, so `Ident` has no `ExprId` and this is deliberately
            // `pattern_binding_types`, not `expr_types` (T3.4; see the field's
            // own doc comment on `Ctx`).
            ctx.pattern_binding_types.insert(id.span, ty);
            ctx.bind(id.name.clone(), ty);
        }
        Pattern::Literal { value, span } => {
            if let Some(base) = literal_base_of(ty, tys) {
                let lit_base = literal_value_base(value);
                if lit_base != base {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.pattern_type_mismatch",
                        *span,
                        format!(
                            "literal pattern `{}` has type `{}`, but the scrutinee is `{}`",
                            value.describe(),
                            lit_base.name(),
                            ty.display(tys)
                        ),
                    ));
                }
            } else {
                ctx.errors.push(CompileError::new(
                    "bynk.types.pattern_type_mismatch",
                    *span,
                    format!(
                        "literal pattern `{}` cannot match a value of type `{}` — literal patterns match `Int`/`String`/`Bool` scrutinees",
                        value.describe(),
                        ty.display(tys)
                    ),
                ));
            }
        }
        // #472: `p where predicate` — check the inner pattern as usual, then
        // type the predicate against the scrutinee's literal-kind base
        // (reusing `check_refinement`, the same per-predicate
        // base-compatibility check a `type X = Base where P` declaration
        // uses). No narrowing: the refinement is a runtime guard only.
        Pattern::Refined {
            inner,
            predicate,
            span,
        } => {
            check_pattern(inner, ty, ctx);
            if let Some(base) = literal_base_of(ty, tys) {
                check_refinement(base, *span, Some(predicate), ctx.errors);
            } else {
                ctx.errors.push(CompileError::new(
                    "bynk.types.pattern_type_mismatch",
                    *span,
                    format!(
                        "a refined pattern cannot match a value of type `{}` — refined patterns match `Int`/`String` scrutinees",
                        ty.display(tys)
                    ),
                ));
            }
        }
        Pattern::Variant {
            type_name,
            variant,
            bindings,
            span: pat_span,
        } => {
            // A variant pattern is meaningful only over a variant-kind value.
            let Some(expected_variants) = variants_of(ty, &ctx.input.types, tys) else {
                ctx.errors.push(CompileError::new(
                    "bynk.types.pattern_type_mismatch",
                    *pat_span,
                    format!(
                        "variant pattern `{}` cannot match a value of type `{}`",
                        variant.name,
                        ty.display(tys)
                    ),
                ));
                bind_as_error(pat, ctx);
                return;
            };
            // v0.25: a qualified `T.Variant` references `T`, and its qualifier
            // must match the scrutinee type's name.
            if let Some(tn) = type_name {
                if ctx.input.types.contains_key(&tn.name) {
                    ctx.refs.record(tn.span, SymbolKind::Type, &tn.name);
                }
                if let Ty::Named { name, .. } = &*tys.get(ty)
                    && tn.name != *name
                {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.pattern_type_mismatch",
                        tn.span,
                        format!(
                            "pattern qualifier `{}` does not match the discriminant type `{}`",
                            tn.name, name
                        ),
                    ));
                }
            }
            let Some(variant_info) = expected_variants.iter().find(|v| v.name == variant.name)
            else {
                ctx.errors.push(CompileError::new(
                    "bynk.types.unknown_variant_in_pattern",
                    *pat_span,
                    format!(
                        "type `{}` has no variant `{}`",
                        ty.display(tys),
                        variant.name
                    ),
                ));
                bind_as_error(pat, ctx);
                return;
            };
            // Clone the payload so the immutable borrow of `ctx.input` (via
            // `expected_variants`) is released before we recurse with `&mut ctx`.
            let payload: Vec<(String, TyId)> = variant_info.payload.clone();
            if bindings.is_empty() {
                // Nullary pattern (or "don't bind the payload"): matches the tag.
                return;
            }
            if payload.is_empty() {
                ctx.errors.push(CompileError::new(
                    "bynk.types.pattern_arity",
                    *pat_span,
                    format!(
                        "variant `{}` has no payload, but the pattern binds fields",
                        variant.name
                    ),
                ));
                bind_as_error(pat, ctx);
                return;
            }
            let any_named = bindings
                .iter()
                .any(|b| matches!(b.kind, PatternBindingKind::Named { .. }));
            if any_named {
                for b in bindings {
                    match &b.kind {
                        PatternBindingKind::Named { field, pattern } => {
                            let Some(field_ty) = payload
                                .iter()
                                .find(|(n, _)| n == &field.name)
                                .map(|(_, t)| *t)
                            else {
                                ctx.errors.push(CompileError::new(
                                    "bynk.types.unknown_pattern_field",
                                    field.span,
                                    format!(
                                        "variant `{}` has no payload field `{}`",
                                        variant.name, field.name
                                    ),
                                ));
                                bind_as_error(pattern, ctx);
                                continue;
                            };
                            check_pattern(pattern, field_ty, ctx);
                        }
                        PatternBindingKind::Positional { .. } => {
                            ctx.errors.push(CompileError::new(
                                "bynk.types.mixed_pattern_bindings",
                                b.span,
                                "pattern bindings must be all named (`field: name`) or all positional",
                            ));
                            bind_as_error(b.pattern(), ctx);
                        }
                    }
                }
            } else if bindings.len() != payload.len() {
                ctx.errors.push(CompileError::new(
                    "bynk.types.pattern_arity",
                    *pat_span,
                    format!(
                        "variant `{}` has {} payload field(s), but the pattern has {} binding(s)",
                        variant.name,
                        payload.len(),
                        bindings.len()
                    ),
                ));
                bind_as_error(pat, ctx);
            } else {
                for (b, (_, field_ty)) in bindings.iter().zip(payload.iter()) {
                    let field_ty = *field_ty;
                    check_pattern(b.pattern(), field_ty, ctx);
                }
            }
        }
        // #474: an or-pattern. Each alternative is checked against the same
        // `ty` in turn — this reuses every existing per-kind check above
        // (including nested payloads) and populates `ctx.expr_types`/binds
        // names exactly as a plain pattern would. Then verify the two
        // cross-alternative rules by reading back what that just wrote.
        Pattern::Or(alts, _) => {
            for alt in alts {
                check_pattern(alt, ty, ctx);
            }
            check_or_pattern_bindings(alts, ctx);
        }
    }
}

/// #474 Rules 1 & 2: every alternative of an or-pattern must bind the same
/// set of names (Rule 1), and a name shared across alternatives must have the
/// exact same type, including refinement (Rule 2 — unlike `join_ty`, no
/// refined-to-base widening is allowed here; that widening is exactly what
/// Rule 2 exists to reject). Reads types back from `ctx.expr_types`, which
/// `check_pattern`'s `Binding` arm has already populated for every bound
/// identifier in each alternative.
fn check_or_pattern_bindings(alts: &[Pattern], ctx: &mut Ctx) {
    let tys = ctx.tys;
    let mut first_names: Option<Vec<String>> = None;
    let mut first_types: HashMap<String, TyId> = HashMap::new();
    for alt in alts {
        let mut names: Vec<String> = Vec::new();
        for id in alt.bound_names() {
            names.push(id.name.clone());
            let Some(bty) = ctx.pattern_binding_types.get(&id.span).cloned() else {
                continue;
            };
            match first_types.get(&id.name) {
                Some(existing) if existing != &bty => {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.or_pattern_type_mismatch",
                        id.span,
                        format!(
                            "`{}` has type `{}` in this alternative, but `{}` in an earlier alternative of the same or-pattern",
                            id.name,
                            bty.display(tys),
                            existing.display(tys)
                        ),
                    ).with_note("every alternative of an or-pattern must give a shared binding the same type, including refinement"));
                }
                Some(_) => {}
                None => {
                    first_types.insert(id.name.clone(), bty);
                }
            }
        }
        names.sort();
        names.dedup();
        match &first_names {
            None => first_names = Some(names),
            Some(fnames) if fnames != &names => {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.or_pattern_binding_mismatch",
                        alt.span(),
                        "this alternative of an or-pattern binds a different set of names than an earlier alternative",
                    )
                    .with_note("every alternative of an or-pattern must bind exactly the same set of names"),
                );
            }
            Some(_) => {}
        }
    }
}

/// The named field a binding targets (`None` for positional).
fn binding_field(b: &PatternBinding) -> Option<&str> {
    match &b.kind {
        PatternBindingKind::Named { field, .. } => Some(field.name.as_str()),
        PatternBindingKind::Positional { .. } => None,
    }
}

/// #474: the leaf patterns an or-pattern stands for, for callers that reason
/// about coverage/duplication alternative-by-alternative — an `Or` expands to
/// its alternatives (never itself containing a nested `Or`, see
/// [`Pattern::Or`]); anything else is its own single alternative.
fn pattern_alternatives(pat: &Pattern) -> Vec<&Pattern> {
    match pat {
        Pattern::Or(alts, _) => alts.iter().collect(),
        p => vec![p],
    }
}

/// Structural equivalence for duplicate-arm detection: two patterns matching
/// exactly the same set of values. Irrefutable sub-patterns (`_`, name
/// bindings) are all equivalent, and binding *names* do not matter.
fn patterns_equal(a: &Pattern, b: &Pattern) -> bool {
    if a.is_irrefutable() && b.is_irrefutable() {
        return true;
    }
    match (a, b) {
        (Pattern::Literal { value: x, .. }, Pattern::Literal { value: y, .. }) => x == y,
        (
            Pattern::Variant {
                variant: va,
                bindings: ba,
                ..
            },
            Pattern::Variant {
                variant: vb,
                bindings: bb,
                ..
            },
        ) => {
            va.name == vb.name
                && ba.len() == bb.len()
                && ba.iter().zip(bb).all(|(x, y)| {
                    binding_field(x) == binding_field(y) && patterns_equal(x.pattern(), y.pattern())
                })
        }
        _ => false,
    }
}

/// A short human rendering of a pattern for diagnostics (`Err(PollClosed)`,
/// `Ok(_)`, `Pending`, `1`).
fn describe_pattern(pat: &Pattern) -> String {
    match pat {
        Pattern::Wildcard(_) | Pattern::Binding(_) => "_".to_string(),
        Pattern::Literal { value, .. } => value.describe(),
        Pattern::Refined {
            inner, predicate, ..
        } => format!(
            "{} where {}",
            describe_pattern(inner),
            predicate
                .predicates
                .iter()
                .map(|p| p.kind.name())
                .collect::<Vec<_>>()
                .join(" && "),
        ),
        Pattern::Variant {
            variant, bindings, ..
        } => {
            if bindings.is_empty() {
                variant.name.clone()
            } else {
                let inner: Vec<String> = bindings
                    .iter()
                    .map(|b| match &b.kind {
                        PatternBindingKind::Named { field, pattern } => {
                            format!("{}: {}", field.name, describe_pattern(pattern))
                        }
                        PatternBindingKind::Positional { pattern } => describe_pattern(pattern),
                    })
                    .collect();
                format!("{}({})", variant.name, inner.join(", "))
            }
        }
        Pattern::Or(alts, _) => alts
            .iter()
            .map(describe_pattern)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

/// #1656: whether every value `pat` matches is already matched by one of
/// `prior` (the earlier unguarded arms' patterns), i.e. `pat` is not useful
/// after them. Sound but bounded, like [`missing_patterns`]:
///
/// - an or-pattern is covered when each alternative is;
/// - a refined pattern is covered when its inner pattern is (it matches a
///   subset of it), while a refined *prior* never covers — its predicate may
///   fail, exactly as a guard may;
/// - an irrefutable pattern is covered when the priors are exhaustive;
/// - a variant pattern is covered by a prior of the same variant whose payload
///   patterns are all irrefutable, by recursion on a single-field payload, or,
///   for a multi-field payload, by one prior that covers it field by field.
///
/// Anything this cannot decide counts as *not* covered, so an arm is only ever
/// reported when it is certainly unreachable.
fn pattern_covered(prior: &[&Pattern], pat: &Pattern, ty: TyId, ctx: &Ctx) -> bool {
    let prior: Vec<&Pattern> = prior
        .iter()
        .copied()
        .flat_map(pattern_alternatives)
        .collect();
    if prior.iter().any(|p| p.is_irrefutable()) {
        return true;
    }
    match pat {
        Pattern::Or(alts, _) => alts.iter().all(|a| pattern_covered(&prior, a, ty, ctx)),
        Pattern::Refined { inner, .. } => pattern_covered(&prior, inner, ty, ctx),
        Pattern::Wildcard(_) | Pattern::Binding(_) => missing_patterns(ty, &prior, ctx).is_empty(),
        Pattern::Literal { value, .. } => prior
            .iter()
            .any(|p| matches!(p, Pattern::Literal { value: v, .. } if v == value)),
        Pattern::Variant { variant, .. } => {
            let Some(info) = variants_of(ty, &ctx.input.types, ctx.tys)
                .and_then(|vs| vs.into_iter().find(|v| v.name == variant.name))
            else {
                return false;
            };
            let Some(fields) = payload_patterns(pat, &info) else {
                return false;
            };
            let matching: Vec<Vec<Option<&Pattern>>> = prior
                .iter()
                .filter_map(|p| payload_patterns(p, &info))
                .collect();
            // A prior that binds every payload field irrefutably (or binds none)
            // matches the whole variant.
            if matching
                .iter()
                .any(|m| m.iter().all(|f| f.is_none_or(Pattern::is_irrefutable)))
            {
                return true;
            }
            if info.payload.len() == 1 {
                let sub: Vec<&Pattern> = matching.iter().filter_map(|m| m[0]).collect();
                return fields[0].is_some_and(|q| pattern_covered(&sub, q, info.payload[0].1, ctx));
            }
            matching.iter().any(|m| {
                m.iter()
                    .zip(&fields)
                    .zip(&info.payload)
                    .all(|((p, q), (_, fty))| match (p, q) {
                        (None, _) => true,
                        (Some(p), Some(q)) => pattern_covered(&[p], q, *fty, ctx),
                        (Some(p), None) => p.is_irrefutable(),
                    })
            })
        }
    }
}

/// #1656: a variant pattern's payload sub-patterns in declared field order —
/// `None` for a field the pattern leaves unbound (a nullary-form pattern, or a
/// named pattern omitting it), which matches anything. `None` overall when
/// `pat` is not a well-formed pattern for `info`'s variant.
fn payload_patterns<'p>(pat: &'p Pattern, info: &VariantInfo) -> Option<Vec<Option<&'p Pattern>>> {
    let Pattern::Variant {
        variant, bindings, ..
    } = pat
    else {
        return None;
    };
    if variant.name != info.name {
        return None;
    }
    let mut fields = vec![None; info.payload.len()];
    if bindings.is_empty() {
        return Some(fields);
    }
    if bindings
        .iter()
        .any(|b| matches!(b.kind, PatternBindingKind::Named { .. }))
    {
        for b in bindings {
            let PatternBindingKind::Named { field, pattern } = &b.kind else {
                return None;
            };
            let i = info.payload.iter().position(|(n, _)| n == &field.name)?;
            fields[i] = Some(pattern);
        }
    } else {
        if bindings.len() != fields.len() {
            return None;
        }
        for (slot, b) in fields.iter_mut().zip(bindings) {
            *slot = Some(b.pattern());
        }
    }
    Some(fields)
}

/// Opens and closes the witness [`missing_patterns`] gives for an extensible
/// enum's future variant (standard-library S3, #1963): `\u{1}FetchError\u{2}`.
/// Control characters, so no real variant or pattern text can collide with it.
const FUTURE_VARIANT_OPEN: char = '\u{1}';
const FUTURE_VARIANT_CLOSE: char = '\u{2}';

/// The extensible enum a first-party unit exports that `ty` is, if any. Types
/// carry no owning unit, so whether a name is one comes from
/// [`crate::resolver::ResolvedCommons::extensible_enums`], decided once per unit
/// from its provenance. A user's own enum of the same name is never imported
/// from a first-party unit (the two would conflict, `bynk.consumes.name_conflict`).
fn extensible_enum(ty: TyId, ctx: &Ctx) -> Option<String> {
    match &*ctx.tys.get(ty) {
        Ty::Named {
            kind: NamedKind::Sum,
            name,
            ..
        } => ctx
            .input
            .extensible_enums
            .contains(name)
            .then(|| name.clone()),
        _ => None,
    }
}

/// If `witness` stands for an extensible enum's future variant, at any depth
/// (`Err(<future>)`), the enum's name and the witness as the user writes it,
/// with the future variant shown as `_`.
fn future_variant_witness(witness: &str) -> Option<(String, String)> {
    let start = witness.find(FUTURE_VARIANT_OPEN)?;
    let len = witness[start..].find(FUTURE_VARIANT_CLOSE)?;
    let name = witness[start + 1..start + len].to_string();
    let shown = format!(
        "{}_{}",
        &witness[..start],
        &witness[start + len + FUTURE_VARIANT_CLOSE.len_utf8()..]
    );
    Some((name, shown))
}

/// #2100 slice 3: a body for a filled-in match arm that is sure to type-check
/// as the match's result: the unrefined built-in scalars, `()`, `None` for an
/// `Option` and `[]` for a `List` (the #852 `field_default_init` set, plus
/// unit), or one of those for `T` when the match is expected to be an
/// `Effect[T]` (an arm body auto-lifts). `expected` is the match's expected
/// type, `first_arm` the first arm body's type when nothing is expected. A
/// user-named type, refined or not, has none.
fn fill_arm_default(
    expected: Option<TyId>,
    first_arm: Option<TyId>,
    tys: &Types,
) -> Option<String> {
    fn plain(ty: TyId, tys: &Types) -> Option<&'static str> {
        Some(match &*tys.get(ty) {
            Ty::Base(BaseType::Int) => "0",
            Ty::Base(BaseType::Float) => "0.0",
            Ty::Base(BaseType::String) => "\"\"",
            Ty::Base(BaseType::Bool) => "false",
            Ty::Unit => "()",
            Ty::Option(_) => "None",
            Ty::List(_) => "[]",
            _ => return None,
        })
    }
    let body = match expected {
        Some(exp) => match &*tys.get(exp) {
            Ty::Effect(inner) => plain(*inner, tys),
            _ => plain(exp, tys),
        },
        None => plain(first_arm?, tys),
    };
    body.map(str::to_string)
}

/// Value shapes of `ty` NOT covered by the sibling patterns `pats` (empty ⇒
/// exhaustive). An extensible enum (S3, #1963) has one more shape than its
/// declared variants, a future variant, which only an irrefutable pattern
/// covers. Bounded by declared sum arity (ADR 0169 DECISION D): a
/// single-field payload recurses; a multi-field refutable payload is
/// conservatively reported uncovered unless a full (all-irrefutable) arm exists.
fn missing_patterns(ty: TyId, pats: &[&Pattern], ctx: &Ctx) -> Vec<String> {
    let tys = ctx.tys;
    // #474: an or-pattern covers all its alternatives — flatten one level
    // before computing coverage, so `Held(...) | Confirmed(...)` contributes
    // as two separate witnesses. Applied here (rather than only at the
    // top-level `check_match` call site) so the recursive calls below, over a
    // matching variant's single-field payload, also see or-aware coverage for
    // a nested `Some(1 | 2)`.
    let flat: Vec<&Pattern> = pats
        .iter()
        .copied()
        .flat_map(pattern_alternatives)
        .collect();
    let pats = &flat[..];
    if pats.iter().any(|p| p.is_irrefutable()) {
        return Vec::new();
    }
    if let Some(variants) = variants_of(ty, &ctx.input.types, tys) {
        let mut missing = Vec::new();
        for v in &variants {
            let matching: Vec<&Pattern> = pats
                .iter()
                .copied()
                .filter(|p| matches!(p, Pattern::Variant { variant, .. } if variant.name == v.name))
                .collect();
            if matching.is_empty() {
                // One `_` per payload field, so the witness is a pattern that
                // could be written as an arm (#2100).
                missing.push(if v.payload.is_empty() {
                    v.name.clone()
                } else {
                    format!("{}({})", v.name, vec!["_"; v.payload.len()].join(", "))
                });
                continue;
            }
            if v.payload.is_empty() {
                continue;
            }
            let has_full = matching.iter().any(|p| {
                matches!(p, Pattern::Variant { bindings, .. }
                    if bindings.is_empty() || bindings.iter().all(|b| b.pattern().is_irrefutable()))
            });
            if has_full {
                continue;
            }
            if v.payload.len() == 1 {
                let field_ty = &v.payload[0].1;
                let sub: Vec<&Pattern> = matching
                    .iter()
                    .filter_map(|p| match p {
                        Pattern::Variant { bindings, .. } if bindings.len() == 1 => {
                            Some(bindings[0].pattern())
                        }
                        _ => None,
                    })
                    .collect();
                for inner in missing_patterns(*field_ty, &sub, ctx) {
                    missing.push(format!("{}({})", v.name, inner));
                }
                continue;
            }
            // Multi-field refutable nesting: needs a full arm (conservative).
            missing.push(format!("{}(…)", v.name));
        }
        if let Some(name) = extensible_enum(ty, ctx) {
            missing.push(format!("{FUTURE_VARIANT_OPEN}{name}{FUTURE_VARIANT_CLOSE}"));
        }
        return missing;
    }
    if matches!(literal_base_of(ty, tys), Some(BaseType::Bool)) {
        let mut missing = Vec::new();
        for b in [true, false] {
            if !pats.iter().any(
                |p| matches!(p, Pattern::Literal { value: LiteralValue::Bool(x), .. } if *x == b),
            ) {
                missing.push(b.to_string());
            }
        }
        return missing;
    }
    // Unbounded (`Int`/`String`): only an irrefutable pattern covers.
    vec!["_".to_string()]
}

pub(crate) fn check_is(
    value: &Expr,
    pattern: &Pattern,
    _span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let value_ty = type_of(value, None, ctx)?;
    let variants = variants_of(value_ty, &ctx.input.types, tys);
    check_is_pattern(pattern, value_ty, &variants, ctx);
    Some(tys.intern(Ty::Base(BaseType::Bool)))
}

/// The per-pattern-kind body of `check_is` (#474: factored out so an
/// or-pattern's alternatives — each itself one of these kinds — can be
/// validated by recursion). Always tests against the same `value_ty`/
/// `variants`, computed once by the caller.
fn check_is_pattern(
    pattern: &Pattern,
    value_ty: TyId,
    variants: &Option<Vec<VariantInfo>>,
    ctx: &mut Ctx,
) {
    let tys = ctx.tys;
    match pattern {
        Pattern::Wildcard(_) => {
            // `_` matches anything, but is only meaningful over a sum today.
            if variants.is_none() {
                ctx.errors.push(CompileError::new(
                    "bynk.types.is_non_sum",
                    pattern.span(),
                    format!(
                        "the `is` operator requires a sum, `Result`, or `Option` value, but got `{}`",
                        value_ty.display(tys)
                    ),
                ));
            }
        }
        // A bare name binding after `is` is match-only surface (ADR 0169); over a
        // sum it matches anything like `_`, and it introduces no useful narrowing.
        Pattern::Binding(_) => {
            if variants.is_none() {
                ctx.errors.push(CompileError::new(
                    "bynk.types.is_non_sum",
                    pattern.span(),
                    format!(
                        "the `is` operator requires a sum, `Result`, or `Option` value, but got `{}`",
                        value_ty.display(tys)
                    ),
                ));
            }
        }
        // v0.130 (DECISION F): literal patterns are match-only; `x is 31` reads
        // as a type test but would mean value equality — steer to `==`.
        Pattern::Literal { value, .. } => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.types.is_literal_pattern",
                    pattern.span(),
                    "the `is` operator tests a value's type or refinement, not equality to a literal",
                )
                .with_note(format!(
                    "to compare by value, use `== {}` instead",
                    value.describe()
                )),
            );
        }
        // #472 (D5, mirrors literal patterns): refined patterns are
        // `match`-only. `is` already has its own refinement check — a bare
        // nullary name resolving to a declared refined type (ADR 0007) —
        // so an inline `_ where P` here would be a confusing near-duplicate.
        // The parser (`parse_eq`) already rejects a *top-level* refined
        // pattern after `is` at parse time (the tree-sitter grammar cannot
        // admit `refined_pattern` inside `is_expr` at all — D4), so this arm
        // is unreachable for any program that reaches the checker; kept for
        // match exhaustiveness and as a defensive second line.
        Pattern::Refined { .. } => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.types.is_refined_pattern",
                    pattern.span(),
                    "the `is` operator does not accept a refined pattern here",
                )
                .with_note(
                    "declare a named refined type and use `x is TypeName`, or use a `match` arm",
                ),
            );
        }
        Pattern::Variant {
            variant,
            bindings,
            type_name,
            ..
        } => {
            // v0.25: a qualified `T.Variant` pattern references `T`.
            if let Some(tn) = type_name
                && ctx.input.types.contains_key(&tn.name)
            {
                ctx.refs.record(tn.span, SymbolKind::Type, &tn.name);
            }
            // 1. Sum-variant interpretation: the name is a variant of `value`'s
            //    sum type. (Takes priority when `value` is that sum.)
            let info = variants
                .as_ref()
                .and_then(|vs| vs.iter().find(|v| v.name == variant.name));
            let Some(info) = info else {
                // 2. v0.13 refinement narrowing: a bare nullary name that
                //    resolves to a refined type whose base matches `value`.
                if type_name.is_none()
                    && bindings.is_empty()
                    && let Some(decl) = ctx.input.types.get(&variant.name)
                    && let TypeBody::Refined { base, .. } = &decl.body
                {
                    if compatible(value_ty, tys.intern(Ty::Base(*base)), tys) {
                        // v0.25: `x is RefinedType` names the type.
                        ctx.refs
                            .record(variant.span, SymbolKind::Type, &variant.name);
                        return;
                    }
                    ctx.errors.push(CompileError::new(
                        "bynk.types.is_base_mismatch",
                        pattern.span(),
                        format!(
                            "`is {}` checks an `{}` value, but got `{}`",
                            variant.name,
                            base.name(),
                            value_ty.display(tys)
                        ),
                    ));
                    return;
                }
                // 3. Neither a variant nor a base-compatible refined type.
                if variants.is_none() {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.is_non_sum",
                        pattern.span(),
                        format!(
                            "the `is` operator requires a sum, `Result`, or `Option` value, but got `{}`",
                            value_ty.display(tys)
                        ),
                    ));
                } else {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.is_unknown_variant",
                        variant.span,
                        format!(
                            "type `{}` has no variant `{}`",
                            value_ty.display(tys),
                            variant.name
                        ),
                    ));
                }
                return;
            };
            // Just validate bindings shape; binding TYPES introduced via
            // `collect_is_bindings` are handled at the consumer site.
            if !bindings.is_empty() && info.payload.is_empty() {
                ctx.errors.push(CompileError::new(
                    "bynk.types.pattern_arity",
                    pattern.span(),
                    format!(
                        "variant `{}` has no payload, but the pattern binds fields",
                        variant.name
                    ),
                ));
            } else if !bindings.is_empty() {
                let any_named = bindings
                    .iter()
                    .any(|b| matches!(b.kind, PatternBindingKind::Named { .. }));
                if !any_named && bindings.len() != info.payload.len() {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.pattern_arity",
                        pattern.span(),
                        format!(
                            "variant `{}` has {} payload field(s), but the pattern has {} binding(s)",
                            variant.name,
                            info.payload.len(),
                            bindings.len()
                        ),
                    ));
                }
            }
            // #710 review: `is` now *tests* nested refutable payload patterns
            // (they lower like a `match` arm), so it must *check* them too — a
            // mistyped nested variant over a typed scrutinee is a compile error,
            // not a silently never-matching branch. Bindings/wildcards are inert
            // (narrowing only). The loose scrutinee has no `variants` and never
            // reaches here, so this validates exactly the typed path.
            let payload = info.payload.clone();
            validate_is_nested_payloads(bindings, &payload, ctx);
        }
        // #474 §2.3.6: an or-pattern is legal after `is` (unlike a literal —
        // each alternative is checked by the same D5 rejection above if it is
        // one). Validate every alternative, then the same Rules 1/2 as a
        // `match` arm, via `gather_pattern_bindings` — the exact `(name, Ty)`
        // set an alternative would contribute as `is`-narrowing bindings.
        Pattern::Or(alts, _) => {
            for alt in alts {
                check_is_pattern(alt, value_ty, variants, ctx);
            }
            let mut first: Option<Vec<(String, TyId)>> = None;
            for alt in alts {
                let mut out = Vec::new();
                gather_pattern_bindings(value_ty, alt, &ctx.input.types, &mut out, tys);
                out.sort_by(|a, b| a.0.cmp(&b.0));
                match &first {
                    None => first = Some(out),
                    Some(f) => {
                        let fnames: Vec<&String> = f.iter().map(|(n, _)| n).collect();
                        let names: Vec<&String> = out.iter().map(|(n, _)| n).collect();
                        if fnames != names {
                            ctx.errors.push(
                                CompileError::new(
                                    "bynk.types.or_pattern_binding_mismatch",
                                    alt.span(),
                                    "this alternative of an or-pattern binds a different set of names than an earlier alternative",
                                )
                                .with_note("every alternative of an or-pattern must bind exactly the same set of names"),
                            );
                        }
                        for (n, t) in &out {
                            if let Some((_, ft)) = f.iter().find(|(fname, _)| fname == n)
                                && ft != t
                            {
                                ctx.errors.push(
                                    CompileError::new(
                                        "bynk.types.or_pattern_type_mismatch",
                                        alt.span(),
                                        format!(
                                            "`{n}` has type `{}` in this alternative, but `{}` in an earlier alternative of the same or-pattern",
                                            t.display(tys),
                                            ft.display(tys)
                                        ),
                                    )
                                    .with_note("every alternative of an or-pattern must give a shared binding the same type, including refinement"),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}

/// #710 review: validate a nested (`is`) payload pattern against its statically
/// known type — variant existence and, recursively, its own payloads. Mirrors
/// `check_pattern`'s variant checks but **binds nothing** (a nested `is` payload
/// only tests; depth-1 narrowing names are collected separately). Non-variant
/// leaves (wildcard / name binding / literal) are inert here.
fn validate_is_pattern(pattern: &Pattern, ty: TyId, ctx: &mut Ctx) {
    let tys = ctx.tys;
    let Pattern::Variant {
        variant, bindings, ..
    } = pattern
    else {
        return;
    };
    // `variants_of` returns an owned list, so no borrow of `ctx.input` is held
    // across the error pushes / recursion below.
    let payload = match variants_of(ty, &ctx.input.types, tys) {
        None => {
            ctx.errors.push(CompileError::new(
                "bynk.types.pattern_type_mismatch",
                pattern.span(),
                format!(
                    "variant pattern `{}` cannot match a value of type `{}`",
                    variant.name,
                    ty.display(tys)
                ),
            ));
            return;
        }
        Some(vs) => match vs.into_iter().find(|v| v.name == variant.name) {
            Some(info) => info.payload,
            None => {
                ctx.errors.push(CompileError::new(
                    "bynk.types.unknown_variant_in_pattern",
                    pattern.span(),
                    format!(
                        "type `{}` has no variant `{}`",
                        ty.display(tys),
                        variant.name
                    ),
                ));
                return;
            }
        },
    };
    validate_is_nested_payloads(bindings, &payload, ctx);
}

/// Recurse into a variant pattern's refutable payload bindings, validating each
/// against the payload field's type (#710 review). Irrefutable bindings
/// (wildcard / name) add no test and are skipped.
fn validate_is_nested_payloads(
    bindings: &[PatternBinding],
    payload: &[(String, TyId)],
    ctx: &mut Ctx,
) {
    for (i, b) in bindings.iter().enumerate() {
        let sub = b.pattern();
        if sub.is_irrefutable() {
            continue;
        }
        let field_ty = match &b.kind {
            PatternBindingKind::Named { field, .. } => payload
                .iter()
                .find(|(n, _)| n == &field.name)
                .map(|(_, t)| t),
            PatternBindingKind::Positional { .. } => payload.get(i).map(|(_, t)| t),
        };
        if let Some(ty) = field_ty.cloned() {
            validate_is_pattern(sub, ty, ctx);
        }
    }
}

/// The bindings the `is` tests in a condition introduce when it is true: the
/// tests [`crate::narrowing::matched_is_tests`] proves matched (#1654, the rule
/// the resolver and the emitter share). Later wins on a name collision.
fn collect_is_bindings(expr: &Expr, ctx: &mut Ctx) -> Vec<(String, TyId)> {
    collect_is_bindings_when(expr, true, ctx)
}

/// [`collect_is_bindings`] for either outcome of the condition: `when_true ==
/// false` is the else-branch's scope (`if !(o is Some(v)) { … } else { v }`).
pub(crate) fn collect_is_bindings_when(
    expr: &Expr,
    when_true: bool,
    ctx: &mut Ctx,
) -> Vec<(String, TyId)> {
    // Memoised per (expression, outcome) in `Ctx::is_binding_cache`, at two
    // grains (review of #1695):
    // - each matched `is` test's own bindings, under `(test, true)`. A
    //   left-nested `&&` chain `t1 && … && tN` asks for each lhs subtree once
    //   per enclosing node, so each test is reached O(N) times, but its pattern
    //   walk over `expr_types` (the real work) runs once;
    // - the whole list for the asked `(expr, outcome)`, so a repeated ask (the
    //   two `check_if` calls, a re-visit) returns at once.
    // The structural walk `matched_is_tests` itself is not memoised: over a
    // chain it is O(N²) node visits in total, pointer-chasing only, which keeps
    // the scoping rule in one place (`crate::narrowing`) rather than restating
    // its table here. The collector is a pure read of `expr_types` (populated
    // before it runs) and `ctx.input.types`, so the cache is sound.
    if let Some(cached) = ctx.is_binding_cache.get(&(expr.id, when_true)) {
        return cached.clone();
    }
    let mut out = Vec::new();
    for test in crate::narrowing::matched_is_tests(expr, when_true) {
        // A test's own bindings are the same as asking for the `is` node itself
        // when true, so they share that key.
        if let Some(cached) = ctx.is_binding_cache.get(&(test.id, true)) {
            out.extend(cached.iter().cloned());
            continue;
        }
        let mut own = Vec::new();
        bindings_of_is_test(test, ctx, &mut own);
        ctx.is_binding_cache.insert((test.id, true), own.clone());
        out.extend(own);
    }
    ctx.is_binding_cache
        .insert((expr.id, when_true), out.clone());
    out
}

/// The bindings one `is` test introduces when it has matched.
fn bindings_of_is_test(expr: &Expr, ctx: &mut Ctx, out: &mut Vec<(String, TyId)>) {
    let tys = ctx.tys;
    let ExprKind::Is { value, pattern } = &expr.kind else {
        return;
    };
    // Recompute value type from the expr_types side-table; this avoids
    // mutating type-checking state.
    let Some(value_ty) = ctx.expr_types.get(&value.id).map(|te| te.ty) else {
        return;
    };
    // v0.13 refinement narrowing: `ident is RefinedType` re-binds the
    // identifier to the refined type in the narrowed branch.
    if let (
        ExprKind::Ident(id),
        Pattern::Variant {
            variant,
            bindings,
            type_name: None,
            ..
        },
    ) = (&value.kind, pattern.as_ref())
        && bindings.is_empty()
        && variants_of(value_ty, &ctx.input.types, tys)
            .is_none_or(|vs| !vs.iter().any(|v| v.name == variant.name))
        && let Some(decl) = ctx.input.types.get(&variant.name)
        && let TypeBody::Refined { base, .. } = &decl.body
        && compatible(value_ty, tys.intern(Ty::Base(*base)), tys)
    {
        out.push((
            id.name.clone(),
            tys.intern(Ty::Named {
                name: variant.name.clone(),
                kind: NamedKind::Refined(*base),
                args: Vec::new(),
            }),
        ));
        return;
    }
    gather_pattern_bindings(value_ty, pattern, &ctx.input.types, out, tys);
}

fn gather_pattern_bindings(
    value_ty: TyId,
    pattern: &Pattern,
    types: &HashMap<String, Arc<TypeDecl>>,
    out: &mut Vec<(String, TyId)>,
    tys: &Types,
) {
    // #474: when Rule 2 holds, every alternative gives a shared name the same
    // type, so the first alternative's mapping is representative — recurse
    // into it alone rather than merging all alternatives. This is safe even
    // when Rule 2 does *not* hold: `check_is_pattern`'s own `Or` arm is what
    // detects and reports that mismatch (by calling this same function once
    // per alternative and comparing), so by the time this call is reached the
    // violation is already a separate diagnostic — sampling one alternative
    // here is best-effort narrowing for error recovery, not a load-bearing
    // invariant. (Do not add a `debug_assert` that all alternatives agree:
    // the negative fixtures deliberately exercise the disagreeing case.)
    if let Pattern::Or(alts, _) = pattern {
        if let Some(first) = alts.first() {
            gather_pattern_bindings(value_ty, first, types, out, tys);
        }
        return;
    }
    let Pattern::Variant {
        variant, bindings, ..
    } = pattern
    else {
        return;
    };
    // #1663: on a pattern error (already reported by `check_is_pattern`), the
    // names the pattern would have bound are still bound — to the error type,
    // so their uses neither echo as unknown names nor raise further errors.
    let error = tys.intern(Ty::Error);
    let bind_error = |p: &Pattern, out: &mut Vec<(String, TyId)>| {
        out.extend(p.bound_names().into_iter().map(|n| (n.name.clone(), error)));
    };
    let Some(variants) = variants_of(value_ty, types, tys) else {
        bind_error(pattern, out);
        return;
    };
    let Some(info) = variants.iter().find(|v| v.name == variant.name) else {
        bind_error(pattern, out);
        return;
    };
    let any_named = bindings
        .iter()
        .any(|b| matches!(b.kind, PatternBindingKind::Named { .. }));
    if any_named {
        let payload_map: HashMap<&str, TyId> =
            info.payload.iter().map(|(n, t)| (n.as_str(), *t)).collect();
        for b in bindings {
            if let PatternBindingKind::Named { field, pattern } = &b.kind
                && let Pattern::Binding(name) = pattern
            {
                match payload_map.get(field.name.as_str()) {
                    Some(ty) => out.push((name.name.clone(), *ty)),
                    None => out.push((name.name.clone(), error)),
                }
            }
        }
    } else {
        for (b, (_, ty)) in bindings.iter().zip(info.payload.iter()) {
            if let Pattern::Binding(name) = b.pattern() {
                out.push((name.name.clone(), *ty));
            }
        }
        for b in bindings.iter().skip(info.payload.len()) {
            bind_error(b.pattern(), out);
        }
    }
}

#[cfg(test)]
mod join_ty_tests {
    use super::{BaseType, NamedKind, Ty, TyId, Types, join_ty};

    fn refined(tys: &Types, name: &str, base: BaseType) -> TyId {
        tys.intern(Ty::Named {
            name: name.to_string(),
            kind: NamedKind::Refined(base),
            args: vec![],
        })
    }
    fn base(tys: &Types, b: BaseType) -> TyId {
        tys.intern(Ty::Base(b))
    }

    /// #726: a refined type and its base join to the base, in either order
    /// (the reported `match r { Ok(e) => e, Err(m) => m }` shape).
    #[test]
    fn refined_and_its_base_widen_to_base() {
        let tys = &Types::new();
        let email = refined(tys, "Email", BaseType::String);
        let string = base(tys, BaseType::String);
        assert_eq!(join_ty(email, string, tys), Some(string));
        assert_eq!(join_ty(string, email, tys), Some(string));
    }

    /// Two distinct refined types over the same base join to that base,
    /// regardless of fold order.
    #[test]
    fn two_refined_over_same_base_join_to_base() {
        let tys = &Types::new();
        let email = refined(tys, "Email", BaseType::String);
        let user = refined(tys, "Username", BaseType::String);
        assert_eq!(join_ty(email, user, tys), Some(base(tys, BaseType::String)));
        assert_eq!(join_ty(user, email, tys), Some(base(tys, BaseType::String)));
    }

    /// Identical types join to themselves — no widening.
    #[test]
    fn identical_refined_types_are_preserved() {
        let tys = &Types::new();
        let email = refined(tys, "Email", BaseType::String);
        assert_eq!(join_ty(email, email, tys), Some(email));
    }

    /// Genuinely unrelated types have no join (still a branch-mismatch error).
    #[test]
    fn unrelated_types_have_no_join() {
        let tys = &Types::new();
        let string = base(tys, BaseType::String);
        let int = base(tys, BaseType::Int);
        assert!(join_ty(string, int, tys).is_none());
        let email = refined(tys, "Email", BaseType::String);
        assert!(join_ty(email, int, tys).is_none());
    }
}
