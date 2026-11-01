//! Call / application dispatch.
//!
//! Split out of `checker.rs` (v0.29.10) verbatim; the parent module
//! re-exports these via `use calls::*`.

use super::codable::{check_op_codable_args, op_encoding};
use super::*;

/// v0.39 (ADR 0072): record a parameter-name inlay hint for one call argument,
/// unless it would be noise — the `_`/`self` placeholders, or an argument that
/// is already the identically-named identifier (`f(count)` for parameter
/// `count`, matching rust-analyzer's suppression).
fn record_param_hint(hints: &mut HintSink, param_name: &str, arg: &Expr) {
    if param_name == "_" || param_name == "self" {
        return;
    }
    if let ExprKind::Ident(id) = &arg.kind
        && id.name == param_name
    {
        return;
    }
    hints.record_param(arg.span, format!("{param_name}:"));
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn check_fn(
    f: &FnDecl,
    input: &ResolvedCommons,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    callees: &mut HashMap<ExprId, Callee>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    requirements: &mut RequirementSink,
    tys: &Types,
) {
    // v0.20a: the fn's type parameters are *rigid* type variables while
    // checking its own body. A type param shadowing a declared type is
    // confusing — diagnose the collision.
    let mut vars: HashSet<String> = f
        .type_params
        .iter()
        .map(|tp| tp.name.name.clone())
        .collect();
    for tp in &f.type_params {
        if input.types.contains_key(&tp.name.name) {
            errors.push(
                CompileError::new(
                    "bynk.generics.type_arg_mismatch",
                    tp.span,
                    format!(
                        "type parameter `{}` shadows the declared type of the same name",
                        tp.name.name
                    ),
                )
                .with_note("rename the type parameter"),
            );
        }
    }
    // #594: an instance method on a generic type inherits the receiver type's
    // parameters as additional rigid vars, so its signature and body may name
    // them (`self: Box[A]`, `f: A -> U`). The receiver's parameters are the
    // type's own — they never shadow a declared type — so they bypass the guard
    // above.
    if let FnName::Method { type_name, .. } = &f.name
        && let Some(decl) = input.types.get(&type_name.name)
    {
        for tp in &decl.type_params {
            vars.insert(tp.name.name.clone());
        }
    }
    let return_ty = match resolve_type_ref_in(&f.return_type, &input.types, &vars, tys) {
        Some(t) => t,
        None => return,
    };
    record_type_refs(&f.return_type, &input.types, &vars, refs);
    let mut param_scope: HashMap<String, TyId> = HashMap::new();
    // For methods, the implicit `self` parameter has the attached type. #594: on
    // a generic receiver, `self` is the type applied to its own parameters as
    // rigid vars (`Box[A]`), so field access substitutes them and the body's
    // uses of `A` line up with the signature.
    if let FnName::Method { type_name, .. } = &f.name
        && f.has_self
        && let Some(decl) = input.types.get(&type_name.name)
    {
        let self_args = decl
            .type_params
            .iter()
            .map(|tp| tys.intern(Ty::Var(tp.name.name.clone())))
            .collect();
        param_scope.insert("self".to_string(), named_ty_with_args(decl, self_args, tys));
    }
    for p in &f.params {
        if let Some(ty) = resolve_type_ref_in(&p.type_ref, &input.types, &vars, tys) {
            record_type_refs(&p.type_ref, &input.types, &vars, refs);
            // v0.31: a fn parameter is in scope over the whole body.
            if p.name.name != "_" {
                locals.record(
                    p.name.name.clone(),
                    p.name.span,
                    crate::locals::LocalKind::Param,
                    ty.display(tys),
                    f.body.span,
                );
            }
            param_scope.insert(p.name.name.clone(), ty);
        }
    }
    let eq = EqScope::for_fn(f, input);
    // Slice 2 (DECISION F): a type parameter in a `Map` key or `Set` element of
    // the signature is declared `Key`.
    if !vars.is_empty() {
        super::keying::require_key_positions(
            tys,
            &vars,
            &eq,
            errors,
            return_ty,
            f.return_type.span(),
        );
        for p in &f.params {
            if let Some(ty) = param_scope.get(&p.name.name).copied() {
                super::keying::require_key_positions(
                    tys,
                    &vars,
                    &eq,
                    errors,
                    ty,
                    p.type_ref.span(),
                );
            }
        }
    }
    // v0.115: check the function's contract clauses (`requires`/`ensures`) in
    // the parameter scope before the body — a contract is the invariant
    // predicate attached to a function (ADR 0144). `result` in an `ensures` is
    // the return value, awaited for an `Effect`.
    if !f.requires.is_empty() || !f.ensures.is_empty() {
        let result_ty = match &*tys.get(return_ty) {
            Ty::Effect(inner) => *inner,
            _ => return_ty,
        };
        let has_result_param = f.params.iter().any(|p| p.name.name == "result");
        let fn_label = format!("function `{}`", f.name.display());
        check_contracts(
            &f.requires,
            &f.ensures,
            &param_scope,
            result_ty,
            has_result_param,
            &fn_label,
            input,
            expr_types,
            errors,
            refs,
            hints,
            locals,
            requirements,
            callees,
            &vars,
            &eq,
            tys,
        );
    }
    let effectful = return_ty.is_effect(tys);
    let mut ctx = Ctx {
        input,
        tys,
        expr_types,
        errors,
        refs,
        hints,
        locals,
        requirements,
        callees,
        scopes: vec![param_scope],
        is_binding_cache: HashMap::new(),
        admitted_literals: HashMap::new(),
        pattern_binding_types: HashMap::new(),
        let_else_removals: HashMap::new(),
        return_ty,
        return_ty_span: f.return_type.span(),
        effectful,
        agent_state_ty: None,
        commit_seen: false,
        caps: CapabilityCtx::default(),
        in_test_body: false,
        oidc_binder: None,
        test_services: HashMap::new(),
        test_actors: HashMap::new(),
        type_vars: vars.clone(),
        eq: eq.clone(),
        last_witness: None,
        store_fields: HashMap::new(),
        in_agent_handler: false,
        in_service_handler: false,
        in_fn_body: true,
    };
    let Some(body_ty) = type_of_block(&f.body, Some(return_ty), &mut ctx) else {
        return;
    };
    // #718: run the held-resource linearity pass over `fn`/method bodies too, not
    // just handlers. The caller side treats passing a held value into a function
    // as a transfer (disposal), so the callee *owns* any held parameter and must
    // dispose it (close, store, or transfer) before returning — otherwise a
    // `swallow(c)` leaks and a double `c.close()` goes undiagnosed. No parameter
    // is borrowed here (the borrowed case is a handler's firing connection), so
    // the borrowed set is empty; every held param is seeded owned.
    linearity::check(
        &f.body,
        &f.params,
        &input.types,
        ctx.expr_types,
        &ctx.pattern_binding_types,
        &HashSet::new(),
        ctx.errors,
        tys,
    );
    if !compatible(body_ty, return_ty, tys) {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.return_mismatch",
                f.body.tail.span,
                format!(
                    "function body has type `{}`, but the declared return type is `{}`",
                    body_ty.display(tys),
                    return_ty.display(tys)
                ),
            )
            .with_label(f.return_type.span(), "declared return type"),
        );
    }
}

/// The shared discipline behind both `check_state_initialiser` (agent `store`
/// fields) and `check_event_field_default` (Events slice 3a, #972, event
/// record fields): the initialiser must be a *static* value of the field
/// type — checked in an empty, pure scope (so `self`, parameters,
/// capabilities, and effects are all out of reach) with the field type as
/// the expected type, so refined literals admit (v0.9.4) and sum variants
/// resolve. The init's expression types are recorded into `expr_types` for
/// emission, and its `Callee` classification (a `.of`/`.unsafe`/variant-
/// constructor call, at most) into `callees` (P6.7, #1163) — every
/// `Callee`-driven reader of a call-shaped expression reads `program.callees`
/// unconditionally (ADR 0334), so a call-shaped static initialiser with no
/// entry there would panic on a certified program the checker legitimately
/// accepted, not a recoverable state. (The IR-side `Cell`-init lowering that
/// first motivated this was deleted by Slice D1 of #1542; the invariant
/// outlives it because `bynk-emit`'s own emitter reads the same table.)
/// `code`/`subject`
/// name the caller's own diagnostic and the noun used in its message ("state
/// field initialiser" / "event field default").
#[allow(clippy::too_many_arguments)]
fn check_static_initialiser(
    init: &Expr,
    field_type: &TypeRef,
    input: &ResolvedCommons,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    callees: &mut HashMap<ExprId, Callee>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    code: &'static str,
    subject: &str,
    tys: &Types,
) {
    let Some((field_ty, result, local_errors)) = type_static_initialiser(
        init, field_type, input, expr_types, callees, refs, hints, locals, tys,
    ) else {
        return; // an unresolved field type is reported elsewhere
    };
    let compatible_result = matches!(&result, Some(t) if compatible(*t, field_ty, tys));
    if !compatible_result || !local_errors.is_empty() {
        let got = result
            .map(|t| t.display(tys))
            .unwrap_or_else(|| "an invalid expression".to_string());
        errors.push(
            CompileError::new(
                code,
                init.span,
                format!(
                    "{subject} must be a static value of type `{}` (got `{got}`)",
                    field_ty.display(tys),
                ),
            )
            .with_note(
                "an initialiser is a compile-time value — a literal (including one admitted to a \
                 refined type), a sum variant, `Some`/`None`/`Ok`/`Err`, a record, or — for an \
                 opaque type — `T.unsafe(lit)` — with no reference to `self`, parameters, or \
                 capabilities",
            ),
        );
    }
}

/// The typing half of [`check_static_initialiser`]: `init` typed against
/// `field_type` in an empty, pure scope, recording its expression types and
/// callees. Returns the resolved field type, the initialiser's type and the
/// diagnostics the typing raised (unreported), or `None` when `field_type`
/// does not resolve.
#[allow(clippy::too_many_arguments)]
fn type_static_initialiser(
    init: &Expr,
    field_type: &TypeRef,
    input: &ResolvedCommons,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    callees: &mut HashMap<ExprId, Callee>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
    tys: &Types,
) -> Option<(TyId, Option<TyId>, Vec<CompileError>)> {
    let field_ty = resolve_type_ref(field_type, &input.types, tys)?;
    let mut local_errors: Vec<CompileError> = Vec::new();
    // A static initialiser is a pure value — no capability calls reach here —
    // so requirements are discarded into a throwaway sink.
    let mut init_requirements = RequirementSink::new();
    let result = {
        let mut ctx = Ctx {
            input,
            tys,
            expr_types,
            errors: &mut local_errors,
            refs,
            hints,
            locals,
            requirements: &mut init_requirements,
            callees,
            scopes: vec![HashMap::new()],
            is_binding_cache: HashMap::new(),
            admitted_literals: HashMap::new(),
            pattern_binding_types: HashMap::new(),
            let_else_removals: HashMap::new(),
            return_ty: field_ty,
            return_ty_span: init.span,
            effectful: false,
            agent_state_ty: None,
            commit_seen: false,
            caps: CapabilityCtx::default(),
            in_test_body: false,
            oidc_binder: None,
            test_services: HashMap::new(),
            test_actors: HashMap::new(),
            type_vars: HashSet::new(),
            eq: EqScope::default(),
            last_witness: None,
            store_fields: HashMap::new(),
            in_agent_handler: false,
            in_service_handler: false,
            in_fn_body: false,
        };
        type_of(init, Some(field_ty), &mut ctx)
    };
    Some((field_ty, result, local_errors))
}

/// #2617: check a `const` declaration declared in this file. Its name is
/// upper-camel (`bynk.resolve.const_name_case`) and is no variant of a sum
/// type in scope (`bynk.resolve.name_conflict`); its initialiser is inside the
/// closed static set (`bynk.resolve.bad_const_initialiser`, decision A) and,
/// typed against the annotation in an empty, pure scope (the
/// `check_static_initialiser` discipline), a value of that type. A diagnostic
/// the typing raises (a literal its refined type rejects, `.unsafe` outside the
/// defining commons) is reported as it stands; a value of another type is
/// `bynk.resolve.bad_const_initialiser`. The initialiser's expression types
/// are recorded for the emitter.
#[allow(clippy::too_many_arguments)]
pub fn check_const_decl(
    c: &ConstDecl,
    input: &ResolvedCommons,
    tys: &Types,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    callees: &mut HashMap<ExprId, Callee>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
) {
    errors.extend(crate::consts::name_case_error(c));
    errors.extend(crate::consts::variant_conflict_error(c, &input.types));
    if let Some(err) = crate::consts::static_error(c, &input.consts, &input.types) {
        errors.push(err);
        return;
    }
    let Some((field_ty, result, local_errors)) = type_static_initialiser(
        &c.init, &c.ty, input, expr_types, callees, refs, hints, locals, tys,
    ) else {
        return; // an unresolved annotation is the resolver's to report
    };
    if !local_errors.is_empty() {
        errors.extend(local_errors);
        return;
    }
    let Some(got) = result else { return };
    if !compatible(got, field_ty, tys) {
        errors.push(
            CompileError::new(
                "bynk.resolve.bad_const_initialiser",
                c.init.span,
                format!(
                    "constant `{}` is declared `{}`, but its initialiser is a `{}`",
                    c.name.name,
                    field_ty.display(tys),
                    got.display(tys),
                ),
            )
            .with_label(c.ty.span(), "declared type"),
        );
    }
}

/// v0.11: type-check an agent state-field initialiser (`field: T = init`).
/// See `check_static_initialiser`. Pushes `bynk.agents.bad_state_initialiser`.
#[allow(clippy::too_many_arguments)]
pub fn check_state_initialiser(
    init: &Expr,
    field_type: &TypeRef,
    input: &ResolvedCommons,
    tys: &Types,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    callees: &mut HashMap<ExprId, Callee>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
) {
    check_static_initialiser(
        init,
        field_type,
        input,
        expr_types,
        callees,
        errors,
        refs,
        hints,
        locals,
        "bynk.agents.bad_state_initialiser",
        "state field initialiser",
        tys,
    );
}

/// Events slice 3a (#972): type-check an event field's default expression
/// (`field: T = init`), reusing `check_static_initialiser`'s empty-pure-scope
/// discipline. Pushes `bynk.types.bad_field_default`.
///
/// One admission `check_static_initialiser` doesn't cover on its own: an
/// opaque type's `T.unsafe(lit)` is, by design (ADR 0182), a bypass of its
/// own refinement — `type_of`'s ordinary static-call handling only
/// checks `lit`'s *base* type, not the refinement, since that's the whole
/// point of `unsafe`. An event field default is different: it becomes part
/// of the wire codec (slice 3a lowers it to its *wire* JSON form and splices
/// it into `deserialise_<Event>`, which validates a defaulted value exactly
/// like a real one), so a default that bypasses its own refinement would
/// compile cleanly and then fail at runtime the first time an old event
/// actually triggers it — a deferred, surprising failure this check closes
/// statically instead. Only `T.unsafe(lit)` on a *literal* argument is
/// checked here (anything else already fails the static-value requirement
/// above); a violated refinement pushes the same `bynk.types.bad_field_default`.
#[allow(clippy::too_many_arguments)]
pub fn check_event_field_default(
    init: &Expr,
    field_type: &TypeRef,
    // #2607: what the default belongs to, for the message ("event field
    // default", "record field default").
    subject: &str,
    input: &ResolvedCommons,
    tys: &Types,
    expr_types: &mut HashMap<ExprId, TypedExpr>,
    callees: &mut HashMap<ExprId, Callee>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    hints: &mut HintSink,
    locals: &mut LocalsSink,
) {
    check_static_initialiser(
        init,
        field_type,
        input,
        expr_types,
        callees,
        errors,
        refs,
        hints,
        locals,
        "bynk.types.bad_field_default",
        subject,
        tys,
    );
    // `T.unsafe(lit)` parses as `ExprKind::MethodCall { receiver: Ident(T),
    // method: "unsafe", .. }` — confirmed by direct AST inspection; the
    // parser never distinguishes a type-qualified call from an ordinary
    // instance method call (that's a resolver-time decision).
    if let ExprKind::MethodCall {
        receiver,
        method,
        args,
        ..
    } = &init.kind
        && let ExprKind::Ident(type_name) = &receiver.kind
        && method.name == "unsafe"
        && let [lit_expr] = args.as_slice()
        && let Some(decl) = input.types.get(&type_name.name)
        && matches!(decl.body, TypeBody::Opaque { .. })
        && let Some(refinement) = refinements::type_decl_refinement(decl)
        && let Some(lit) = refinements::const_literal(lit_expr)
        && let Some(failed) = refinements::first_failed_predicate(refinement, &lit)
    {
        errors.push(
            CompileError::new(
                "bynk.types.bad_field_default",
                lit_expr.span,
                format!(
                    "`{}.unsafe(...)` bypasses its own refinement, but a {subject} \
                     must be a value the wire could actually carry — this literal fails `{}`",
                    type_name.name,
                    failed.name(),
                ),
            )
            .with_note(
                "a default is spliced into the same codec that validates a real wire value on \
                 receipt, so a refinement-violating `.unsafe(lit)` default would compile cleanly \
                 and then fail at runtime the first time an old event actually triggers it",
            ),
        );
    }
}

/// Standard-library S3 (#1963): refuse a use of the experimental first-party
/// item `path` at `span` unless the project's `[unstable] allow` covers it.
fn refuse_experimental(path: &str, span: Span, ctx: &mut Ctx) {
    if let Some(err) = ctx.input.stability.policy.check(path, span) {
        ctx.errors.push(err);
    }
}

/// S3 (#1963): a reference to an imported first-party name the project's policy
/// blocks, called or passed as a value. The unit's blocked imports were decided where its provenance is
/// known, so this holds in handler bodies too.
pub(crate) fn refuse_experimental_import(name: &Ident, ctx: &mut Ctx) {
    if let Some(path) = ctx.input.stability.blocked_imports.get(&name.name).cloned() {
        refuse_experimental(&path, name.span, ctx);
    }
}

/// v0.91 (ADR 0116 D6): the `bynk.list` free functions whose method forms
/// shipped with slice 1 are **deprecated** in favour of the method-chain
/// vocabulary. `reverse`/`traverse` stay (no method form yet). Emits a
/// non-failing warning (ADR 0117) with a machine-applicable rewrite to the
/// method form — `map(xs, f)` → `xs.map(f)`, `find(xs, p)` → `xs.filter(p).first()`.
fn warn_bynk_list_deprecation(name: &Ident, args: &[Expr], call_span: Span, ctx: &mut Ctx) {
    if ctx.input.imported_from.get(&name.name).map(String::as_str) != Some("bynk.list") {
        return;
    }
    // The method-form spelling each free function rewrites to.
    let method_form: &str = match name.name.as_str() {
        "map" => "xs.map(f)",
        "filter" => "xs.filter(p)",
        "any" => "xs.any(p)",
        "all" => "xs.all(p)",
        "find" => "xs.filter(p).first()",
        _ => return, // reverse / traverse keep their free-function form for now
    };
    let mut err = CompileError::new(
        "bynk.list.deprecated_function",
        name.span,
        format!(
            "`bynk.list.{}` is deprecated — use the `List` method form `{method_form}`",
            name.name
        ),
    )
    .with_note(
        "the `bynk.list.*` free functions are superseded by the method-chain vocabulary (ADR 0116); the method form reads left-to-right and chains",
    );
    // The auto-fix needs the (list, fn) shape — a wrong-arity call is reported
    // elsewhere; only offer the rewrite when it is well-formed.
    if args.len() == 2 {
        let mut edits = vec![
            // delete `name(` — the receiver becomes the first argument.
            (
                Span::new(name.span.start, args[0].span.start),
                String::new(),
            ),
            // the `, ` between the two args becomes `.<method>(`.
            (
                Span::new(args[0].span.end, args[1].span.start),
                format!(
                    ".{}(",
                    if name.name == "find" {
                        "filter"
                    } else {
                        &name.name
                    }
                ),
            ),
        ];
        if name.name == "find" {
            // `filter(p)` then `.first()` after the closing `)`.
            edits.push((
                Span::new(call_span.end, call_span.end),
                ".first()".to_string(),
            ));
        }
        err = err.with_suggestion(
            format!("rewrite to the `List` method form `{method_form}`"),
            edits,
            Applicability::MachineApplicable,
        );
    }
    ctx.errors.push(err);
}

pub(crate) fn check_call(
    name: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    // #593: the binding's expected type grounds a generic variant constructor
    // whose payload cannot determine every parameter (`let o: Opt[Int] = Nil`).
    expected: Option<TyId>,
    // P6.0 (#1139): the outer `Call` expression's own identity — keys the
    // `Callee` classification recorded at whichever branch below dispatches.
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if let Some(fn_decl) = ctx.input.fns.get(&name.name) {
        ctx.refs.record(name.span, SymbolKind::Fn, &name.name);
        let fn_decl = Arc::clone(fn_decl);
        ctx.callees
            .insert(expr_id, Callee::Fn(Arc::clone(&fn_decl), Vec::new()));
        warn_bynk_list_deprecation(name, args, span, ctx);
        refuse_experimental_import(name, ctx);
        let ret = check_call_against_fn(name, &fn_decl, type_args, args, ctx);
        // Slice 1: the comparator witnesses the generic-call path resolved.
        if let Some(witnesses) = ctx.last_witness.take() {
            ctx.callees
                .insert(expr_id, Callee::Fn(Arc::clone(&fn_decl), witnesses));
        }
        return ret;
    }
    // v0.20a: explicit type arguments only apply to (generic) functions.
    if !type_args.is_empty() {
        ctx.errors.push(CompileError::new(
            "bynk.generics.type_arg_mismatch",
            span,
            format!(
                "`{}` is not a generic function — it takes no type arguments",
                name.name
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    // Could be a bare variant constructor with payload. Borrow the matching
    // sum decls' own `Arc`s (a pointer bump each, no full-body clones); the
    // ambiguity check below reuses the same list.
    let owners: Vec<&Arc<TypeDecl>> = ctx
        .input
        .types
        .values()
        .filter(|t| matches!(&t.body, TypeBody::Sum(s) if s.variants.iter().any(|v| v.name.name == name.name)))
        .collect();
    if owners.len() == 1 {
        ctx.callees.insert(
            expr_id,
            Callee::Ctor {
                sum: Arc::clone(owners[0]),
                tag: name.name.clone(),
            },
        );
        return check_variant_construction(owners[0], &name.name, args, span, expected, ctx);
    }
    // Agent instantiation: `AgentName(key)` constructs an instance keyed by
    // `key`. The result type carries the agent's name so subsequent
    // `agent_instance.method(args)` lookups can find the agent's handler set.
    if let Some(agent) = ctx.input.agents.get(&name.name).cloned() {
        ctx.refs.record(name.span, SymbolKind::Agent, &name.name);
        ctx.callees
            .insert(expr_id, Callee::AgentInit(name.name.clone()));
        let key_ty = resolve_type_ref(&agent.key_type, &ctx.input.types, tys);
        if args.len() != 1 {
            let mut err = CompileError::new(
                "bynk.agent.construction_arity",
                span,
                format!(
                    "agent `{}` is constructed with one key argument, but {} were given",
                    name.name,
                    args.len()
                ),
            );
            // #1925: the agent hides a `uses`-imported value of its name,
            // which may be what the call meant.
            if let Some(s) = ctx.input.shadowed_imports.get(&name.name) {
                err = err.with_note(format!(
                    "this unit's agent `{}` hides the {} of that name from `{}`; rename one to use the {}",
                    name.name, s.imported, s.unit, s.imported
                ));
            }
            ctx.errors.push(err);
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        }
        let arg_ty = type_of(&args[0], key_ty, ctx);
        if let (Some(a), Some(k)) = (arg_ty, key_ty)
            && !compatible(a, k, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.agent.key_mismatch",
                args[0].span,
                format!(
                    "agent `{}` key is `{}`, but a value of type `{}` was given",
                    name.name,
                    k.display(tys),
                    a.display(tys)
                ),
            ));
        }
        return Some(tys.intern(Ty::Named {
            name: name.name.clone(),
            kind: NamedKind::Record,
            args: Vec::new(),
        }));
    }
    // v0.20a: value application — calling a scope binding (param/local) of
    // function type. Placed AFTER fns/variants/agents: putting scope first
    // would change the meaning of currently-passing programs (the additive
    // guard); the resulting ident/call precedence asymmetry is pre-existing
    // and documented in §5.
    if let Some(ty) = ctx.lookup(&name.name) {
        return match &*tys.get(ty) {
            Ty::Fn { params, ret } => {
                ctx.callees
                    .insert(expr_id, Callee::Value(name.name.clone()));
                check_value_application(name, params, *ret, args, span, ctx)
            }
            _ => {
                // Relocated from the resolver (which has no type info): a
                // non-function-typed value called as a function.
                ctx.errors.push(
                    CompileError::new(
                        "bynk.resolve.param_as_function",
                        span,
                        format!(
                            "`{}` has type `{}` and is not callable",
                            name.name,
                            ty.display(tys)
                        ),
                    )
                    .with_note("only values of function type can be applied"),
                );
                for a in args {
                    let _ = type_of(a, None, ctx);
                }
                None
            }
        };
    }
    // Nothing owns the call. The resolver's reference walk reports these in
    // `fn`/method bodies, but handler/service/agent bodies and test bodies
    // never pass through that walk — the checker is their only backstop, and
    // a silent `None` here admitted any unknown call and emitted it verbatim.
    // Mirror the resolver's ladder. #1708: test bodies were exempt, for the
    // same since-repaired reason as `check_ident`'s twin gate; dropped with it.
    for a in args {
        let _ = type_of(a, None, ctx);
    }
    if owners.len() > 1 {
        ctx.errors.push(CompileError::new(
            "bynk.resolve.ambiguous_variant",
            name.span,
            format!(
                "the variant name `{}` is declared on multiple sum types — qualify it as `TypeName.{}(...)`",
                name.name, name.name
            ),
        ));
        return None;
    }
    // #1925: no local function or agent answers the call, and a local
    // declaration of another kind hides the import.
    if let Some(err) = ctx.input.shadowed_import_error(&name.name, name.span) {
        ctx.errors.push(err);
        return None;
    }
    if ctx.input.types.contains_key(&name.name) {
        ctx.errors.push(CompileError::new(
            "bynk.resolve.type_as_function",
            span,
            format!(
                "`{}` is a type, not a function — use `{}.of(value)` or `{} {{ ... }}` instead",
                name.name, name.name, name.name
            ),
        ));
        return None;
    }
    ctx.errors.push(
        CompileError::new(
            "bynk.resolve.unknown_function",
            span,
            format!("unknown function `{}`", name.name),
        )
        .with_note("only functions declared in this commons are callable"),
    );
    None
}

fn check_value_application(
    name: &Ident,
    params: &[TyId],
    ret: TyId,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if ret.is_effect(tys) && !ctx.effectful {
        ctx.errors.push(
            CompileError::new(
                "bynk.effect.fn_value_in_pure_context",
                span,
                format!(
                    "`{}` is an effectful function (`{}`) and cannot be called in a pure context",
                    name.name,
                    Ty::Fn {
                        params: params.to_vec(),
                        ret,
                    }
                    .display(tys)
                ),
            )
            .with_note(
                "effectful function values may only be called where the enclosing body is effectful (its return type is an Effect)",
            ),
        );
    }
    if params.len() != args.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.call_arity",
            span,
            format!(
                "`{}` takes {} argument(s), but {} were given",
                name.name,
                params.len(),
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    for (arg, param_ty) in args.iter().zip(params) {
        let arg_ty = type_of(arg, Some(*param_ty), ctx);
        if let Some(a) = arg_ty
            && !compatible(a, *param_ty, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                arg.span,
                format!(
                    "argument has type `{}`, but `{}` expects `{}`",
                    a.display(tys),
                    name.name,
                    param_ty.display(tys)
                ),
            ));
        }
    }
    Some(ret)
}

/// v0.20a: instantiate and check a call to a generic function. Two-pass
/// argument-directed inference: pass 1 types every **non-lambda** argument
/// left-to-right against the (possibly still Var-bearing) expected — safe
/// because every expected-driven feature in `type_of` matches concrete
/// variants, so a Var falls through benignly (pinned by a unit test) — and
/// unifies; pass 2 types **lambda** arguments against the now-substituted
/// expecteds (a lambda whose expected params are still Var-bearing is
/// uninferable) and unifies the result, capturing return-position variables.
/// Conflicts demand exact equality (`bynk.generics.type_arg_mismatch`); the
/// explicit `name[T](…)` form builds the substitution directly.
fn check_generic_call(
    name: &Ident,
    fn_decl: &FnDecl,
    type_args: &[TypeRef],
    args: &[Expr],
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let vars: HashSet<String> = fn_decl
        .type_params
        .iter()
        .map(|tp| tp.name.name.clone())
        .collect();
    if fn_decl.params.len() != args.len() {
        // Same backstop as the non-generic path: the resolver only covers
        // `fn`/method bodies, so the checker must report for handler bodies.
        ctx.errors.push(
            CompileError::new(
                "bynk.resolve.arity_mismatch",
                name.span,
                format!(
                    "function `{}` expects {} argument(s), but {} were given",
                    name.name,
                    fn_decl.params.len(),
                    args.len()
                ),
            )
            .with_label(fn_decl.name.ident().span, "function declared here"),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let var_params: Vec<Option<TyId>> = fn_decl
        .params
        .iter()
        .map(|p| resolve_type_ref_in(&p.type_ref, &ctx.input.types, &vars, tys))
        .collect();
    let ret_pattern = resolve_type_ref_in(&fn_decl.return_type, &ctx.input.types, &vars, tys)?;

    let mut subst: HashMap<String, TyId> = HashMap::new();
    if !type_args.is_empty() {
        if type_args.len() != fn_decl.type_params.len() {
            ctx.errors.push(CompileError::new(
                "bynk.generics.type_arg_mismatch",
                name.span,
                format!(
                    "`{}` takes {} type argument(s), but {} were given",
                    name.name,
                    fn_decl.type_params.len(),
                    type_args.len()
                ),
            ));
            return None;
        }
        for (tp, ta) in fn_decl.type_params.iter().zip(type_args) {
            // Resolve explicit type args with the *enclosing* fn's type
            // params in scope, so `identity[A](x)` inside a generic body
            // works. #712: report (not silently drop) an unknown type arg —
            // the resolver never validates `Call::type_args` (it destructures
            // `Call { name, args, .. }`), so this is the only guard.
            let ty = resolve_expr_type_ref(ta, ctx)?;
            subst.insert(tp.name.name.clone(), ty);
        }
    }

    let mut arg_tys: Vec<Option<TyId>> = vec![None; args.len()];
    // Pass 1 — non-lambda arguments.
    for (i, arg) in args.iter().enumerate() {
        if matches!(arg.kind, ExprKind::Lambda(_)) {
            continue;
        }
        let expected = var_params[i].map(|p| substitute(p, &subst, tys));
        let ty = type_of(arg, expected, ctx);
        if let (Some(pattern), Some(actual)) = (var_params[i], ty)
            && !unify(pattern, actual, &mut subst, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.generics.type_arg_mismatch",
                arg.span,
                format!(
                    "argument {} infers a type for `{}`'s type parameter that conflicts with an earlier argument — annotate with `{}[T](…)`",
                    i + 1,
                    name.name,
                    name.name
                ),
            ));
            return None;
        }
        arg_tys[i] = ty;
    }
    // Pass 2 — lambda arguments, against substituted expecteds.
    for (i, arg) in args.iter().enumerate() {
        if !matches!(arg.kind, ExprKind::Lambda(_)) {
            continue;
        }
        let expected = var_params[i].map(|p| substitute(p, &subst, tys));
        let params_unconstrained = expected.is_some_and(|e| {
            matches!(&*tys.get(e), Ty::Fn { params, .. }
                if params.iter().any(|p| contains_var(*p, tys)))
        });
        let fully_annotated = matches!(
            &arg.kind,
            ExprKind::Lambda(l) if l.params.iter().all(|p| lambda_param_is_typed(p, ctx))
        );
        if params_unconstrained && !fully_annotated {
            ctx.errors.push(
                CompileError::new(
                    "bynk.generics.uninferable_type_arg",
                    arg.span,
                    format!(
                        "the lambda's parameter types depend on `{}`'s type parameters, which the other arguments do not determine",
                        name.name
                    ),
                )
                .with_note("annotate the lambda's parameters, or give explicit type arguments: `name[T](…)`"),
            );
            return None;
        }
        // A fully-annotated lambda grounds the variables itself: type it
        // bottom-up and let unify capture them.
        let ty = if params_unconstrained {
            type_of(arg, None, ctx)
        } else {
            type_of(arg, expected, ctx)
        };
        if let (Some(pattern), Some(actual)) = (var_params[i], ty)
            && !unify(pattern, actual, &mut subst, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.generics.type_arg_mismatch",
                arg.span,
                format!(
                    "the lambda's type conflicts with `{}`'s inferred type arguments",
                    name.name
                ),
            ));
            return None;
        }
        arg_tys[i] = ty;
    }
    // Every type parameter must now be determined.
    for tp in &fn_decl.type_params {
        if !subst.contains_key(&tp.name.name) {
            ctx.errors.push(
                CompileError::new(
                    "bynk.generics.uninferable_type_arg",
                    name.span,
                    format!(
                        "type parameter `{}` of `{}` is neither inferable from the arguments nor given explicitly",
                        tp.name.name, name.name
                    ),
                )
                .with_label(tp.span, "declared here")
                .with_note("give explicit type arguments: `name[T](…)`"),
            );
            return None;
        }
    }
    // Final compatibility over the fully-ground parameter types.
    let mut ok = true;
    for (i, (pattern, arg)) in var_params.iter().zip(args).enumerate() {
        record_param_hint(ctx.hints, &fn_decl.params[i].name.name, arg);
        let (Some(pattern), Some(arg_ty)) = (pattern, arg_tys[i].as_ref()) else {
            continue;
        };
        let ground = substitute(*pattern, &subst, tys);
        if !compatible(*arg_ty, ground, tys) {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                arg.span,
                format!(
                    "argument {} to `{}` has type `{}`, but `{}` is expected",
                    i + 1,
                    name.name,
                    arg_ty.display(tys),
                    ground.display(tys)
                ),
            ));
            ok = false;
        }
    }
    if !ok {
        return None;
    }
    // Constrained generics slice 0: each `Eq`-bounded type parameter of the
    // callee must be instantiated with an equality-supporting type. A callee
    // that compares a parameter it has not yet declared (reported in its own
    // body) is held to it too, so one pass reports a whole chain of generics
    // that need the bound and a single fix-all applies them.
    let compared = match ctx.input.fns.get(&name.name) {
        Some(decl) if std::ptr::eq(decl.as_ref(), fn_decl) => {
            super::equality::compared_type_params(decl, ctx)
        }
        _ => Default::default(),
    };
    let ord_names = super::ordering::witness_params(fn_decl, &ctx.input.types);
    let key_names = super::keying::key_params(fn_decl, &ctx.input.types);
    let eq_params: Vec<(String, Option<Span>)> = fn_decl
        .type_params
        .iter()
        .filter(|tp| super::equality::has_eq(&tp.bounds) || compared.contains(&tp.name.name))
        // An `Ord`- or `Key`-bounded parameter is checked as such below, which
        // covers `Eq`.
        .filter(|tp| !ord_names.contains(&tp.name.name) && !key_names.contains(&tp.name.name))
        .map(|tp| (tp.name.name.clone(), Some(tp.span)))
        .collect();
    if !super::equality::check_declared_eq_args(&name.name, name.span, &eq_params, &subst, ctx) {
        return None;
    }
    // Slice 1: each `Ord`-bounded type parameter takes an orderable type, and
    // the call carries a comparator witness for it.
    let witnesses = super::codable::check_declared_witness_args(
        &name.name,
        name.span,
        &super::codable::witness_list(fn_decl, &ctx.input.types),
        &subst,
        ctx,
    )?;
    // Slice 2: each `Key`-bounded type parameter takes a key type. `Key` is
    // witness-free: nothing is recorded on the call.
    if !super::keying::check_declared_key_args(&name.name, name.span, &key_names, &subst, ctx) {
        return None;
    }
    // v0.39 (ADR 0072): when the user omitted the type arguments, show the
    // inferred ones as a `Type`-kind hint after the function name —
    // `identity` ⟨`[Int]`⟩ `(5)`. Declaration order; skipped if any var stayed
    // unresolved (defensive — the arg loop above already grounds them).
    if type_args.is_empty() && !fn_decl.type_params.is_empty() {
        let rendered: Option<Vec<String>> = fn_decl
            .type_params
            .iter()
            .map(|tp| subst.get(&tp.name.name).map(|t| t.display(tys)))
            .collect();
        if let Some(parts) = rendered {
            ctx.hints
                .record(name.span, format!("[{}]", parts.join(", ")));
        }
    }
    if !witnesses.is_empty() {
        ctx.last_witness = Some(witnesses);
    }
    let ret = substitute(ret_pattern, &subst, tys);
    // v0.20b: the return is ground *up to the caller's rigid type
    // parameters* — a generic fn calling another generic fn (bynk.list's
    // `map` calling `reverse`) legitimately instantiates the callee at its
    // own rigid vars, which flow through `compatible` by name-equality.
    Some(ret)
}

fn check_call_against_fn(
    name: &Ident,
    fn_decl: &FnDecl,
    type_args: &[TypeRef],
    args: &[Expr],
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // v0.20a: generic functions take the instantiation path; the
    // non-generic path below runs byte-identically to v0.19 (the additive
    // guard). Explicit type args on a non-generic fn are rejected.
    if !fn_decl.type_params.is_empty() {
        return check_generic_call(name, fn_decl, type_args, args, ctx);
    }
    if !type_args.is_empty() {
        ctx.errors.push(CompileError::new(
            "bynk.generics.type_arg_mismatch",
            name.span,
            format!(
                "`{}` is not a generic function — it takes no type arguments",
                name.name
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    if fn_decl.params.len() != args.len() {
        // The resolver reports this in `fn`/method bodies; handler/service/
        // agent/test bodies only reach the checker, so report here too (a
        // resolve error stops the pipeline first, so this cannot double up).
        ctx.errors.push(
            CompileError::new(
                "bynk.resolve.arity_mismatch",
                name.span,
                format!(
                    "function `{}` expects {} argument(s), but {} were given",
                    name.name,
                    fn_decl.params.len(),
                    args.len()
                ),
            )
            .with_label(fn_decl.name.ident().span, "function declared here"),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let resolved_params: Vec<(Option<TyId>, &Param)> = fn_decl
        .params
        .iter()
        .map(|p| (resolve_type_ref(&p.type_ref, &ctx.input.types, tys), p))
        .collect();
    let mut ok = true;
    for (i, ((param_ty, param), arg)) in resolved_params.iter().zip(args.iter()).enumerate() {
        record_param_hint(ctx.hints, &param.name.name, arg);
        let arg_ty = type_of(arg, *param_ty, ctx);
        let (Some(arg_ty), Some(param_ty)) = (arg_ty, *param_ty) else {
            ok = false;
            continue;
        };
        if !compatible(arg_ty, param_ty, tys) {
            ctx.errors.push(
                CompileError::new(
                    "bynk.types.argument_mismatch",
                    arg.span,
                    format!(
                        "argument {} to `{}` has type `{}`, but parameter `{}` expects `{}`",
                        i + 1,
                        name.name,
                        arg_ty.display(tys),
                        param.name.name,
                        param_ty.display(tys)
                    ),
                )
                .with_label(param.span, "parameter declared here"),
            );
            ok = false;
        }
    }
    if !ok {
        return None;
    }
    resolve_type_ref(&fn_decl.return_type, &ctx.input.types, tys)
}

/// Slice 3: infer the type arguments of a capability operation from its
/// arguments, when they determine every type parameter. Returns the inference
/// (if complete) and the arguments' own types, which the caller reuses so no
/// argument is checked twice.
fn infer_op_type_args(
    op: &CapabilityOpInfo,
    args: &[Expr],
    ctx: &mut Ctx,
) -> (Option<HashMap<String, TyId>>, Vec<Option<TyId>>) {
    let tys = ctx.tys;
    let mut subst: HashMap<String, TyId> = HashMap::new();
    let mut typed = Vec::with_capacity(args.len());
    let mut ok = true;
    for (pattern, arg) in op.params.iter().zip(args) {
        // Only an argument whose parameter mentions a type parameter takes part
        // in inference; the others are checked against their own parameter type,
        // so contextual typing (`None`, `[]`, a literal for a `Float`) still holds.
        if !contains_var(*pattern, tys) {
            typed.push(type_of(arg, Some(*pattern), ctx));
            continue;
        }
        let t = type_of(arg, None, ctx);
        match t {
            Some(t) => ok &= unify(*pattern, t, &mut subst, tys),
            None => ok = false,
        }
        typed.push(t);
    }
    let complete = ok && op.type_params.iter().all(|tp| subst.contains_key(tp));
    (complete.then_some(subst), typed)
}

/// Type-check a kernel-method argument against its expected type, with the
/// expected type propagated in (so lambdas and literals type contextually).
pub(crate) fn check_arg(arg: &Expr, expected: TyId, what: &str, ctx: &mut Ctx) {
    let tys = ctx.tys;
    let Some(actual) = type_of(arg, Some(expected), ctx) else {
        return;
    };
    if !compatible(actual, expected, tys) {
        ctx.errors.push(CompileError::new(
            "bynk.types.type_mismatch",
            arg.span,
            format!(
                "{what} has type `{}`, but `{}` is required",
                actual.display(tys),
                expected.display(tys)
            ),
        ));
    }
}

/// Record a capability reference's binding edge, qualifying flattened bare
/// names (`consumes U { Cap }`) to their providing unit (v0.25). A bare
/// non-flattened name is the consuming unit's own declaration — qualified
/// at assembly.
fn record_capability_ref(span: Span, name: &str, ctx: &mut Ctx) {
    if let Some(unit) = ctx.input.cross_context.flattened_caps.get(name) {
        ctx.refs
            .record_in_unit(span, SymbolKind::Capability, name, unit);
    } else {
        ctx.refs.record(span, SymbolKind::Capability, name);
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn check_static_call(
    type_name: &Ident,
    method: &Ident,
    // #926: explicit type arguments for a generic capability operation
    // (`Cap.op[T](…)`). Consumed only by the capability-dispatch branch below
    // — the user-declared-static branch never receives a non-empty slice,
    // since `check_method_call`'s gate only lets type args through for a
    // capability (or the `Json` codec, handled separately) receiver.
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    // #593: threaded to `check_variant_construction` so a qualified generic
    // variant (`Opt.Nil`) can ground its arguments from the binding's type.
    expected: Option<TyId>,
    // P6.0 (#1139): the outer expression's identity — the `MethodCall` node
    // `check_method_call` is dispatching on behalf of.
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // Capability dispatch (v0.5): if `type_name` names a capability declared
    // in the context, dispatch via the capability table. If the capability is
    // declared but not in `given`, error specifically.
    if ctx.caps.declared_capabilities.contains_key(&type_name.name)
        && !ctx.caps.capabilities.contains_key(&type_name.name)
    {
        record_capability_ref(type_name.span, &type_name.name, ctx);
        ctx.callees.insert(
            expr_id,
            Callee::Capability {
                cap: type_name.name.clone(),
                op: method.name.clone(),
                wire: None,
            },
        );
        let mut err = CompileError::new(
            "bynk.given.undeclared_capability",
            type_name.span,
            format!(
                "capability `{}` is used but not listed in the handler's `given` clause",
                type_name.name
            ),
        )
        .with_note(format!(
            "add `{}` to the handler's `given` clause so the dependency surface is visible at the declaration site",
            type_name.name
        ));
        // v0.26 (ADR 0054): the one-click counterpart of the note.
        if let Some((span, insert)) = given_insertion_edit(
            &ctx.caps.given_entries,
            ctx.caps.given_anchor,
            &type_name.name,
        ) {
            err = err.with_suggestion(
                format!("add `{}` to the `given` clause", type_name.name),
                vec![(span, insert)],
                Applicability::MachineApplicable,
            );
        }
        ctx.errors.push(err);
        // v0.99: an uncovered direct capability call — record it so the ghost
        // `given` inlay hint can offer the same clause at the declaration site
        // (DECISION D/E). `span` is the call site (the inner `given_insertion_edit`
        // binding shadowed it only within the `if let` above).
        record_requirement(
            ctx,
            &type_name.name,
            span,
            RequirementSource::DirectCall {
                op: method.name.clone(),
            },
            false,
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    if let Some(cap) = ctx.caps.capabilities.get(&type_name.name).cloned() {
        record_capability_ref(type_name.span, &type_name.name, ctx);
        // S3 (#1963): a flattened capability's operation, by its owning unit.
        if let Some(unit) = ctx
            .input
            .cross_context
            .flattened_caps
            .get(&type_name.name)
            .cloned()
        {
            refuse_experimental(
                &format!("{unit}.{}.{}", type_name.name, method.name),
                method.span,
                ctx,
            );
        }
        ctx.callees.insert(
            expr_id,
            Callee::Capability {
                cap: type_name.name.clone(),
                op: method.name.clone(),
                wire: None,
            },
        );
        if !ctx.effectful {
            ctx.errors.push(
                CompileError::new(
                    "bynk.effect.capability_in_pure_context",
                    span,
                    format!(
                        "capability `{}` can only be called inside an effectful body (one returning `Effect[T]`)",
                        type_name.name
                    ),
                ),
            );
        }
        ctx.caps.given_used.insert(type_name.name.clone());
        // v0.99: a covered direct capability call — the call site *is* the
        // reason (DECISION C). Recorded so hover can explain what a declared
        // `given Cap` is for; no inlay hint (already covered).
        record_requirement(
            ctx,
            &type_name.name,
            span,
            RequirementSource::DirectCall {
                op: method.name.clone(),
            },
            true,
        );
        let Some(op) = cap.ops.iter().find(|o| o.name == method.name) else {
            ctx.errors.push(CompileError::new(
                "bynk.capability.unknown_operation",
                method.span,
                format!(
                    "capability `{}` has no operation named `{}`",
                    type_name.name, method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        };
        // v0.36 (ADR 0069, slice 2): the op is an index symbol keyed by the
        // compound `"Cap.op"` name; this local call is a reference. S4a
        // (#1966): a derived capability declares no operations, so its call
        // references the base's, in the base's unit, and is refused as the
        // base's would be.
        match &cap.derived_from {
            Some((unit, base)) => {
                refuse_experimental(&format!("{unit}.{base}.{}", method.name), method.span, ctx);
                ctx.refs.record_in_unit(
                    method.span,
                    SymbolKind::CapabilityOp,
                    &format!("{base}.{}", method.name),
                    unit,
                );
            }
            None => ctx.refs.record(
                method.span,
                SymbolKind::CapabilityOp,
                &format!("{}.{}", type_name.name, method.name),
            ),
        }
        // P4c (#2628): a `D1` statement's SQL is a literal.
        d1::check_sql_argument(
            cap.derived_from.as_ref(),
            &type_name.name,
            &method.name,
            args,
            ctx,
        );
        if op.params.len() != args.len() {
            ctx.errors.push(CompileError::new(
                "bynk.capability.op_arity",
                span,
                format!(
                    "capability operation `{}.{}` expects {} argument(s), but {} were given",
                    type_name.name,
                    method.name,
                    op.params.len(),
                    args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        }
        let op_clone = op.clone();
        // #926: resolve the op's own type parameter(s) from an explicit
        // call-site type argument — explicit only, never inferred (mirrors
        // `Json.decode[T]`, not `#594`'s inference). `check_generic_call`
        // (this file) is the model; only its arity-check + substitution-build
        // half applies here, since a capability op has no argument-driven
        // inference pass.
        let mut subst: HashMap<String, TyId> = HashMap::new();
        // Slice 3: an operation with `Codable` parameters infers its type
        // arguments from the arguments, when they determine every one.
        let mut arg_tys: Option<Vec<Option<TyId>>> = None;
        if type_args.is_empty() && !op_clone.codable_params.is_empty() {
            let (inferred, typed) = infer_op_type_args(&op_clone, args, ctx);
            arg_tys = Some(typed);
            if let Some(found) = inferred {
                subst = found;
            }
        }
        if !op_clone.type_params.is_empty() || !type_args.is_empty() {
            if type_args.is_empty() && subst.len() < op_clone.type_params.len() {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.generics.uninferable_type_arg",
                        span,
                        format!(
                            "capability operation `{}.{}` takes a type parameter, but none of its arguments determine it",
                            type_name.name, method.name
                        ),
                    )
                    .with_note(format!(
                        "give it explicitly: `{}.{}[T](…)`",
                        type_name.name, method.name
                    )),
                );
                if arg_tys.is_none() {
                    for a in args {
                        let _ = type_of(a, None, ctx);
                    }
                }
                return None;
            }
            if !type_args.is_empty() && type_args.len() != op_clone.type_params.len() {
                ctx.errors.push(CompileError::new(
                    "bynk.generics.type_arg_mismatch",
                    span,
                    format!(
                        "capability operation `{}.{}` takes {} type argument(s), but {} were given",
                        type_name.name,
                        method.name,
                        op_clone.type_params.len(),
                        type_args.len()
                    ),
                ));
                for a in args {
                    let _ = type_of(a, None, ctx);
                }
                return None;
            }
            for (tp, ta) in op_clone.type_params.iter().zip(type_args) {
                let ty = resolve_expr_type_ref(ta, ctx)?;
                // Events track, slice 0 (spine #936): owner-only emission —
                // `Events.emit[E]` may only name an event `E` declared in
                // *this* context, even though a `consumes`-visible foreign
                // event resolves here just as validly for every other
                // purpose. `is_local_type` already distinguishes "declared
                // here" from "visible via uses/consumes" (the same table
                // ADR 0256's locale-types-split gap analysis used), so this
                // is a check over existing data, not new provenance
                // plumbing — the primary boundary guarantee the threat
                // model (events.md §6) names.
                //
                // First-party-gated like every other Events-special-cased
                // site (mirrors #934's Idempotency precedent): a bare
                // `type_name.name == "Events"` string match would also fire
                // for a third-party capability that happens to declare its
                // own `Events` with an `emit` method — only bynk's own
                // capability (declared here, in `bynk` itself, or flattened
                // in via `consumes bynk { Events }`) gets this check.
                let is_first_party_events = type_name.name == "Events"
                    && method.name == "emit"
                    && (ctx.input.commons.name.joined() == crate::firstparty::BYNK_UNIT
                        || ctx
                            .input
                            .cross_context
                            .flattened_caps
                            .get("Events")
                            .map(String::as_str)
                            == Some(crate::firstparty::BYNK_UNIT));
                if is_first_party_events && let Ty::Named { name: ename, .. } = &*tys.get(ty) {
                    if ctx.input.is_local_event(ename) {
                        // Locally-declared event — the owner. Fine.
                    } else if ctx.input.is_local_type(ename) {
                        // Events track, slice 0: `UnitTable`/`ResolvedCommons`
                        // record which local names are specifically events
                        // (as opposed to any other locally-declared type) —
                        // `Events.emit[SomeLocalRecord]` compiled clean before
                        // this check existed, silently buffering an emission
                        // no `from Events(...)` subscriber could ever match
                        // (`discover_event_subscribers` finds no owner for a
                        // non-event name and just drops it).
                        ctx.errors.push(
                            CompileError::new(
                                "bynk.event.emit_not_an_event",
                                ta.span(),
                                format!(
                                    "`{ename}` is not a declared `event` — `Events.emit` may only name an event type"
                                ),
                            )
                            .with_note(
                                "declare it with `event Name = { ... }`, or check that the type argument names the event you meant",
                            ),
                        );
                    } else {
                        ctx.errors.push(
                            CompileError::new(
                                "bynk.event.emit_outside_owner",
                                ta.span(),
                                format!(
                                    "`{ename}` is not declared in this context — only the context that declares an event may emit it"
                                ),
                            )
                            .with_note(
                                "a foreign event is visible via `consumes` for subscription (`from Events(...)`), but only its owning context may `Events.emit` it",
                            ),
                        );
                    }
                }
                subst.insert(tp.clone(), ty);
            }
        }
        // Slice 3: each `Codable` type parameter takes a codable type, and the
        // call records how it encodes the parameters that mention one.
        if !check_op_codable_args(
            &format!("{}.{}", type_name.name, method.name),
            span,
            &op_clone.codable_params,
            &subst,
            ctx,
        ) {
            if arg_tys.is_none() {
                for a in args {
                    let _ = type_of(a, None, ctx);
                }
            }
            return None;
        }
        if let Some(wire) = op_encoding(
            &op_clone.type_params,
            &op_clone.codable_params,
            &op_clone.params,
            op_clone.return_ty,
            &subst,
            tys,
        ) {
            ctx.callees.insert(
                expr_id,
                Callee::Capability {
                    cap: type_name.name.clone(),
                    op: method.name.clone(),
                    wire: Some(wire),
                },
            );
        }
        for (i, (param_ty, arg)) in op_clone.params.iter().zip(args.iter()).enumerate() {
            let param_ty = substitute(*param_ty, &subst, tys);
            let arg_ty = match &arg_tys {
                Some(cached) => cached[i],
                None => type_of(arg, Some(param_ty), ctx),
            };
            if let Some(actual) = arg_ty
                && !compatible(actual, param_ty, tys)
            {
                ctx.errors.push(CompileError::new(
                    "bynk.types.argument_mismatch",
                    arg.span,
                    format!(
                        "argument {} to capability `{}.{}` has type `{}`, but parameter expects `{}`",
                        i + 1,
                        type_name.name,
                        method.name,
                        actual.display(tys),
                        param_ty.display(tys)
                    ),
                ));
            }
        }
        return Some(substitute(op_clone.return_ty, &subst, tys));
    }
    let decl = ctx.input.types.get(&type_name.name)?;
    ctx.refs
        .record(type_name.span, SymbolKind::Type, &type_name.name);

    // 1) User-declared static method. Borrow the method table and decl
    // straight out of the (immutable) input rather than cloning the whole
    // `MethodTable`/`FnDecl` on every static-shaped call.
    if let Some(method_decl) = ctx
        .input
        .methods
        .get(&type_name.name)
        .and_then(|table| table.statics.get(&method.name))
    {
        ctx.callees
            .insert(expr_id, Callee::Static(Arc::clone(method_decl)));
        return check_method_args(method_decl, args, ctx, type_name, method);
    }

    // 2) Built-in `of` constructor on refined or opaque types.
    if method.name == OF
        && let Some(base) = type_decl_base(decl)
    {
        ctx.callees
            .insert(expr_id, Callee::Refine(Arc::clone(decl)));
        if args.len() != 1 {
            ctx.errors.push(CompileError::new(
                "bynk.types.constructor_arity",
                span,
                format!(
                    "constructor `{}.of` expects 1 argument, but {} were given",
                    type_name.name,
                    args.len()
                ),
            ));
            return None;
        }
        let arg = &args[0];
        let expected = tys.intern(Ty::Base(base));
        let arg_ty = type_of(arg, Some(expected), ctx)?;
        if !compatible(arg_ty, expected, tys) {
            ctx.errors.push(CompileError::new(
                "bynk.types.constructor_base_mismatch",
                arg.span,
                format!(
                    "constructor `{}.of` expects a `{}` argument, but got `{}`",
                    type_name.name,
                    base.name(),
                    arg_ty.display(tys)
                ),
            ));
            return None;
        }
        // `.of` is always the runtime constructor: it returns
        // `Result[T, ValidationError]`. Compile-time literal admission (v0.9.4)
        // happens instead wherever an expected refined type is known — see
        // `admit_refined_literal`, used by `type_of` — so `.of`'s type never
        // depends on the form of its argument.
        return Some(tys.intern(Ty::Result(
            named_ty(decl, tys),
            tys.intern(Ty::ValidationError),
        )));
    }

    // 2b) Built-in `unsafe` constructor on opaque types — only available
    // inside the defining commons.
    if method.name == UNSAFE
        && let TypeBody::Opaque { base, .. } = &decl.body
    {
        ctx.callees
            .insert(expr_id, Callee::Unsafe(Arc::clone(decl)));
        if !ctx.input.is_local_type(&decl.name.name) {
            ctx.errors.push(
                CompileError::new(
                    "bynk.types.opaque_unsafe_outside",
                    method.span,
                    format!(
                        "`{}.unsafe(...)` is only available within the commons that defines the opaque type `{}`",
                        type_name.name, type_name.name
                    ),
                )
                .with_note(
                    "outside the defining commons, opaque values are constructed via `T.of(value)`",
                ),
            );
            return None;
        }
        if args.len() != 1 {
            ctx.errors.push(CompileError::new(
                "bynk.types.constructor_arity",
                span,
                format!(
                    "`{}.unsafe` expects 1 argument, but {} were given",
                    type_name.name,
                    args.len()
                ),
            ));
            return None;
        }
        let arg = &args[0];
        let expected = tys.intern(Ty::Base(*base));
        let arg_ty = type_of(arg, Some(expected), ctx)?;
        if !compatible(arg_ty, expected, tys) {
            ctx.errors.push(CompileError::new(
                "bynk.types.constructor_base_mismatch",
                arg.span,
                format!(
                    "`{}.unsafe` expects a `{}` argument, but got `{}`",
                    type_name.name,
                    base.name(),
                    arg_ty.display(tys)
                ),
            ));
            return None;
        }
        return Some(named_ty(decl, tys));
    }

    // 3) Qualified variant construction `TypeName.Variant(args)`.
    if let TypeBody::Sum(_) = &decl.body {
        ctx.callees.insert(
            expr_id,
            Callee::Ctor {
                sum: Arc::clone(decl),
                tag: method.name.clone(),
            },
        );
        return check_variant_construction(decl, &method.name, args, span, expected, ctx);
    }

    ctx.errors.push(
        CompileError::new(
            "bynk.types.unknown_static_member",
            method.span,
            format!(
                "type `{}` has no static method or variant named `{}`",
                type_name.name, method.name
            ),
        )
        // Finding #46: `decl` comes from the combined cross-file symbol
        // table — see resolver.rs:1029 for the full rationale.
        .with_note("type declared here"),
    );
    None
}

fn check_method_args(
    method_decl: &FnDecl,
    args: &[Expr],
    ctx: &mut Ctx,
    type_name: &Ident,
    method: &Ident,
) -> Option<TyId> {
    let tys = ctx.tys;
    if method_decl.params.len() != args.len() {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.method_arity",
                method.span,
                format!(
                    "static method `{}.{}` expects {} argument(s), but {} were given",
                    type_name.name,
                    method.name,
                    method_decl.params.len(),
                    args.len()
                ),
            )
            .with_label(method_decl.name.ident().span, "method declared here"),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let mut ok = true;
    for (i, (param, arg)) in method_decl.params.iter().zip(args.iter()).enumerate() {
        record_param_hint(ctx.hints, &param.name.name, arg);
        let expected = resolve_type_ref(&param.type_ref, &ctx.input.types, tys);
        let actual = type_of(arg, expected, ctx);
        let (Some(actual), Some(expected)) = (actual, expected) else {
            ok = false;
            continue;
        };
        if !compatible(actual, expected, tys) {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                arg.span,
                format!(
                    "argument {} to `{}.{}` has type `{}`, but parameter `{}` expects `{}`",
                    i + 1,
                    type_name.name,
                    method.name,
                    actual.display(tys),
                    param.name.name,
                    expected.display(tys)
                ),
            ));
            ok = false;
        }
    }
    if !ok {
        return None;
    }
    resolve_type_ref(&method_decl.return_type, &ctx.input.types, tys)
}

/// v0.82 (ADR 0110): resolve a storage-map operation `<map>.<op>(args)` on a
/// `store Map[K, V]` field. The ops are effect-typed (storage I/O, awaited with
/// `<-`): `put`/`update`/`upsert`/`remove` → `Effect[()]`, `get` →
/// `Effect[Option[V]]`, `contains` → `Effect[Bool]`, `size` → `Effect[Int]`.
/// `update` on an absent key is a runtime fault; `upsert` is the default-if-absent
/// form. Dispatched by receiver provenance, so it never shadows the immutable
/// value `Map`'s pure methods.
pub(crate) fn check_store_map_op(
    method: &Ident,
    args: &[Expr],
    k: TyId,
    v: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let vfn = || Ty::Fn {
        params: vec![v],
        ret: v,
    };
    // v0.105 (slice 3b-ii): a held `Map[K, Connection]` stores connection ids and
    // resolves them by identity; `update`/`upsert` transform the value through a
    // `(V) -> V` function, which has no meaning for a held resource — you cannot
    // derive a new connection from an old one. Reject them (the other entry ops —
    // `put`/`get`/`remove`/`contains`/`size` — are admitted) so the program is a
    // clean compile error rather than a silent miscompile.
    if v.is_held(tys) && matches!(method.name.as_str(), "update" | "upsert") {
        ctx.errors.push(
            CompileError::new(
                "bynk.held.unsupported_map_op",
                method.span,
                format!(
                    "a held `Map[K, Connection]` has no `{}` operation — a held resource cannot be transformed by a `(Connection) -> Connection` function",
                    method.name
                ),
            )
            .with_note(
                "held connections are stored and resolved by identity; use `put`/`get`/`remove`",
            ),
        );
        for a in args {
            type_of(a, None, ctx);
        }
        return None;
    }
    let (expected, result): (Vec<TyId>, TyId) = match method.name.as_str() {
        "put" => (vec![k, v], tys.intern(Ty::Unit)),
        "get" => (vec![k], tys.intern(Ty::Option(v))),
        "remove" => (vec![k], tys.intern(Ty::Unit)),
        "contains" => (vec![k], tys.intern(Ty::Base(BaseType::Bool))),
        "size" => (vec![], tys.intern(Ty::Base(BaseType::Int))),
        "update" => (vec![k, tys.intern(vfn())], tys.intern(Ty::Unit)),
        "upsert" => (vec![k, v, tys.intern(vfn())], tys.intern(Ty::Unit)),
        other => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.store.unknown_op",
                    method.span,
                    format!(
                        "a `Map` store field has no operation `{other}` — expected `put`, `get`, \
                         `update`, `upsert`, `remove`, `contains`, or `size`"
                    ),
                )
                .with_note("storage-map ops are entry-level and effectful (await with `<-`)"),
            );
            for a in args {
                type_of(a, None, ctx);
            }
            return None;
        }
    };
    let effect = Ty::Effect(result);
    if args.len() != expected.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.call_arity",
            span,
            format!(
                "`Map.{}` takes {} argument(s), found {}",
                method.name,
                expected.len(),
                args.len()
            ),
        ));
        for a in args {
            type_of(a, None, ctx);
        }
        return Some(tys.intern(effect));
    }
    for (a, exp) in args.iter().zip(expected.iter()) {
        if let Some(at) = type_of(a, Some(*exp), ctx)
            && !compatible(at, *exp, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                a.span,
                format!(
                    "expected `{}`, found `{}`",
                    exp.display(tys),
                    at.display(tys)
                ),
            ));
        }
    }
    Some(tys.intern(effect))
}

/// v0.99: record a capability requirement at `site` into the ledger, and — when
/// the enclosing handler's `given` does not cover it — push the diagnostic. The
/// single producer behind both the bare diagnostic and the editor surfaces
/// (DECISION D): every requirement is *recorded*, covered or not; only an
/// uncovered one errors. The `code`/`message` are the consuming feature's, so a
/// store op keeps its precise diagnostic; the ledger's *reason* renders from
/// `source` alone (DECISION C), never from `code`.
fn require_capability(
    site: Span,
    capability: &str,
    source: RequirementSource,
    ctx: &mut Ctx,
    code: &'static str,
    message: &str,
) {
    let covered = ctx.caps.capabilities.contains_key(capability);
    if covered {
        ctx.caps.given_used.insert(capability.to_string());
    } else {
        ctx.errors
            .push(CompileError::new(code, site, message).with_note(format!(
                "add `{capability}` to the handler's `given` clause"
            )));
    }
    record_requirement(ctx, capability, site, source, covered);
}

/// Push a [`Requirement`] into the ledger. For an uncovered requirement it also
/// computes the materialization edit (the ghost `given` inlay hint's one-click
/// apply) from the handler's existing `given` entries and anchor — the same
/// `given_insertion_edit` the undeclared-capability quick-fix uses.
fn record_requirement(
    ctx: &mut Ctx,
    capability: &str,
    site: Span,
    source: RequirementSource,
    covered: bool,
) {
    let materialize = if covered {
        None
    } else {
        given_insertion_edit(&ctx.caps.given_entries, ctx.caps.given_anchor, capability).map(
            |(edit_span, edit_text)| Materialize {
                anchor: ctx.caps.given_anchor.unwrap_or(ctx.return_ty_span),
                edit_span,
                edit_text,
            },
        )
    };
    ctx.requirements.record(Requirement {
        capability: capability.to_string(),
        site,
        source,
        covered,
        materialize,
    });
}

/// v0.87 (ADR 0113): resolve a storage-`Cache` operation `<cache>.<op>(args)` on
/// a `store Cache[K, V]` field. The op set is the storage `Map`'s
/// (`put`/`get`/`update`/`upsert`/`remove`/`contains`/`size`); every op but
/// `remove` additionally requires `given Clock` (eviction reads the clock).
pub(crate) fn check_store_cache_op(
    method: &Ident,
    args: &[Expr],
    k: TyId,
    v: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let vfn = || Ty::Fn {
        params: vec![v],
        ret: v,
    };
    let (expected, result): (Vec<TyId>, TyId) = match method.name.as_str() {
        "put" => (vec![k, v], tys.intern(Ty::Unit)),
        "get" => (vec![k], tys.intern(Ty::Option(v))),
        "remove" => (vec![k], tys.intern(Ty::Unit)),
        "contains" => (vec![k], tys.intern(Ty::Base(BaseType::Bool))),
        "size" => (vec![], tys.intern(Ty::Base(BaseType::Int))),
        "update" => (vec![k, tys.intern(vfn())], tys.intern(Ty::Unit)),
        "upsert" => (vec![k, v, tys.intern(vfn())], tys.intern(Ty::Unit)),
        other => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.store.unknown_op",
                    method.span,
                    format!(
                        "a `Cache` store field has no operation `{other}` — expected `put`, \
                         `get`, `update`, `upsert`, `remove`, `contains`, or `size`"
                    ),
                )
                .with_note("storage-cache ops are entry-level and effectful (await with `<-`)"),
            );
            for a in args {
                type_of(a, None, ctx);
            }
            return None;
        }
    };
    // D4: every op but `remove` reads the clock for TTL expiry.
    if method.name != "remove" {
        require_capability(
            method.span,
            "Clock",
            RequirementSource::StoreOp {
                kind: StoreKind::Cache,
                op: method.name.clone(),
            },
            ctx,
            "bynk.store.cache_needs_clock",
            "a `Cache` operation applies TTL expiry, which reads the clock — the handler must declare `given Clock`",
        );
    }
    let effect = Ty::Effect(result);
    if args.len() != expected.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.call_arity",
            span,
            format!(
                "`Cache.{}` takes {} argument(s), found {}",
                method.name,
                expected.len(),
                args.len()
            ),
        ));
        for a in args {
            type_of(a, None, ctx);
        }
        return Some(tys.intern(effect));
    }
    for (a, exp) in args.iter().zip(expected.iter()) {
        if let Some(at) = type_of(a, Some(*exp), ctx)
            && !compatible(at, *exp, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                a.span,
                format!(
                    "expected `{}`, found `{}`",
                    exp.display(tys),
                    at.display(tys)
                ),
            ));
        }
    }
    Some(tys.intern(effect))
}

/// v0.95 (ADR 0121): resolve a storage-`Log` operation `<log>.<op>(args)` on a
/// `store Log[T]` field. `append(e)` is the effectful, **non-idempotent** write
/// (`Effect[()]`) and the one clock-consuming op — it stamps `Clock.now()`, so it
/// requires `given Clock`. The time-window roots — `since(Instant)`/
/// `before(Instant)` / `between(Instant, Instant)` / `recent(Int)` / `reversed()`
/// — and the general query vocabulary lift the log into a lazy `Query[T]` over its
/// entry values; these need no clock (window bounds are explicit `Instant`s).
pub(crate) fn check_store_log_op(
    method: &Ident,
    args: &[Expr],
    elem: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let query = || Ty::Query(elem);
    let arity = |n: usize, ctx: &mut Ctx| {
        if args.len() != n {
            ctx.errors.push(CompileError::new(
                "bynk.types.call_arity",
                span,
                format!(
                    "`Log.{}` takes {n} argument(s), found {}",
                    method.name,
                    args.len()
                ),
            ));
            for a in args {
                type_of(a, None, ctx);
            }
            return false;
        }
        true
    };
    let window_arg = |a: &Expr, what: &str, ctx: &mut Ctx| {
        if let Some(at) = type_of(a, Some(tys.intern(Ty::Base(BaseType::Instant))), ctx)
            && !compatible(at, tys.intern(Ty::Base(BaseType::Instant)), tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                a.span,
                format!("{what} expects `Instant`, found `{}`", at.display(tys)),
            ));
        }
    };
    match method.name.as_str() {
        // The one effectful write — non-idempotent; stamps the clock.
        "append" => {
            require_capability(
                method.span,
                "Clock",
                RequirementSource::StoreOp {
                    kind: StoreKind::Log,
                    op: method.name.clone(),
                },
                ctx,
                "bynk.store.log_needs_clock",
                "`Log.append` stamps the current time, which reads the clock — the handler must declare `given Clock`",
            );
            if !arity(1, ctx) {
                return Some(tys.intern(Ty::Effect(tys.intern(Ty::Unit))));
            }
            if let Some(at) = type_of(&args[0], Some(elem), ctx)
                && !compatible(at, elem, tys)
            {
                ctx.errors.push(CompileError::new(
                    "bynk.types.argument_mismatch",
                    args[0].span,
                    format!(
                        "expected `{}`, found `{}`",
                        elem.display(tys),
                        at.display(tys)
                    ),
                ));
            }
            Some(tys.intern(Ty::Effect(tys.intern(Ty::Unit))))
        }
        // Time-window query roots → `Query[T]` (lazy; no clock).
        "since" | "before" => {
            if !arity(1, ctx) {
                return Some(tys.intern(query()));
            }
            window_arg(&args[0], &format!("`Log.{}`", method.name), ctx);
            Some(tys.intern(query()))
        }
        "between" => {
            if !arity(2, ctx) {
                return Some(tys.intern(query()));
            }
            window_arg(&args[0], "`Log.between` start", ctx);
            window_arg(&args[1], "`Log.between` end", ctx);
            Some(tys.intern(query()))
        }
        "recent" => {
            if !arity(1, ctx) {
                return Some(tys.intern(query()));
            }
            check_arg(
                &args[0],
                tys.intern(Ty::Base(BaseType::Int)),
                "the `Log.recent` count",
                ctx,
            );
            Some(tys.intern(query()))
        }
        "reversed" => {
            if !arity(0, ctx) {
                return Some(tys.intern(query()));
            }
            Some(tys.intern(query()))
        }
        // The general query vocabulary over the entry values.
        name if is_query_op(name) => check_query_kernel_method(method, args, elem, span, ctx),
        other => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.store.unknown_op",
                    method.span,
                    format!(
                        "a `Log` store field has no operation `{other}` — `append`, the \
                         time-window roots (`since`/`before`/`between`/`recent`/`reversed`), \
                         and the query builders/terminals"
                    ),
                )
                .with_note(
                    "`Log` reads are lazy `Query[T]`; only `append` is effectful and writes",
                ),
            );
            for a in args {
                type_of(a, None, ctx);
            }
            None
        }
    }
}

/// v0.83: resolve a storage-set operation `<set>.<op>(args)` on a `store Set[T]`
/// field. Effect-typed entry ops: `add(t)`/`remove(t)` → `Effect[()]` (both
/// idempotent), `contains(t)` → `Effect[Bool]`, `size()` → `Effect[Int]`. Set
/// algebra (`union`/`intersection`/`difference`) is deferred (it needs a value
/// `Set` return type). Dispatched by receiver provenance.
pub(crate) fn check_store_set_op(
    method: &Ident,
    args: &[Expr],
    t: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let (expected, result): (Vec<TyId>, TyId) = match method.name.as_str() {
        "add" => (vec![t], tys.intern(Ty::Unit)),
        "remove" => (vec![t], tys.intern(Ty::Unit)),
        "contains" => (vec![t], tys.intern(Ty::Base(BaseType::Bool))),
        "size" => (vec![], tys.intern(Ty::Base(BaseType::Int))),
        other => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.store.unknown_op",
                    method.span,
                    format!(
                        "a `Set` store field has no operation `{other}` — expected `add`, \
                         `remove`, `contains`, or `size`"
                    ),
                )
                .with_note(
                    "set algebra (`union`/`intersection`/`difference`) is not in this slice",
                ),
            );
            for a in args {
                type_of(a, None, ctx);
            }
            return None;
        }
    };
    let effect = Ty::Effect(result);
    if args.len() != expected.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.call_arity",
            span,
            format!(
                "`Set.{}` takes {} argument(s), found {}",
                method.name,
                expected.len(),
                args.len()
            ),
        ));
        for a in args {
            type_of(a, None, ctx);
        }
        return Some(tys.intern(effect));
    }
    for (a, exp) in args.iter().zip(expected.iter()) {
        if let Some(at) = type_of(a, Some(*exp), ctx)
            && !compatible(at, *exp, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                a.span,
                format!(
                    "expected `{}`, found `{}`",
                    exp.display(tys),
                    at.display(tys)
                ),
            ));
        }
    }
    Some(tys.intern(effect))
}

/// v0.98 (ADR 0125): resolve a storage-`Cell` operation `<cell>.<op>(args)` on a
/// `store Cell[T]` field. The single method-shaped cell op is `update(f)` —
/// `f: (T) -> T` — a read-modify-write typed `Effect[()]`. Reading a cell is the
/// bare-name sugar and writing it is `:=`, so `read`/`write` are not callable
/// methods (DECISION B). The combiner is a non-effectful `Ty::Fn`, so an
/// effectful body (including a bare read of another cell) fails the existing
/// function-type check (DECISION E). Dispatched by receiver provenance.
pub(crate) fn check_store_cell_op(
    method: &Ident,
    args: &[Expr],
    t: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let tfn = || Ty::Fn {
        params: vec![t],
        ret: t,
    };
    let (expected, result): (Vec<TyId>, TyId) = match method.name.as_str() {
        "update" => (vec![tys.intern(tfn())], tys.intern(Ty::Unit)),
        other => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.store.unknown_op",
                    method.span,
                    format!("a `Cell` store field has no operation `{other}` — expected `update`"),
                )
                .with_note(
                    "a cell is read by its bare name and written with `:=`; `update` is the only \
                     method-shaped op",
                ),
            );
            for a in args {
                type_of(a, None, ctx);
            }
            return None;
        }
    };
    let effect = Ty::Effect(result);
    if args.len() != expected.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.call_arity",
            span,
            format!(
                "`Cell.{}` takes {} argument(s), found {}",
                method.name,
                expected.len(),
                args.len()
            ),
        ));
        for a in args {
            type_of(a, None, ctx);
        }
        return Some(tys.intern(effect));
    }
    for (a, exp) in args.iter().zip(expected.iter()) {
        if let Some(at) = type_of(a, Some(*exp), ctx)
            && !compatible(at, *exp, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                a.span,
                format!(
                    "expected `{}`, found `{}`",
                    exp.display(tys),
                    at.display(tys)
                ),
            ));
        }
    }
    Some(tys.intern(effect))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn check_method_call(
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    expected: Option<TyId>,
    // P6.0 (#1139): the outer `MethodCall` expression's own identity.
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // v0.22b: explicit type arguments apply only to the `Json.decode[T]`
    // static — every other method/static takes none (the 0039/0045 rule). A
    // user-declared type named `Json` shadows the codec module and takes no type
    // arguments. #594: a generic *user* instance method infers its type
    // arguments from the receiver and the argument types — the explicit
    // `x.map[U](…)` form is deferred, so explicit type args are rejected here.
    // #926: a capability operation may declare its own type parameter(s),
    // resolved only from an explicit call-site type argument (never
    // inferred) — the same explicit-only discipline as `Json.decode[T]`, so
    // it joins that carve-out rather than `#594`'s inference. Covers a bare
    // local/declared capability receiver and a cross-context one (flattened
    // `Cap.op[T](…)` or qualified `B.Cap.op[T](…)`); arity/substitution
    // happens downstream in `check_static_call` /
    // `check_cross_context_capability_call`.
    let receiver_is_capability = matches!(&receiver.kind, ExprKind::Ident(id)
            if ctx.caps.capabilities.contains_key(&id.name)
                || ctx.caps.declared_capabilities.contains_key(&id.name))
        || flatten_ident_chain(receiver).is_some_and(|chain| {
            ctx.input
                .cross_context
                .resolve_cross_capability(&chain)
                .is_some()
        });
    if !type_args.is_empty()
        && !matches!(&receiver.kind, ExprKind::Ident(id) if id.name == JSON
            && !ctx.input.types.contains_key(JSON))
        && !receiver_is_capability
    {
        ctx.errors.push(CompileError::new(
            "bynk.generics.type_arg_mismatch",
            span,
            format!(
                "`{}` does not take explicit type arguments — a generic method infers them from the receiver and arguments",
                method.name
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    // v0.25 / v0.178: a test body invokes the target's service as
    // `svc.call(args)`. The emitter wires the call from the same service set;
    // the checker resolves the service's `on call` handler and verifies the
    // call's arity and argument types (v0.178, #662 — before this, the branch
    // matched the literal method name `call` without checking the handler
    // exists, so `api.call(…)` on a `from http` service passed `check` and
    // crashed at runtime, #654). #1934: the call is typed by what the handler
    // yields, and the binding edge is recorded for test-file references. Returns here: the
    // resolution must not fall through to the receiver walk, which would
    // report the service name as unknown (#504 backstop).
    if let ExprKind::Ident(id) = &receiver.kind
        && ctx.lookup(id.name.as_str()).is_none()
        && let Some(sig) = ctx.test_services.get(&id.name).cloned()
    {
        if let Some(unit) = ctx.input.cross_context.self_context.clone() {
            ctx.refs
                .record_in_unit(id.span, SymbolKind::Service, &id.name, &unit);
        }
        return check_test_service_address(&sig, id, method, args, expr_id, ctx);
    }
    // v0.6: cross-context service call. Two shapes:
    //   - `Alias.service(args)`           where Alias is from `consumes X as Alias`
    //   - `prefix.tail.service(args)`     where `prefix.tail` is a consumed context's
    //                                     qualified name (parsed as nested FieldAccess).
    // The full-qualified-name form must be checked before the bare-ident form
    // (the prefix's first segment doesn't resolve as anything local).
    if ctx.lookup_root_ident(receiver).is_none() && !ctx.root_ident_is_store_field(receiver) {
        // v0.15: cross-context capability call — `B.Cap.op(args)` /
        // `Alias.Cap.op(args)`. Checked before the service-call shape because
        // the receiver carries an extra (capability) segment.
        if let Some(chain) = flatten_ident_chain(receiver)
            && let Some((consumed, cap)) = ctx.input.cross_context.resolve_cross_capability(&chain)
        {
            // v0.25: the capability name-segment (`Cap` in `B.Cap` /
            // `Alias.Cap`) is the outermost field of the receiver chain.
            if let ExprKind::FieldAccess { field, .. } = &receiver.kind {
                ctx.refs
                    .record_in_unit(field.span, SymbolKind::Capability, &cap, &consumed);
            }
            return check_cross_context_capability_call(
                receiver, &consumed, &cap, method, type_args, args, span, expr_id, ctx,
            );
        }
        if let Some(consumed) = cross_context_prefix(receiver, ctx) {
            return check_cross_context_call(receiver, &consumed, method, args, span, expr_id, ctx);
        }
        // Looks like a dotted prefix (no local binding for the root). If the
        // chain matches the shape of a consumed-context call but the prefix
        // isn't actually consumed, surface an explicit diagnostic so the user
        // can fix the missing `consumes` clause rather than seeing a silent
        // "no methods" error later.
        //
        // #1930: not for a chain rooted in an asset (`sheet.url.startsWith(…)`),
        // which is a method on the asset's `url`. Only here, after both
        // resolutions above failed: an asset named like a consumed context's
        // root segment (`shop` beside `consumes shop.payment`) still lets a
        // real cross-context call resolve (review of #1937).
        if let ExprKind::FieldAccess { .. } = &receiver.kind
            && !ctx.root_ident_is_asset(receiver)
            && let Some(chain) = flatten_ident_chain(receiver)
            && chain.contains('.')
        {
            let info = &ctx.input.cross_context;
            let in_context = info.self_context.is_some();
            if in_context && info.resolve_prefix(&chain).is_none() {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.resolve.unconsumed_context",
                        receiver.span,
                        format!(
                            "`{chain}.{}` looks like a cross-context service call, but `{chain}` is not in this context's `consumes` clauses",
                            method.name
                        ),
                    )
                    .with_note(
                        "add a `consumes {chain}` clause at the top of the context, or use an alias and call it through the alias",
                    ),
                );
                for a in args {
                    let _ = type_of(a, None, ctx);
                }
                return None;
            }
        }
    }
    // Detect capability call (v0.5): receiver is a bare Ident naming a
    // capability declared in the context (in scope via `given`, or declared
    // but undeclared in `given` — the static-call path emits the error).
    if let ExprKind::Ident(id) = &receiver.kind
        && ctx.lookup(id.name.as_str()).is_none()
        && (ctx.caps.capabilities.contains_key(&id.name)
            || ctx.caps.declared_capabilities.contains_key(&id.name))
    {
        return check_static_call(id, method, type_args, args, span, expected, expr_id, ctx);
    }
    // Detect static-call shape: receiver is a bare Ident naming a declared
    // type (not a local/param). Dispatch to check_static_call. `type_args` is
    // always empty here — `check_method_call`'s gate above only admits a
    // non-empty slice for a capability (handled just above) or the `Json`
    // codec (handled separately, `check_json_static`).
    if let ExprKind::Ident(id) = &receiver.kind
        && ctx.lookup(id.name.as_str()).is_none()
        && ctx.input.types.contains_key(&id.name)
    {
        return check_static_call(id, method, type_args, args, span, expected, expr_id, ctx);
    }
    // v0.20b: qualified statics on the built-in collection types —
    // `List.empty()` / `Map.empty()`. Like an empty `[]`, they need an
    // expected type to pin their element/key/value types.
    if let ExprKind::Ident(id) = &receiver.kind
        && ctx.lookup(id.name.as_str()).is_none()
        && !ctx.input.types.contains_key(&id.name)
        && (id.name == LIST || id.name == MAP || id.name == SET)
    {
        let ns = if id.name == LIST {
            LIST
        } else if id.name == MAP {
            MAP
        } else {
            SET
        };
        ctx.callees.insert(
            expr_id,
            Callee::Intrinsic {
                ns,
                op: method.name.clone(),
            },
        );
        return check_collection_static(id, method, args, span, expected, ctx);
    }
    // v0.22a: the numeric parse statics — `Int.parse(s)` / `Float.parse(s)`.
    // The parser only admits these keywords in receiver position when
    // followed by `.`, so the Ident shape here is exactly the static form.
    if let ExprKind::Ident(id) = &receiver.kind
        && (id.name == INT || id.name == FLOAT)
    {
        let ns = if id.name == INT { INT } else { FLOAT };
        ctx.callees.insert(
            expr_id,
            Callee::Intrinsic {
                ns,
                op: method.name.clone(),
            },
        );
        return check_numeric_parse_static(id, method, args, span, ctx);
    }
    // #2622: the `Decimal` statics — `Decimal.parse(s)` / `Decimal.fromInt(n)`.
    if let ExprKind::Ident(id) = &receiver.kind
        && id.name == DECIMAL
        && ctx.lookup(DECIMAL).is_none()
        && !ctx.input.types.contains_key(DECIMAL)
    {
        ctx.callees.insert(
            expr_id,
            Callee::Intrinsic {
                ns: DECIMAL,
                op: method.name.clone(),
            },
        );
        return check_decimal_static(method, args, span, ctx);
    }
    // v0.86 (ADR 0112): the `Duration.millis(n)` static constructor — the way to
    // build a `Duration` from a runtime `Int` (the literal covers constants).
    if let ExprKind::Ident(id) = &receiver.kind
        && id.name == DURATION
        && ctx.lookup(DURATION).is_none()
        && !ctx.input.types.contains_key(DURATION)
    {
        ctx.callees.insert(
            expr_id,
            Callee::Intrinsic {
                ns: DURATION,
                op: method.name.clone(),
            },
        );
        return check_duration_static(method, args, span, ctx);
    }
    // v0.90 (ADR 0114 D6): the `Instant.fromEpochMillis(n)` static constructor.
    if let ExprKind::Ident(id) = &receiver.kind
        && id.name == INSTANT
        && ctx.lookup(INSTANT).is_none()
        && !ctx.input.types.contains_key(INSTANT)
    {
        ctx.callees.insert(
            expr_id,
            Callee::Intrinsic {
                ns: INSTANT,
                op: method.name.clone(),
            },
        );
        return check_instant_static(method, args, span, ctx);
    }
    // v0.110 (ADR 0142 D2): the `Bytes` static constructors —
    // `Bytes.fromUtf8(s)` / `Bytes.fromBase64(s)` / `Bytes.empty()`.
    if let ExprKind::Ident(id) = &receiver.kind
        && id.name == BYTES
        && ctx.lookup(BYTES).is_none()
        && !ctx.input.types.contains_key(BYTES)
    {
        ctx.callees.insert(
            expr_id,
            Callee::Intrinsic {
                ns: BYTES,
                op: method.name.clone(),
            },
        );
        return check_bytes_static(method, args, span, ctx);
    }
    // #2380 (L5): `TokenClaims.of(lines)` — how a test builds the claims an
    // `Oidc` actor's handler reads.
    if let ExprKind::Ident(id) = &receiver.kind
        && id.name == TOKEN_CLAIMS
        && ctx.lookup(TOKEN_CLAIMS).is_none()
        && !ctx.input.types.contains_key(TOKEN_CLAIMS)
    {
        ctx.callees.insert(
            expr_id,
            Callee::Intrinsic {
                ns: TOKEN_CLAIMS,
                op: method.name.clone(),
            },
        );
        return check_token_claims_static(method, args, span, ctx);
    }
    // #2341 (L4): `RequestHeaders.of(lines)` / `RequestHeaders.empty()` — how
    // a test builds the headers an HTTP handler reads.
    if let ExprKind::Ident(id) = &receiver.kind
        && id.name == REQUEST_HEADERS
        && ctx.lookup(REQUEST_HEADERS).is_none()
        && !ctx.input.types.contains_key(REQUEST_HEADERS)
    {
        ctx.callees.insert(
            expr_id,
            Callee::Intrinsic {
                ns: REQUEST_HEADERS,
                op: method.name.clone(),
            },
        );
        return check_request_headers_static(method, args, span, ctx);
    }
    // v0.22b: the typed JSON codec statics (ADR 0045).
    if let ExprKind::Ident(id) = &receiver.kind
        && id.name == JSON
        && ctx.lookup(JSON).is_none()
        && !ctx.input.types.contains_key(JSON)
    {
        ctx.callees.insert(
            expr_id,
            Callee::Intrinsic {
                ns: JSON,
                op: method.name.clone(),
            },
        );
        return check_json_static(method, type_args, args, span, expected, ctx);
    }
    // v0.100: the `Stream.of(xs)` static constructor (real-time track slice 0).
    if let ExprKind::Ident(id) = &receiver.kind
        && id.name == STREAM
        && ctx.lookup(STREAM).is_none()
        && !ctx.input.types.contains_key(STREAM)
    {
        ctx.callees.insert(
            expr_id,
            Callee::Intrinsic {
                ns: STREAM,
                op: method.name.clone(),
            },
        );
        return check_stream_static(method, args, span, ctx);
    }
    // v0.20b: `insert`/`prepend` return their receiver's collection type —
    // propagate an expected collection type down the chain so
    // `let m: Map[String, Int] = Map.empty().insert("a", 1)` infers. #1889:
    // `append`/`concat` return their receiver's list type too.
    let recv_expected = match (expected, method.name.as_str()) {
        (Some(t), "insert") => peel_to_map(t, tys)
            .map(|(k, v)| tys.intern(Ty::Map(k, v)))
            .or_else(|| peel_to_set(t, tys).map(|e| tys.intern(Ty::Set(e)))),
        (Some(t), "remove") => peel_to_set(t, tys).map(|e| tys.intern(Ty::Set(e))),
        (Some(t), "prepend" | "append" | "concat") => {
            peel_to_list(t, tys).map(|e| tys.intern(Ty::List(e)))
        }
        _ => None,
    };
    let recv_ty = type_of(receiver, recv_expected, ctx)?;
    // v0.20b: built-in kernel methods on the collection types. These are
    // compiler-known special forms typed directly here — generic in their
    // accumulator without the (deferred) declared-generic-methods feature;
    // the deferral bites only on declared methods (ADR 0037).
    match &*tys.get(recv_ty) {
        Ty::List(elem) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_list_kernel_method(method, args, *elem, span, ctx);
        }
        // v0.91 (ADR 0115): a chained builder/terminal on a lazy `Query[T]`.
        Ty::Query(elem) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_query_kernel_method(method, args, *elem, span, ctx);
        }
        // v0.100: a chained builder/terminal on a `Stream[T]`.
        Ty::Stream(elem) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_stream_kernel_method(method, args, *elem, span, ctx);
        }
        // v0.102: the held-resource operations on a `Connection[F]` — `send(f)`
        // (non-consuming) and `close()` (consuming). The linearity pass tracks
        // the ownership transitions; this types the operations.
        Ty::Connection(frame) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_connection_method(method, args, *frame, span, ctx);
        }
        Ty::Map(key, val) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_map_kernel_method(method, args, *key, *val, span, ctx);
        }
        // K6a (#2596): the `Set` kernel.
        Ty::Set(elem) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_set_kernel_method(method, args, *elem, span, ctx);
        }
        // v0.21: the numeric kernel — conversions as value methods on the
        // bare base types. A refined value reaches these too, but via the
        // refined-receiver fallback below (after its own declared methods),
        // not via any `.raw` surface — a refined type's only source-level
        // constructors are `.of` and literal admission (ADR 0182); unlike an
        // opaque type it exposes no `.unsafe` (that is opaque-only, confined
        // to the defining commons).
        Ty::Base(base @ (BaseType::Int | BaseType::Float)) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_numeric_kernel_method(method, args, *base, span, ctx);
        }
        // v0.86 (ADR 0112): the `Duration` kernel — `toMillis`/`toString`.
        Ty::Base(BaseType::Duration) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_duration_kernel_method(method, args, span, ctx);
        }
        // v0.90 (ADR 0114): the `Instant` kernel — `toEpochMillis`/`toString`.
        Ty::Base(BaseType::Instant) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_instant_kernel_method(method, args, span, ctx);
        }
        // #2341 (decision A): an HTTP handler's `headers` — `get`, `bearer`,
        // `clientIp`.
        Ty::RequestHeaders => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_request_headers_method(method, args, span, ctx);
        }
        // #2380 (L5): an `Oidc` binder's claims — `get`, `has`.
        Ty::TokenClaims => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_token_claims_method(method, args, span, ctx);
        }
        // v0.110 (ADR 0142): the `Bytes` kernel — `length`/`toBase64`/`decodeUtf8`.
        Ty::Base(BaseType::Bytes) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_bytes_kernel_method(method, args, span, ctx);
        }
        // #2622: the `Decimal` kernel.
        Ty::Base(BaseType::Decimal) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_decimal_kernel_method(method, args, span, ctx);
        }
        // v0.22a: the string kernel (ADR 0046).
        Ty::Base(BaseType::String) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_string_kernel_method(method, args, span, ctx);
        }
        // v0.22a: the Option/Result combinators as kernel methods (ADR 0048).
        Ty::Option(inner) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_option_kernel_method(method, args, *inner, span, ctx);
        }
        Ty::Result(ok, err) => {
            ctx.callees.insert(
                expr_id,
                Callee::Kernel {
                    recv: recv_ty,
                    op: method.name.clone(),
                },
            );
            return check_result_kernel_method(method, args, *ok, *err, span, ctx);
        }
        // The `Effect[Result[T, E]]` combinators (§2.8.3): `mapOk`/`mapErr`/
        // `flatMapOk`/`flatMapErr` on the universal cross-context shape. Only an
        // `Effect` wrapping a `Result` has methods; any other `Effect[_]` falls
        // through to the "no methods" error below.
        Ty::Effect(inner) => {
            // #2683: `within` is on every `Effect[T]`, and wins over the
            // `Effect[Result]` combinators (it is not one of them), so
            // `Effect[Result[T, E]].within(d)` is the plain nested case.
            if method.name == "within" {
                ctx.callees.insert(
                    expr_id,
                    Callee::Kernel {
                        recv: recv_ty,
                        op: method.name.clone(),
                    },
                );
                return check_effect_within_method(method, args, *inner, span, ctx);
            }
            // #2711: `race` is on every `Effect[T]` too.
            if method.name == "race" {
                ctx.callees.insert(
                    expr_id,
                    Callee::Kernel {
                        recv: recv_ty,
                        op: method.name.clone(),
                    },
                );
                return check_effect_race_method(args, *inner, span, ctx);
            }
            if let Ty::Result(ok, err) = &*tys.get(*inner) {
                ctx.callees.insert(
                    expr_id,
                    Callee::Kernel {
                        recv: recv_ty,
                        op: method.name.clone(),
                    },
                );
                return check_effect_result_kernel_method(method, args, *ok, *err, span, ctx);
            }
        }
        _ => {}
    }
    // Find a named type for the receiver, then look up its instance methods.
    let type_name = match &*tys.get(recv_ty) {
        Ty::Named { name, .. } => name.clone(),
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_on_non_named_type",
                method.span,
                format!(
                    "type `{}` has no methods — only user-declared types support method calls",
                    recv_ty.display(tys)
                ),
            ));
            return None;
        }
    };
    // Agent handler dispatch: when the receiver is an agent instance, look
    // up the method against the agent's declared `on call` handlers and
    // resolve to the handler's return type.
    if let Some(agent) = ctx.input.agents.get(&type_name).cloned() {
        let Some(handler) = agent.handlers.iter().find(|h| {
            h.method_name
                .as_ref()
                .is_some_and(|n| n.name == method.name)
        }) else {
            ctx.errors.push(CompileError::new(
                "bynk.agent.handler_not_found",
                method.span,
                format!(
                    "agent `{}` has no handler named `{}`",
                    type_name, method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        };
        ctx.callees.insert(
            expr_id,
            Callee::Agent {
                agent: type_name.clone(),
                handler: method.name.clone(),
            },
        );
        // #304: the handler is a first-class index symbol, keyed by the
        // compound `"Agent.handler"` name — same convention and same
        // recorded-regardless-of-downstream-errors placement as the
        // ordinary instance-method call below.
        ctx.refs.record(
            method.span,
            SymbolKind::Handler,
            &format!("{type_name}.{}", method.name),
        );
        if handler.params.len() != args.len() {
            ctx.errors.push(CompileError::new(
                "bynk.agent.handler_arity",
                method.span,
                format!(
                    "agent handler `{}.{}` expects {} argument(s), but {} were given",
                    type_name,
                    method.name,
                    handler.params.len(),
                    args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        }
        for (p, arg) in handler.params.iter().zip(args.iter()) {
            let pty = resolve_type_ref(&p.type_ref, &ctx.input.types, tys);
            let arg_ty = type_of(arg, pty, ctx);
            if let (Some(a), Some(p_ty)) = (arg_ty, pty.as_ref())
                && !compatible(a, *p_ty, tys)
            {
                ctx.errors.push(CompileError::new(
                    "bynk.types.argument_mismatch",
                    arg.span,
                    format!(
                        "argument has type `{}`, but `{}.{}` expects `{}`",
                        a.display(tys),
                        type_name,
                        method.name,
                        p_ty.display(tys)
                    ),
                ));
            }
        }
        return resolve_type_ref(&handler.return_type, &ctx.input.types, tys);
    }
    let table = ctx
        .input
        .methods
        .get(&type_name)
        .cloned()
        .unwrap_or_default();
    let Some(method_decl) = table.instance.get(&method.name).cloned() else {
        // #561: a refined receiver inherits its base type's read-only kernel
        // methods as a fallback *after* its own declared methods (DECISION D).
        // A refined value erases to its base at runtime, so the base methods
        // already apply bit-for-bit; only the checker had to be taught to look.
        // Results are base-typed (DECISION B) and refined arguments widen via
        // `compatible`, so the base kernel checkers need no change. The match
        // is `Refined` only — opaque types deliberately do not widen, so they
        // keep reporting `method_not_found`. `Bool` (and any base without a
        // kernel) inherits nothing and falls through to the same error.
        if let Ty::Named {
            kind: NamedKind::Refined(base),
            ..
        } = &*tys.get(recv_ty)
        {
            if !matches!(base, BaseType::Bool) {
                ctx.callees.insert(
                    expr_id,
                    Callee::Kernel {
                        recv: recv_ty,
                        op: method.name.clone(),
                    },
                );
            }
            match base {
                BaseType::Int | BaseType::Float => {
                    return check_numeric_kernel_method(method, args, *base, span, ctx);
                }
                BaseType::String => return check_string_kernel_method(method, args, span, ctx),
                BaseType::Duration => return check_duration_kernel_method(method, args, span, ctx),
                BaseType::Instant => return check_instant_kernel_method(method, args, span, ctx),
                BaseType::Bytes => return check_bytes_kernel_method(method, args, span, ctx),
                BaseType::Decimal => return check_decimal_kernel_method(method, args, span, ctx),
                BaseType::Bool => {}
            }
        }
        ctx.errors.push(CompileError::new(
            "bynk.types.method_not_found",
            method.span,
            format!(
                "type `{}` has no instance method named `{}`",
                type_name, method.name
            ),
        ));
        return None;
    };
    // v0.36 (ADR 0069): the method is a first-class index symbol, keyed by the
    // compound `"Type.method"` name. Recorded already-spelled from the resolved
    // receiver type; the bare edge resolves through the same `uses`/`consumes`
    // qualification as cross-file type references.
    ctx.refs.record(
        method.span,
        SymbolKind::Method,
        &format!("{type_name}.{}", method.name),
    );
    ctx.callees.insert(
        expr_id,
        Callee::Method(Arc::clone(&method_decl), Vec::new()),
    );
    // #594: a generic instance method — one on a generic receiver, or one
    // carrying its own type parameters — resolves its parameter and return
    // types against a substitution seeded from the receiver's type arguments,
    // then infers the method's own parameters from the argument types
    // (argument-directed unification, ADR 0029). A method on a non-generic
    // receiver with no own type parameters takes the plain path below,
    // byte-identically to before.
    let recv_type_params: Vec<String> = ctx
        .input
        .types
        .get(&type_name)
        .map(|d| d.type_params.iter().map(|p| p.name.name.clone()).collect())
        .unwrap_or_default();
    if !recv_type_params.is_empty() || !method_decl.type_params.is_empty() {
        let ret = check_generic_method_call(
            &type_name,
            &recv_type_params,
            recv_ty,
            &method_decl,
            method,
            args,
            ctx,
        );
        // Slice 1: the comparator witnesses the generic-call path resolved.
        if let Some(witnesses) = ctx.last_witness.take() {
            ctx.callees
                .insert(expr_id, Callee::Method(Arc::clone(&method_decl), witnesses));
        }
        return ret;
    }
    // Param count excludes the implicit `self`.
    if method_decl.params.len() != args.len() {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.method_arity",
                method.span,
                format!(
                    "method `{}.{}` expects {} argument(s), but {} were given",
                    type_name,
                    method.name,
                    method_decl.params.len(),
                    args.len()
                ),
            )
            .with_label(method_decl.name.ident().span, "method declared here"),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let mut ok = true;
    for (i, (param, arg)) in method_decl.params.iter().zip(args.iter()).enumerate() {
        record_param_hint(ctx.hints, &param.name.name, arg);
        let expected = resolve_type_ref(&param.type_ref, &ctx.input.types, tys);
        let actual = type_of(arg, expected, ctx);
        let (Some(actual), Some(expected)) = (actual, expected) else {
            ok = false;
            continue;
        };
        if !compatible(actual, expected, tys) {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                arg.span,
                format!(
                    "argument {} to `{}.{}` has type `{}`, but parameter `{}` expects `{}`",
                    i + 1,
                    type_name,
                    method.name,
                    actual.display(tys),
                    param.name.name,
                    expected.display(tys)
                ),
            ));
            ok = false;
        }
    }
    let _ = span;
    if !ok {
        return None;
    }
    resolve_type_ref(&method_decl.return_type, &ctx.input.types, tys)
}

/// #594: type-check a call to a generic instance method — one attached to a
/// generic type (so `self` carries the type's parameters) and/or carrying its
/// own `[U]` parameters. Mirrors [`check_generic_call`] (the free-function
/// path, ADR 0029): resolve the method's parameter/return patterns with the
/// rigid vars in scope, seed the substitution from the receiver's concrete type
/// arguments, then drive argument-directed inference over the method's own
/// parameters. The receiver's parameters are already ground (from the receiver
/// type), so only the method's own parameters need inferring.
fn check_generic_method_call(
    type_name: &str,
    recv_type_params: &[String],
    recv_ty: TyId,
    method_decl: &FnDecl,
    method: &Ident,
    args: &[Expr],
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // Rigid vars: the receiver type's parameters plus the method's own.
    let mut vars: HashSet<String> = recv_type_params.iter().cloned().collect();
    for tp in &method_decl.type_params {
        vars.insert(tp.name.name.clone());
    }
    // Param count excludes the implicit `self`.
    if method_decl.params.len() != args.len() {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.method_arity",
                method.span,
                format!(
                    "method `{}.{}` expects {} argument(s), but {} were given",
                    type_name,
                    method.name,
                    method_decl.params.len(),
                    args.len()
                ),
            )
            .with_label(method_decl.name.ident().span, "method declared here"),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    // Seed the substitution from the receiver's concrete type arguments: the
    // type's parameters are ground the moment the receiver's type is known
    // (`self.map(g)` on a `Box[User]` binds the type's `A` to `User`; on `self`
    // it binds `A` to its own rigid var). An arity mismatch here means the
    // receiver was under-applied — reported where the receiver was typed — so we
    // leave those parameters unseeded and fail below as uninferable.
    let mut subst: HashMap<String, TyId> = HashMap::new();
    if let Ty::Named {
        args: recv_args, ..
    } = &*tys.get(recv_ty)
        && recv_args.len() == recv_type_params.len()
    {
        for (name, arg) in recv_type_params.iter().zip(recv_args.iter()) {
            subst.insert(name.clone(), *arg);
        }
    }
    // Resolve the method's parameter and return patterns with every rigid var in
    // scope (a name matching one resolves to `Ty::Var`, not an unknown type).
    let var_params: Vec<Option<TyId>> = method_decl
        .params
        .iter()
        .map(|p| resolve_type_ref_in(&p.type_ref, &ctx.input.types, &vars, tys))
        .collect();
    let ret_pattern = resolve_type_ref_in(&method_decl.return_type, &ctx.input.types, &vars, tys)?;

    let mut arg_tys: Vec<Option<TyId>> = vec![None; args.len()];
    // Pass 1 — non-lambda arguments (mirrors `check_generic_call`).
    for (i, arg) in args.iter().enumerate() {
        if matches!(arg.kind, ExprKind::Lambda(_)) {
            continue;
        }
        let expected = var_params[i].map(|p| substitute(p, &subst, tys));
        let ty = type_of(arg, expected, ctx);
        if let (Some(pattern), Some(actual)) = (var_params[i], ty)
            && !unify(pattern, actual, &mut subst, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.generics.type_arg_mismatch",
                arg.span,
                format!(
                    "argument {} infers a type for `{}.{}`'s type parameter that conflicts with an earlier argument or the receiver",
                    i + 1,
                    type_name,
                    method.name
                ),
            ));
            return None;
        }
        arg_tys[i] = ty;
    }
    // Pass 2 — lambda arguments, against the now-substituted expecteds.
    for (i, arg) in args.iter().enumerate() {
        if !matches!(arg.kind, ExprKind::Lambda(_)) {
            continue;
        }
        let expected = var_params[i].map(|p| substitute(p, &subst, tys));
        let params_unconstrained = expected.is_some_and(|e| {
            matches!(&*tys.get(e), Ty::Fn { params, .. }
                if params.iter().any(|p| contains_var(*p, tys)))
        });
        let fully_annotated = matches!(
            &arg.kind,
            ExprKind::Lambda(l) if l.params.iter().all(|p| lambda_param_is_typed(p, ctx))
        );
        if params_unconstrained && !fully_annotated {
            ctx.errors.push(
                CompileError::new(
                    "bynk.generics.uninferable_type_arg",
                    arg.span,
                    format!(
                        "the lambda's parameter types depend on `{}.{}`'s type parameters, which the receiver and other arguments do not determine",
                        type_name, method.name
                    ),
                )
                .with_note("annotate the lambda's parameters"),
            );
            return None;
        }
        let ty = if params_unconstrained {
            type_of(arg, None, ctx)
        } else {
            type_of(arg, expected, ctx)
        };
        if let (Some(pattern), Some(actual)) = (var_params[i], ty)
            && !unify(pattern, actual, &mut subst, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.generics.type_arg_mismatch",
                arg.span,
                format!(
                    "the lambda's type conflicts with `{}.{}`'s inferred type arguments",
                    type_name, method.name
                ),
            ));
            return None;
        }
        arg_tys[i] = ty;
    }
    // Every one of the method's own type parameters must now be determined (the
    // receiver's parameters were seeded above).
    for tp in &method_decl.type_params {
        if !subst.contains_key(&tp.name.name) {
            ctx.errors.push(
                CompileError::new(
                    "bynk.generics.uninferable_type_arg",
                    method.span,
                    format!(
                        "type parameter `{}` of `{}.{}` is not inferable from the receiver or the arguments",
                        tp.name.name, type_name, method.name
                    ),
                )
                .with_label(tp.span, "declared here"),
            );
            return None;
        }
    }
    // Final compatibility over the fully-ground parameter types.
    let mut ok = true;
    for (i, (pattern, arg)) in var_params.iter().zip(args).enumerate() {
        record_param_hint(ctx.hints, &method_decl.params[i].name.name, arg);
        let (Some(pattern), Some(arg_ty)) = (pattern, arg_tys[i].as_ref()) else {
            continue;
        };
        let ground = substitute(*pattern, &subst, tys);
        if !compatible(*arg_ty, ground, tys) {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                arg.span,
                format!(
                    "argument {} to `{}.{}` has type `{}`, but `{}` is expected",
                    i + 1,
                    type_name,
                    method.name,
                    arg_ty.display(tys),
                    ground.display(tys)
                ),
            ));
            ok = false;
        }
    }
    if !ok {
        return None;
    }
    // The same call-site bound as a generic function's, over the method's own
    // parameters and the receiver parameters its `where` clause bounds (or that
    // it compares without yet saying so).
    let decl = ctx.input.methods.get(type_name).and_then(|t| {
        t.instance
            .get(&method.name)
            .or_else(|| t.statics.get(&method.name))
            .cloned()
    });
    let compared = match &decl {
        Some(decl) if std::ptr::eq(decl.as_ref(), method_decl) => {
            super::equality::compared_type_params(decl, ctx)
        }
        _ => Default::default(),
    };
    let recv_decl = ctx.input.types.get(type_name);
    let declared_eq = |name: &str| {
        // A bound the receiver type declares (`type Set2[T: Eq]`) holds in its
        // methods, with no `where` clause.
        recv_decl.is_some_and(|d| {
            d.type_params
                .iter()
                .any(|tp| tp.name.name == name && super::equality::has_eq(&tp.bounds))
        }) || method_decl
            .type_params
            .iter()
            .any(|tp| tp.name.name == name && super::equality::has_eq(&tp.bounds))
            || method_decl
                .where_bounds
                .iter()
                .any(|wb| wb.param.name == name && super::equality::has_eq(&wb.bounds))
    };
    let mut eq_params: Vec<(String, Option<Span>)> = method_decl
        .type_params
        .iter()
        .map(|tp| (tp.name.name.clone(), Some(tp.span)))
        .collect();
    eq_params.extend(recv_type_params.iter().map(|p| (p.clone(), None)));
    let ord_params = super::ordering::witness_params(method_decl, &ctx.input.types);
    let key_params = super::keying::key_params(method_decl, &ctx.input.types);
    eq_params.retain(|(p, _)| {
        (declared_eq(p) || compared.contains(p))
            && !ord_params.iter().any(|o| o == p)
            && !key_params.iter().any(|k| k == p)
    });
    let callee = format!("{type_name}.{}", method.name);
    if !super::equality::check_declared_eq_args(&callee, method.span, &eq_params, &subst, ctx) {
        return None;
    }
    let witnesses = super::codable::check_declared_witness_args(
        &callee,
        method.span,
        &super::codable::witness_list(method_decl, &ctx.input.types),
        &subst,
        ctx,
    )?;
    if !super::keying::check_declared_key_args(&callee, method.span, &key_params, &subst, ctx) {
        return None;
    }
    if !witnesses.is_empty() {
        ctx.last_witness = Some(witnesses);
    }
    // v0.39 (ADR 0072)-style hint: show the inferred method type arguments after
    // the method name (`box.map` [Int] `(f)`). Only the method's own
    // parameters — the receiver's are visible in the receiver's type.
    if !method_decl.type_params.is_empty() {
        let rendered: Option<Vec<String>> = method_decl
            .type_params
            .iter()
            .map(|tp| subst.get(&tp.name.name).map(|t| t.display(tys)))
            .collect();
        if let Some(parts) = rendered {
            ctx.hints
                .record(method.span, format!("[{}]", parts.join(", ")));
        }
    }
    Some(substitute(ret_pattern, &subst, tys))
}

/// v0.178 (#662) / v0.182 (#664): resolve a test-body service invocation against
/// the target's declared handlers and check its arity and argument types. The
/// address form depends on the method name:
///
/// - `svc.call(args)` — the `on call` handler (#662);
/// - `svc.<VERB>("/path", …)` — an http route (`GET`/`POST`/`PUT`/`PATCH`/`DELETE`);
/// - `svc.schedule("<expr>", …)` — a cron handler by its schedule string;
/// - `svc.message(msg)` — the queue message handler.
///
/// #1934 (re-opening ADR 0203): the call is typed by what it yields. A
/// resolved handler yields its declared return type. An http
/// address carrying a `Wire(…)` argument, or naming a method the path declares
/// no handler for, yields `Effect[HttpOutcome[T]]` instead (`by Nobody`, which
/// lives on the statement, is applied by [`nobody_outcome`]); an asset fetch
/// yields `Effect[HttpResult[()]]`. The call-site principal (`by <Actor>`) is
/// checked separately at the statement level. Returns `None` in an arm that
/// reported an error.
fn check_test_service_address(
    sig: &TestServiceSig,
    id: &Ident,
    method: &Ident,
    args: &[Expr],
    // P6.0 (#1139): the outer `MethodCall` expression's own identity.
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    use bynk_syntax::ast::{ExprKind as EK, HandlerKind};

    // `svc.call(...)` — the `on call` handler (Slice 0).
    if method.name == "call" {
        ctx.callees.insert(
            expr_id,
            Callee::TestService {
                service: id.name.clone(),
                address: "call".to_string(),
            },
        );
        let Some(handler) = sig.call_handler() else {
            let message = match &sig.protocol {
                Some(protocol) => format!(
                    "`{}` is a `from {protocol}` service and has no `on call` handler to invoke",
                    id.name
                ),
                None => format!("service `{}` has no `on call` handler to invoke", id.name),
            };
            ctx.errors.push(
                CompileError::new("bynk.test.service_no_call_handler", method.span, message)
                    .with_note(
                        "call an `on call` service with `svc.call(...)`, an http route with `svc.GET(\"/path\")`, cron with `svc.schedule(\"…\")`, or a queue with `svc.message(m)`",
                    ),
            );
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        };
        let params = handler.params.clone();
        check_address_args(&id.name, "call", &params, args, method.span, ctx);
        return handler_outcome(handler, ctx);
    }

    // `svc.<VERB>("/path", …)` — an http route. The first argument is the route
    // *pattern* (a compile-time-resolved name), the rest are positional path
    // params then the body, matched against the handler's declared params.
    // `HttpMethod::from_ident` is the single source of truth the emitter shares.
    if bynk_syntax::ast::HttpMethod::from_ident(&method.name).is_some() {
        ctx.callees.insert(
            expr_id,
            Callee::TestService {
                service: id.name.clone(),
                address: method.name.clone(),
            },
        );
        // #1883, slice 3 ([DECISION F]): `svc.GET(style)` — an `Asset` as the
        // address. The platform serves an asset before the Worker runs, so no
        // handler is involved: the call answers with the file the target's
        // Worker ships, as the platform would (`HttpResult.Raw`).
        if let Some(first) = args.first()
            && !matches!(first.kind, EK::StrLit(_))
            && is_asset_value(first, ctx)
        {
            let _ = type_of(first, None, ctx);
            // Review of #1922: only an HTTP service fronts a Worker the
            // platform serves assets beside.
            if sig.protocol.as_deref() != Some("http") {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.test.service_bad_address",
                        method.span,
                        format!(
                            "`{}` is not a `from http` service, so it cannot fetch an asset",
                            id.name
                        ),
                    )
                    .with_note("fetch an asset through one of the target's `from http` services: `api.GET(asset)`"),
                );
                // Review of #1923: still type the rest, as every other
                // error arm here does, so a mistake inside them is reported.
                for a in &args[1..] {
                    let _ = type_of(a, None, ctx);
                }
                return None;
            }
            if method.name != "GET" {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.test.asset_address_method",
                        method.span,
                        format!(
                            "an asset is fetched with `{}.GET(asset)`, not `{}`",
                            id.name, method.name
                        ),
                    )
                    .with_note(
                        "the platform serves an asset to `GET` and answers every other method with `405`",
                    ),
                );
            }
            if args.len() > 1 {
                ctx.errors.push(CompileError::new(
                    "bynk.test.service_call_arity",
                    args[1].span,
                    format!(
                        "`{}.{}(asset)` takes the asset alone; remove the other arguments",
                        id.name, method.name
                    ),
                ));
                for a in &args[1..] {
                    let _ = type_of(a, None, ctx);
                }
            }
            ctx.callees.insert(
                expr_id,
                Callee::TestAssetFetch {
                    service: id.name.clone(),
                },
            );
            // `Raw(body, contentType)` or `NotFound`: no handler's `T` is involved.
            let unit = ctx.tys.intern(Ty::Unit);
            return Some(effect_of(ctx.tys.intern(Ty::HttpResult(unit)), ctx));
        }
        let Some(EK::StrLit(path)) = args.first().map(|a| &a.kind) else {
            ctx.errors.push(
                CompileError::new(
                    "bynk.test.service_bad_address",
                    method.span,
                    format!(
                        "`{}.{}` addresses an http route, so its first argument must be the route pattern string (e.g. `\"/todos\"`)",
                        id.name, method.name
                    ),
                ),
            );
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        };
        // The classification recorded above (verb only) is enriched here,
        // now that the route pattern — the thing `sig.handlers` is actually
        // matched against below — is known; a later go-to-definition
        // consumer needs the path, not just the verb, to resolve a handler.
        ctx.callees.insert(
            expr_id,
            Callee::TestService {
                service: id.name.clone(),
                address: format!("{} {path}", method.name),
            },
        );
        let matched = sig.handlers.iter().find(|h| {
            matches!(&h.kind, HandlerKind::Http { method: m, path: p } if m.as_str() == method.name && p == path)
        });
        let Some(handler) = matched else {
            // #707: the method has no handler at this path. If the *path* is
            // declared (for some other method), this is a **wrong-method** call —
            // the `405` fall-through test. Allow it: it drives the router with a
            // method the path has no handler for and observes `Rejected(
            // MethodNotAllowed)`. There is no handler, so no args are matched (a
            // `405` is synthesised before the body is read), and the outcome is an
            // `HttpOutcome` over `()`: no handler's `T` is involved.
            let path_declared = sig
                .handlers
                .iter()
                .any(|h| matches!(&h.kind, HandlerKind::Http { path: p, .. } if p == path));
            if path_declared {
                let _ = type_of(&args[0], None, ctx);
                // A wrong-method call has no handler, so it takes only the path;
                // diagnose extra args rather than silently dropping them (the
                // lowering forwards only method+path to the generic driver).
                if args.len() > 1 {
                    ctx.errors.push(
                        CompileError::new(
                            "bynk.test.service_call_arity",
                            method.span,
                            format!(
                                "`{}.{}(\"{}\")` is a wrong-method `405` test and takes only the route path, but {} argument(s) were given",
                                id.name,
                                method.name,
                                path,
                                args.len() - 1
                            ),
                        )
                        .with_note("a wrong-method call reaches no handler, so it passes no body or params"),
                    );
                    for a in &args[1..] {
                        let _ = type_of(a, None, ctx);
                    }
                }
                let unit = ctx.tys.intern(Ty::Unit);
                return Some(effect_of(ctx.tys.intern(Ty::HttpOutcome(unit)), ctx));
            }
            ctx.errors.push(
                CompileError::new(
                    "bynk.test.service_unknown_route",
                    method.span,
                    format!(
                        "`{}` declares no route at `\"{}\"` (no handler for any method)",
                        id.name, path
                    ),
                )
                .with_note("the path must match a declared route; drive a wrong method against an existing path to test the `405` fall-through"),
            );
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        };
        let params = handler.params.clone();
        // Type the route-pattern string as an ordinary string; check the rest.
        let _ = type_of(&args[0], None, ctx);
        check_address_args(
            &id.name,
            &method.name,
            &params,
            &args[1..],
            method.span,
            ctx,
        );
        // A `Wire(…)` argument reaches the router unvalidated, so the call
        // observes the boundary as well as the handler: the raw driver decodes
        // an `HttpOutcome`. The emitter picks that driver on the same test.
        let outcome = handler_outcome(handler, ctx);
        if args[1..].iter().any(|a| matches!(a.kind, EK::Wire(_))) {
            return outcome.map(|t| wire_outcome(t, ctx));
        }
        return outcome;
    }

    // `svc.schedule("<expr>", …)` — a cron handler, matched by its schedule.
    if method.name == "schedule" {
        ctx.callees.insert(
            expr_id,
            Callee::TestService {
                service: id.name.clone(),
                address: "schedule".to_string(),
            },
        );
        let Some(EK::StrLit(expr)) = args.first().map(|a| &a.kind) else {
            ctx.errors.push(CompileError::new(
                "bynk.test.service_bad_address",
                method.span,
                format!(
                    "`{}.schedule` addresses a cron handler, so its first argument must be the schedule string",
                    id.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        };
        // Enrich with the schedule expression `sig.handlers` is matched
        // against below — same reasoning as the http-verb branch above.
        ctx.callees.insert(
            expr_id,
            Callee::TestService {
                service: id.name.clone(),
                address: format!("schedule {expr}"),
            },
        );
        let matched = sig
            .handlers
            .iter()
            .find(|h| matches!(&h.kind, HandlerKind::Cron { expr: e } if e == expr));
        let Some(handler) = matched else {
            ctx.errors.push(CompileError::new(
                "bynk.test.service_unknown_route",
                method.span,
                format!(
                    "`{}` declares no `on schedule(\"{}\")` handler",
                    id.name, expr
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        };
        let params = handler.params.clone();
        let _ = type_of(&args[0], None, ctx);
        check_address_args(&id.name, "schedule", &params, &args[1..], method.span, ctx);
        return handler_outcome(handler, ctx);
    }

    // `svc.message(msg)` — the queue message handler.
    if method.name == "message" {
        ctx.callees.insert(
            expr_id,
            Callee::TestService {
                service: id.name.clone(),
                address: "message".to_string(),
            },
        );
        let matched = sig
            .handlers
            .iter()
            .find(|h| matches!(&h.kind, HandlerKind::Message));
        let Some(handler) = matched else {
            ctx.errors.push(CompileError::new(
                "bynk.test.service_unknown_route",
                method.span,
                format!("`{}` declares no `on message(...)` handler", id.name),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        };
        let params = handler.params.clone();
        check_address_args(&id.name, "message", &params, args, method.span, ctx);
        return handler_outcome(handler, ctx);
    }

    // #2052: `svc.event(payload)` / `svc.event(payload, envelope)` — one
    // delivery to a `from Events(E)` service's `on event` handler.
    if method.name == "event"
        && let Some(handler) = sig
            .handlers
            .iter()
            .find(|h| matches!(&h.kind, HandlerKind::Event))
    {
        ctx.callees.insert(
            expr_id,
            Callee::TestService {
                service: id.name.clone(),
                address: "event".to_string(),
            },
        );
        let handler = handler.clone();
        check_event_address(sig, id, &handler, args, method.span, ctx);
        return handler_outcome(&handler, ctx);
    }

    // Not a recognised address form for this service.
    let proto = sig.protocol.as_deref().unwrap_or("call");
    let mut err = CompileError::new(
        "bynk.test.service_bad_address",
        method.span,
        format!(
            "`{}.{}` is not a way to address a `from {proto}` service in a test body",
            id.name, method.name
        ),
    );
    if method.name == "event" {
        err = err.with_note(
            "only a `from Events(E)` service is addressed with `svc.event(e)`; it delivers one event to the service's `on event` handler",
        );
    }
    ctx.errors.push(err);
    for a in args {
        let _ = type_of(a, None, ctx);
    }
    None
}

/// #1934: what a case's call to `handler` yields — its declared return type,
/// an `Effect[_]` (`bynk.service.return_not_effect`). `None` if the type does
/// not resolve in the case, which a checked target's handler always does.
fn handler_outcome(handler: &TestHandler, ctx: &mut Ctx) -> Option<TyId> {
    resolve_type_ref(&handler.return_type, &ctx.input.types, ctx.tys)
}

/// `Effect[ty]`.
fn effect_of(ty: TyId, ctx: &Ctx) -> TyId {
    ctx.tys.intern(Ty::Effect(ty))
}

/// #1934: an http handler's outcome `Effect[HttpResult[T]]` as a `system`-tier
/// case observes it past the handler's types: `Effect[HttpOutcome[T]]`.
fn wire_outcome(outcome: TyId, ctx: &Ctx) -> TyId {
    let tys = ctx.tys;
    let inner = match &*tys.get(outcome) {
        Ty::Effect(t) => *t,
        _ => outcome,
    };
    let t = match &*tys.get(inner) {
        Ty::HttpResult(t) => *t,
        _ => return outcome,
    };
    tys.intern(Ty::Effect(tys.intern(Ty::HttpOutcome(t))))
}

/// #1934: the type an effect-let binds for `value` driven `by Nobody`. The
/// no-credential driver decodes an `HttpOutcome` (a `401` from the auth seam is
/// `Rejected(Unauthorized)`), so a typed http address's `Effect[HttpResult[T]]`
/// becomes `Effect[HttpOutcome[T]]`; anything else is unchanged.
/// #2155: on an actor sum with an open member (`Visitor`), say why the seam
/// never answers `401`.
fn with_open_sum_note(err: CompileError, handler: &TestHandler, ctx: &Ctx) -> CompileError {
    let open = handler
        .by_clause
        .as_ref()
        .is_some_and(|by| by.is_sum() && crate::actors::sum_has_open_member(by, &ctx.test_actors));
    if open {
        err.with_note(
            "this actor sum admits an open member (`Visitor`): a request with a missing or bad credential resolves as it, so the seam never answers `401`",
        )
    } else {
        err
    }
}

/// #2135: whether `name` is a reserved bad-signature principal — `Forged`
/// (a wrong secret) or `Stale` (a timestamp past the tolerance).
pub(crate) fn is_bad_signature_principal(name: &str) -> bool {
    matches!(name, "Forged" | "Stale")
}

pub(crate) fn nobody_outcome(value: &Expr, ty: TyId, ctx: &Ctx) -> TyId {
    let is_http_address = matches!(
        ctx.callees.get(&value.id),
        Some(Callee::TestService { address, .. })
            if address.split(' ').next().is_some_and(|m| bynk_syntax::ast::HttpMethod::from_ident(m).is_some())
    );
    if is_http_address {
        wire_outcome(ty, ctx)
    } else {
        ty
    }
}

/// What identity, if any, a resolved actor expects (v0.182).
enum ActorIdentity {
    Typed(TyId),
    CallerString,
    Unit,
    Unknown,
}

/// Resolve an actor name to the identity it carries (v0.182).
fn resolve_actor_identity(name: &str, ctx: &Ctx) -> ActorIdentity {
    let tys = ctx.tys;
    use crate::actors::{Identity, prelude_actor};
    if let Some(decl) = ctx.test_actors.get(name) {
        // #2019: a refinement (`actor Admin = User where …`) narrows its base by
        // a claim and inherits the base's identity: the Worker's Bearer seam
        // follows the refinement to its base and mints the base identity
        // (`bearer_seam_for`). A refinement whose base is missing or is itself a
        // refinement is malformed (`bynk.actor.refinement_base_unsupported`),
        // so it resolves to no identity at all rather than passing as `Unit`.
        let decl = match &decl.refinement {
            Some(r) => match ctx.test_actors.get(&r.base.name) {
                Some(base) if base.refinement.is_none() => base,
                _ => return ActorIdentity::Unknown,
            },
            None => decl,
        };
        return match &decl.identity {
            Some(t) => match resolve_type_ref(t, &ctx.input.types, tys) {
                Some(ty) => ActorIdentity::Typed(ty),
                None => ActorIdentity::Unit,
            },
            None => ActorIdentity::Unit,
        };
    }
    match prelude_actor(name) {
        Some(c) => match c.identity {
            Identity::Unit => ActorIdentity::Unit,
            Identity::CallerId => ActorIdentity::CallerString,
            Identity::Declared(_) => ActorIdentity::Unit,
        },
        None => ActorIdentity::Unknown,
    }
}

/// The actor a handler runs as: its declared `by <Actor>`, or the protocol
/// default (`Call`->`Caller`, `Cron`->`Scheduler`, `Queue`->`Producer`; http
/// has no default, so an http handler always declares one) (v0.182).
fn handler_actor_name(handler: &TestHandler, protocol: Option<&str>) -> Option<String> {
    if let Some(by) = &handler.by_clause {
        return Some(by.primary().name.clone());
    }
    match protocol {
        None => Some("Caller".to_string()),
        Some("cron") => Some("Scheduler".to_string()),
        Some("queue") => Some("Producer".to_string()),
        _ => None,
    }
}

/// #1883, slice 3: whether `e` is an `Asset` value — an asset name in scope
/// (not shadowed by a local), or a field/expression that types as `Asset` —
/// without recording anything. Decides the `svc.GET(asset)` address form.
fn is_asset_value(e: &Expr, ctx: &Ctx) -> bool {
    match &e.kind {
        ExprKind::Ident(id) => ctx
            .lookup(id.name.as_str())
            .map(|t| matches!(&*ctx.tys.get(t), Ty::Asset))
            .unwrap_or_else(|| ctx.input.assets.contains_key(&id.name)),
        _ => false,
    }
}

/// v0.182 (#664): resolve the handler a test-body address invocation targets,
/// without side effects. Shared by the argument check and the principal check.
fn resolve_test_address<'a>(
    sig: &'a TestServiceSig,
    method: &str,
    args: &[Expr],
) -> Option<&'a TestHandler> {
    use bynk_syntax::ast::{ExprKind as EK, HandlerKind, HttpMethod};
    if method == "call" {
        return sig.call_handler();
    }
    if HttpMethod::from_ident(method).is_some() {
        let EK::StrLit(path) = &args.first()?.kind else {
            return None;
        };
        return sig.handlers.iter().find(|h| {
            matches!(&h.kind, HandlerKind::Http { method: m, path: p } if m.as_str() == method && p == path)
        });
    }
    if method == "schedule" {
        let EK::StrLit(expr) = &args.first()?.kind else {
            return None;
        };
        return sig
            .handlers
            .iter()
            .find(|h| matches!(&h.kind, HandlerKind::Cron { expr: e } if e == expr));
    }
    if method == "message" {
        return sig
            .handlers
            .iter()
            .find(|h| matches!(&h.kind, HandlerKind::Message));
    }
    // #2052: the `on event` handler of a `from Events(E)` service.
    if method == "event" {
        return sig
            .handlers
            .iter()
            .find(|h| matches!(&h.kind, HandlerKind::Event));
    }
    None
}

/// v0.182 (#664): validate the call-site principal of a test `effect_let`
/// against the ADDRESSED HANDLER. This is where per-case identity isolation is
/// enforced: an identity-carrying handler driven with a unit principal (`by
/// Visitor`) or no `by` at all would otherwise emit a call with
/// `deps.identity === undefined`. The identity value is typed against the
/// handler's required identity type, not the principal actor's.
pub(crate) fn check_effect_let_principal(
    value: &Expr,
    principal: Option<&bynk_syntax::ast::CallSiteActor>,
    ctx: &mut Ctx,
) {
    let tys = ctx.tys;
    // #2380 (L5): a call-site principal's claims (`by W("id", claims)`) are a
    // `TokenClaims`, and only an `Oidc` actor's handler reads them.
    if let Some(p) = principal
        && let Some(claims) = &p.claims
    {
        let want = tys.intern(Ty::TokenClaims);
        check_arg(claims, want, "the principal's claims", ctx);
        let oidc = ctx.test_actors.get(&p.actor.name).is_some_and(|decl| {
            decl.auth
                .as_ref()
                .is_some_and(|a| a.name.as_str() == "Oidc")
        });
        if !oidc {
            ctx.errors.push(
                CompileError::new(
                    "bynk.test.principal_claims_not_oidc",
                    claims.span,
                    format!(
                        "`{}` isn't an `Oidc` actor, so its handlers read no token claims",
                        p.actor.name
                    ),
                )
                .with_note("claims are passed as `by Workload(\"id\", TokenClaims.of([\"name: value\"]))` for an `Oidc` actor"),
            );
        }
    }
    let ExprKind::MethodCall {
        receiver,
        method,
        args,
        ..
    } = &value.kind
    else {
        if let Some(p) = principal {
            report_principal_actor(p, ctx);
        }
        return;
    };
    let ExprKind::Ident(id) = &receiver.kind else {
        if let Some(p) = principal {
            report_principal_actor(p, ctx);
        }
        return;
    };
    let Some(sig) = ctx.test_services.get(&id.name).cloned() else {
        if let Some(p) = principal {
            report_principal_actor(p, ctx);
        }
        return;
    };
    let Some(handler) = resolve_test_address(&sig, &method.name, args).cloned() else {
        if let Some(p) = principal
            && args.first().is_some_and(|a| is_asset_value(a, ctx))
        {
            // #1883, slice 3: an asset is public and served before the Worker
            // runs, so no principal reaches it.
            ctx.errors.push(
                CompileError::new(
                    "bynk.test.principal_on_asset",
                    p.span,
                    format!(
                        "an asset is public and served before the Worker runs, so `by {}` is meaningless",
                        p.actor.name
                    ),
                )
                .with_note("drop the `by` clause on an asset fetch"),
            );
            if let Some(id) = &p.identity {
                let _ = type_of(id, None, ctx);
            }
            return;
        }
        if let Some(p) = principal {
            // #707: a wrong-method `405` test (an http method whose path is
            // declared for *another* method) reaches no handler, so a `by` clause
            // is meaningless — reject it clearly. (An unresolvable address of any
            // other shape already errors in `check_test_service_address`; only the
            // now-allowed wrong-method call would otherwise drop the `by`.)
            let wrong_method = bynk_syntax::ast::HttpMethod::from_ident(&method.name).is_some()
                && matches!(args.first().map(|a| &a.kind), Some(bynk_syntax::ast::ExprKind::StrLit(path))
                    if sig.handlers.iter().any(|h| matches!(&h.kind, bynk_syntax::ast::HandlerKind::Http { path: p, .. } if p == path)));
            if wrong_method {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.test.principal_on_wrong_method",
                        p.span,
                        format!(
                            "a wrong-method `405` test reaches no handler, so `by {}` is meaningless",
                            p.actor.name
                        ),
                    )
                    .with_note("drop the `by` clause on a wrong-method call"),
                );
                if let Some(id) = &p.identity {
                    let _ = type_of(id, None, ctx);
                }
            } else {
                report_principal_actor(p, ctx);
            }
        }
        return;
    };

    // #706: `by Nobody` is the reserved "no credential" principal. It drives the
    // route with no `Authorization` header so the real auth seam rejects it
    // (`401` → `Rejected(Unauthorized)`), so it is valid on any http handler
    // regardless of the required identity — the whole point is that *no* valid
    // credential is presented. It carries no identity (`by Nobody(...)` is
    // meaningless). The `system`-only tier rule is enforced at emit time, like
    // `Wire`'s (the checker has no tier).
    if let Some(p) = principal
        && p.actor.name == "Nobody"
    {
        // #710-style: `by Nobody` is only *implemented* for a Bearer- or
        // (#2135) Signature-secured route — the no-auth driver leaves out the
        // `Authorization` header the Bearer seam checks, or the signature
        // headers the Signature seam checks. On an unsecured
        // (`Visitor`/`None`) or `Oidc` route there is no such seam to reject a
        // missing credential, so reject it here rather than emit a call to a
        // driver that was never generated.
        // #2155: an actor sum is secured when a member is.
        let secured = handler
            .by_clause
            .as_ref()
            .is_some_and(|by| crate::actors::route_is_secured(by, &ctx.test_actors));
        if !secured {
            let err = CompileError::new(
                "bynk.test.nobody_needs_secured_route",
                p.span,
                "`by Nobody` drives the auth seam to a `401`, but this handler's route has no seam that refuses a missing credential",
            )
            .with_note(
                "use `by Nobody` only on a route guarded by a `Bearer` or `Signature` actor; a public (`Visitor`) route has no seam to test",
            );
            ctx.errors.push(with_open_sum_note(err, &handler, ctx));
        }
        if let Some(idv) = &p.identity {
            ctx.errors.push(CompileError::new(
                "bynk.test.actor_no_identity",
                p.span,
                "`Nobody` presents no credential, so it takes no identity — write `by Nobody`",
            ));
            let _ = type_of(idv, None, ctx);
        }
        return;
    }

    // #2135: `by Forged` / `by Stale` are reserved principals, like `Nobody`:
    // the `system` driver signs a `Signature` route with a wrong secret, or
    // `tolerance + 60` seconds in the past, so the real seam refuses it. They
    // name no actor and take no identity. The `system`-only tier rule is
    // enforced with the case's tier (`bynk.test.forged_needs_system`).
    if let Some(p) = principal
        && is_bad_signature_principal(&p.actor.name)
    {
        let name = p.actor.name.as_str();
        match handler
            .by_clause
            .as_ref()
            .and_then(|by| crate::actors::signature_seam_of_route(by, &ctx.test_actors))
        {
            None => {
                let err = CompileError::new(
                    "bynk.test.forged_needs_signature_route",
                    p.span,
                    format!(
                        "`by {name}` sends a bad signature to a `Signature` seam, but this handler's route has no `Signature` seam that refuses one"
                    ),
                )
                .with_note(
                    "use `by Forged` and `by Stale` on a route guarded by a `Signature` actor; on a Bearer route, `by Nobody` drives the seam to a `401`",
                );
                ctx.errors.push(with_open_sum_note(err, &handler, ctx));
            }
            Some(seam)
                if name == "Stale"
                    && (seam.timestamp_header.is_none() || seam.tolerance_secs.is_none()) =>
            {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.test.stale_needs_timestamp",
                        p.span,
                        "`by Stale` signs in the past, but this route's `Signature` actor declares no `timestamp` header and `tolerance` for the seam to refuse it by",
                    )
                    .with_note(
                        "declare `timestamp = \"<Header>\"` and `tolerance = <seconds>` on the actor, or test the seam with `by Forged`",
                    ),
                );
            }
            Some(_) => {}
        }
        if let Some(idv) = &p.identity {
            ctx.errors.push(CompileError::new(
                "bynk.test.actor_no_identity",
                p.span,
                format!("`{name}` presents a signature, not an identity, so it takes none — write `by {name}`"),
            ));
            let _ = type_of(idv, None, ctx);
        }
        return;
    }

    // #2020: a sum (`by who: User | Visitor`) runs as whichever member
    // resolves, so the case may act as any member, each with its own identity.
    if let Some(by) = handler.by_clause.as_ref().filter(|by| by.is_sum()) {
        check_sum_principal(by, principal, value.span, ctx);
        return;
    }

    let required = handler_actor_name(&handler, sig.protocol.as_deref())
        .map(|actor| resolve_actor_identity(&actor, ctx));

    match (required, principal) {
        (Some(ActorIdentity::Typed(ty)), principal) => match principal {
            Some(p) => match resolve_actor_identity(&p.actor.name, ctx) {
                ActorIdentity::Unknown => report_principal_actor(p, ctx),
                ActorIdentity::Unit | ActorIdentity::CallerString => {
                    ctx.errors.push(CompileError::new(
                        "bynk.test.principal_identity_mismatch",
                        p.span,
                        format!(
                            "this handler runs as an actor carrying `{}`, but `by {}` supplies no matching identity",
                            ty.display(tys),
                            p.actor.name
                        ),
                    ));
                    if let Some(idv) = &p.identity {
                        let _ = type_of(idv, None, ctx);
                    }
                }
                ActorIdentity::Typed(_) => match &p.identity {
                    Some(idv) => {
                        let got = type_of(idv, Some(ty), ctx);
                        if let Some(g) = got
                            && !compatible(g, ty, tys)
                        {
                            ctx.errors.push(CompileError::new(
                                "bynk.types.argument_mismatch",
                                idv.span,
                                format!(
                                    "identity has type `{}`, but the handler expects `{}`",
                                    g.display(tys),
                                    ty.display(tys)
                                ),
                            ));
                        }
                    }
                    None => ctx.errors.push(CompileError::new(
                        "bynk.test.actor_identity_required",
                        p.span,
                        format!(
                            "actor `{}` carries an identity, so write `by {}(...)`",
                            p.actor.name, p.actor.name
                        ),
                    )),
                },
            },
            None => ctx.errors.push(
                CompileError::new(
                    "bynk.test.principal_required",
                    value.span,
                    format!(
                        "this handler runs as a verified actor carrying `{}`; the case must act as it with `by <Actor>(<identity>)`",
                        ty.display(tys)
                    ),
                )
                .with_note("append a call-site actor, e.g. `... by User(\"alice\")`"),
            ),
        },
        // #1978: an `on call` handler that binds its caller (`by c: Caller`)
        // reads `deps.identity`, the calling context's name. A test has no
        // calling context, so the case must name one with `by Caller("…")`;
        // without it the emitted call carries no identity and fails `tsc`.
        (Some(ActorIdentity::CallerString), principal) if handler_binds_caller(&handler, ctx) => {
            match principal {
                None => ctx.errors.push(
                    CompileError::new(
                        "bynk.test.actor_identity_required",
                        value.span,
                        "this handler binds its caller's identity (`by <binder>: Caller`), so the case must name the calling context with `by Caller(\"...\")`",
                    )
                    .with_note("append a call-site actor, e.g. `... by Caller(\"orders\")`"),
                ),
                Some(p) => match resolve_actor_identity(&p.actor.name, ctx) {
                    ActorIdentity::CallerString if p.identity.is_none() => {
                        ctx.errors.push(CompileError::new(
                            "bynk.test.actor_identity_required",
                            p.span,
                            format!(
                                "this handler binds its caller's identity, so write `by {}(\"...\")` naming the calling context",
                                p.actor.name
                            ),
                        ))
                    }
                    ActorIdentity::Unit => {
                        ctx.errors.push(CompileError::new(
                            "bynk.test.principal_identity_mismatch",
                            p.span,
                            format!(
                                "this handler binds its caller's identity, but `by {}` supplies none — write `by Caller(\"...\")`",
                                p.actor.name
                            ),
                        ));
                        if let Some(idv) = &p.identity {
                            let _ = type_of(idv, None, ctx);
                        }
                    }
                    _ => report_principal_actor(p, ctx),
                },
            }
        }
        (_, Some(p)) => report_principal_actor(p, ctx),
        (_, None) => {}
    }
}

/// #2020: check a call-site principal against a handler whose `by` clause is
/// an actor sum (`by who: User | Visitor`, or an `on call` sum such as
/// `Caller | Legacy`). The handler reads the resolved member as `deps.who`, so
/// the case must name a member, with exactly the identity that member
/// carries: `by User("ada")` or `by Visitor`. A non-member, or no `by` at all,
/// would leave `deps.who` unbuilt.
fn check_sum_principal(
    by: &bynk_syntax::ast::ByClause,
    principal: Option<&bynk_syntax::ast::CallSiteActor>,
    value_span: Span,
    ctx: &mut Ctx,
) {
    let members = format!(
        "`{}`",
        by.actors
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(" | ")
    );
    let Some(p) = principal else {
        ctx.errors.push(
            CompileError::new(
                "bynk.test.principal_required",
                value_span,
                format!(
                    "this handler runs as one of the actors {members}; the case must act as one with `by <Actor>`"
                ),
            )
            .with_note(format!(
                "append a call-site actor naming a member, e.g. `... by {}`",
                sum_member_example(&by.actors[0].name, ctx)
            )),
        );
        return;
    };
    let name = &p.actor.name;
    if !by.actors.iter().any(|a| &a.name == name) {
        if matches!(resolve_actor_identity(name, ctx), ActorIdentity::Unknown) {
            report_principal_actor(p, ctx);
            return;
        }
        ctx.errors.push(
            CompileError::new(
                "bynk.test.principal_identity_mismatch",
                p.span,
                format!(
                    "this handler runs as one of the actors {members}, but `by {name}` names none of them"
                ),
            )
            .with_note("act as a member of the handler's `by` sum"),
        );
        if let Some(idv) = &p.identity {
            let _ = type_of(idv, None, ctx);
        }
        return;
    }
    match resolve_actor_identity(name, ctx) {
        // The sum's `Caller` arm carries the calling context's name
        // (`{ tag: "Caller", identity: string }`), so it must be named.
        ActorIdentity::CallerString if p.identity.is_none() => ctx.errors.push(CompileError::new(
            "bynk.test.actor_identity_required",
            p.span,
            format!(
                "the `{name}` arm carries the calling context's name, so write `by {name}(\"...\")`"
            ),
        )),
        _ => report_principal_actor(p, ctx),
    }
}

/// An example call-site principal for a sum member, for a note.
fn sum_member_example(name: &str, ctx: &Ctx) -> String {
    match resolve_actor_identity(name, ctx) {
        ActorIdentity::Typed(_) | ActorIdentity::CallerString => format!("{name}(...)"),
        ActorIdentity::Unit | ActorIdentity::Unknown => name.to_string(),
    }
}

/// #1978: whether an `on call` handler binds the prelude `Caller`'s identity
/// (`by c: Caller`), so its emitted `deps` requires `identity`. This mirrors
/// `bynk_lower::lower_actor_seam_ir`, which resolves a sum `by` clause
/// (`by who: Caller | …`) as a sum (`deps.who`) before it consults
/// [`crate::actors::caller_binder_for`]; hence the `!is_sum()` that
/// `caller_binder_for` alone does not need.
fn handler_binds_caller(handler: &TestHandler, ctx: &Ctx) -> bool {
    use crate::actors::{Identity, prelude_actor};
    let Some(by) = &handler.by_clause else {
        return false;
    };
    let Some(binder) = &by.binder else {
        return false;
    };
    let name = &by.primary().name;
    matches!(handler.kind, bynk_syntax::ast::HandlerKind::Call)
        && !by.is_sum()
        && !ctx.test_actors.contains_key(name)
        && prelude_actor(name).map(|c| c.identity) == Some(Identity::CallerId)
        && !handler.params.iter().any(|p| p.name.name == binder.name)
}

/// Resolve a call-site principal actor for its own diagnostics (v0.182).
fn report_principal_actor(p: &bynk_syntax::ast::CallSiteActor, ctx: &mut Ctx) {
    let tys = ctx.tys;
    let name = &p.actor.name;
    match resolve_actor_identity(name, ctx) {
        // A declared actor that resolves to nothing is a malformed refinement,
        // already reported at its declaration
        // (`bynk.actor.refinement_base_unsupported`). It is still an actor of
        // the target, so `unknown_actor` would mislead.
        ActorIdentity::Unknown if ctx.test_actors.contains_key(name) => {
            if let Some(id) = &p.identity {
                let _ = type_of(id, None, ctx);
            }
        }
        // #2135: a reserved bad-signature principal reaching no http route
        // (a `call`, an event, a cron or queue address) has no `Signature`
        // seam to refuse it.
        ActorIdentity::Unknown if is_bad_signature_principal(name) => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.test.forged_needs_signature_route",
                    p.span,
                    format!("`by {name}` sends a bad signature to a `Signature` seam, but this address reaches no `Signature`-secured http route"),
                )
                .with_note("use `by Forged` and `by Stale` on an http route guarded by one `Signature` actor"),
            );
            if let Some(id) = &p.identity {
                let _ = type_of(id, None, ctx);
            }
        }
        ActorIdentity::Unknown => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.test.unknown_actor",
                    p.actor.span,
                    format!("`{name}` is not an actor of the target context or a prelude actor"),
                )
                .with_note("name an `actor` the target declares, or a prelude actor (`Visitor`, `Caller`, …)"),
            );
            if let Some(id) = &p.identity {
                let _ = type_of(id, None, ctx);
            }
        }
        ActorIdentity::Typed(ty) => match &p.identity {
            Some(id) => {
                let got = type_of(id, Some(ty), ctx);
                if let Some(g) = got
                    && !compatible(g, ty, tys)
                {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.argument_mismatch",
                        id.span,
                        format!(
                            "identity has type `{}`, but actor `{name}` expects `{}`",
                            g.display(tys),
                            ty.display(tys)
                        ),
                    ));
                }
            }
            None => ctx.errors.push(CompileError::new(
                "bynk.test.actor_identity_required",
                p.span,
                format!("actor `{name}` carries an identity, so `by {name}(...)` needs an identity value"),
            )),
        },
        ActorIdentity::CallerString => {
            if let Some(id) = &p.identity {
                let _ = type_of(id, None, ctx);
            }
        }
        ActorIdentity::Unit => {
            if let Some(id) = &p.identity {
                let _ = type_of(id, None, ctx);
                ctx.errors.push(CompileError::new(
                    "bynk.test.actor_no_identity",
                    p.span,
                    format!("actor `{name}` has no identity, so write `by {name}` with no argument"),
                ));
            }
        }
    }
}

/// Shared arity + argument-type check for a resolved test-body service address.
/// `positional` are the arguments that map to the handler's declared params (for
/// http/cron the leading pattern string has already been split off).
fn check_address_args(
    svc: &str,
    addr: &str,
    params: &[bynk_syntax::ast::Param],
    positional: &[Expr],
    err_span: Span,
    ctx: &mut Ctx,
) {
    let tys = ctx.tys;
    if params.len() != positional.len() {
        ctx.errors.push(
            CompileError::new(
                "bynk.test.service_call_arity",
                err_span,
                format!(
                    "`{svc}.{addr}` expects {} argument(s), but {} were given",
                    params.len(),
                    positional.len()
                ),
            )
            // Finding #46: `handler_span` is the target unit's handler — a
            // test file and the unit it tests are normally different files.
            .with_note("handler declared here"),
        );
        for a in positional {
            let _ = type_of(a, None, ctx);
        }
        return;
    }
    for (i, (param, arg)) in params.iter().zip(positional.iter()).enumerate() {
        record_param_hint(ctx.hints, &param.name.name, arg);
        // Slice C: a `Wire(<String>)` argument is *raw* — it deliberately bypasses
        // the param's type so a case can drive the boundary with input the type
        // forbids. Validate only that the inner is a `String` (the wire form); the
        // `system`-only tier rule is enforced at emit time (where the tier is
        // known), alongside `system_needs_wire`. Intercept before the generic
        // `type_of`, which would otherwise report the address-arg `Wire` as
        // misplaced.
        if let bynk_syntax::ast::ExprKind::Wire(inner) = &arg.kind {
            let _ = type_of(inner, Some(tys.intern(Ty::Base(BaseType::String))), ctx);
            continue;
        }
        let expected = resolve_type_ref(&param.type_ref, &ctx.input.types, tys);
        let arg_ty = type_of(arg, expected, ctx);
        if let (Some(a), Some(p)) = (arg_ty, expected)
            && !compatible(a, p, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                arg.span,
                format!(
                    "argument {} has type `{}`, but `{svc}.{addr}` expects `{}` for `{}`",
                    i + 1,
                    a.display(tys),
                    p.display(tys),
                    param.name.name
                ),
            ));
        }
    }
}

/// #2052: arity and argument types of `svc.event(payload)` /
/// `svc.event(payload, envelope)`. The payload is typed against the handler's
/// first parameter, which `bynk.event.handler_param_type_mismatch` holds to the
/// header's event type `E`; the envelope against `EventEnvelope`, whether or
/// not the handler declares one (a `via schema(N)` guard reads it either way).
fn check_event_address(
    sig: &TestServiceSig,
    id: &Ident,
    handler: &TestHandler,
    args: &[Expr],
    err_span: Span,
    ctx: &mut Ctx,
) {
    let tys = ctx.tys;
    let svc = &id.name;
    if args.is_empty() || args.len() > 2 {
        ctx.errors.push(
            CompileError::new(
                "bynk.test.service_call_arity",
                err_span,
                format!(
                    "`{svc}.event` takes the event and, optionally, an `EventEnvelope`, but {} argument(s) were given",
                    args.len()
                ),
            )
            .with_note("deliver an event with `svc.event(e)`, or `svc.event(e, env)` to choose its envelope"),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return;
    }
    // #2052 review: a default envelope names the event's publisher, which must
    // resolve to one context, as the service's header resolves `E`.
    if args.len() == 1
        && let Some(Err(candidates)) = &sig.event_publisher
    {
        let found = if candidates.is_empty() {
            "no context the target declares or consumes declares it".to_string()
        } else {
            format!(
                "it is declared by {}",
                candidates
                    .iter()
                    .map(|c| format!("`{c}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        ctx.errors.push(
            CompileError::new(
                "bynk.test.event_publisher_ambiguous",
                err_span,
                format!("`{svc}.event(e)` has no single publisher to name in its default envelope: {found}"),
            )
            .with_note("pass the envelope yourself: `svc.event(e, env)`"),
        );
    }
    let payload = &args[0];
    let expected = handler
        .params
        .first()
        .and_then(|p| resolve_type_ref(&p.type_ref, &ctx.input.types, tys));
    let arg_ty = type_of(payload, expected, ctx);
    if let (Some(a), Some(p)) = (arg_ty, expected)
        && !compatible(a, p, tys)
    {
        let other_event = match &*tys.get(a) {
            Ty::Named { name, .. } if sig.known_events.contains(name) => Some(name.clone()),
            _ => None,
        };
        let err = match other_event {
            Some(other) => CompileError::new(
                "bynk.test.event_not_subscribed",
                payload.span,
                format!(
                    "`{svc}` subscribes to `{}`, not `{other}`",
                    p.display(tys)
                ),
            )
            .with_note(
                "an event address delivers only the event the service's `from Events(E)` header names",
            ),
            None => CompileError::new(
                "bynk.types.argument_mismatch",
                payload.span,
                format!(
                    "argument 1 has type `{}`, but `{svc}.event` expects the event it subscribes to, `{}`",
                    a.display(tys),
                    p.display(tys)
                ),
            ),
        };
        ctx.errors.push(err);
    }
    let Some(envelope) = args.get(1) else {
        return;
    };
    let envelope_ref = TypeRef::Named(Ident {
        name: "EventEnvelope".to_string(),
        span: envelope.span,
    });
    let expected = resolve_type_ref(&envelope_ref, &ctx.input.types, tys);
    let env_ty = type_of(envelope, expected, ctx);
    let Some(a) = env_ty else {
        return;
    };
    // `EventEnvelope` is `bynk`'s; a suite names it only when its target
    // consumes `bynk`, and no other value is one.
    let mismatch = match expected {
        Some(p) => !compatible(a, p, tys),
        None => true,
    };
    if mismatch {
        let mut err = CompileError::new(
            "bynk.types.argument_mismatch",
            envelope.span,
            format!(
                "argument 2 has type `{}`, but `{svc}.event` expects an `EventEnvelope`",
                a.display(tys)
            ),
        );
        if expected.is_none() {
            err = err.with_note(
                "`EventEnvelope` comes from `bynk`; the suite's target must `consumes bynk` to build one",
            );
        }
        ctx.errors.push(err);
    }
}

/// If `receiver` resolves to a consumed-context prefix (an alias or a
/// dotted qualified name appearing in `consumes`), return the consumed
/// context's qualified name. Otherwise None. Local bindings, types, and
/// capabilities take precedence — those are checked at the call site.
fn cross_context_prefix(receiver: &Expr, ctx: &Ctx) -> Option<String> {
    let info = &ctx.input.cross_context;
    if info.consumed_contexts.is_empty() && info.aliases.is_empty() {
        return None;
    }
    // Walk the receiver to assemble a candidate dotted name. Supports:
    //   Ident(X)                                 -> "X"
    //   FieldAccess { Ident(A), B }              -> "A.B"
    //   FieldAccess { FieldAccess { Ident(A), B }, C } -> "A.B.C"
    let candidate = flatten_ident_chain(receiver)?;
    let head = candidate.split('.').next().unwrap_or("");
    // The head must not shadow a local binding / capability / declared type.
    if ctx.lookup(head).is_some() {
        return None;
    }
    if ctx.caps.capabilities.contains_key(head) || ctx.caps.declared_capabilities.contains_key(head)
    {
        return None;
    }
    // If the head is a known local type, only an alias whose name happens to
    // collide could redirect this; aliases conflicting with types are an
    // error in project.rs, so a clash here is impossible at this point.
    info.resolve_prefix(candidate.as_str())
}

/// Flatten an `Ident`/`FieldAccess` chain into its dotted name, or None if
/// any segment isn't a bare identifier.
fn flatten_ident_chain(expr: &Expr) -> Option<String> {
    match &expr.kind {
        ExprKind::Ident(id) => Some(id.name.clone()),
        ExprKind::FieldAccess { receiver, field } => {
            let head = flatten_ident_chain(receiver)?;
            Some(format!("{head}.{}", field.name))
        }
        _ => None,
    }
}

/// Type-check a cross-context service call (v0.6 §4.2). `receiver` carries
/// the prefix's source span for diagnostics. `consumed` is the resolved
/// qualified name of the consumed context.
/// v0.15: type-check a cross-context capability call `B.Cap.op(args)` /
/// `Alias.Cap.op(args)`. The capability operation signatures are carried in
/// `consumed_capabilities` (in the providing context's namespace); the
/// capability must be listed in the handler/provider's `given` clause.
#[allow(clippy::too_many_arguments)]
fn check_cross_context_capability_call(
    receiver: &Expr,
    consumed: &str,
    cap: &str,
    method: &Ident,
    // #926 (Decision G): explicit type arguments for a generic capability
    // operation, qualified form (`B.Cap.op[T](…)` / `Alias.Cap.op[T](…)`).
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    // P6.0 (#1139): the outer `MethodCall` expression's own identity.
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // S3 (#1963): a qualified capability's operation, `U.Cap.op`.
    refuse_experimental(
        &format!("{consumed}.{cap}.{}", method.name),
        method.span,
        ctx,
    );
    ctx.callees.insert(
        expr_id,
        Callee::CrossCap {
            unit: consumed.to_string(),
            cap: cap.to_string(),
            op: method.name.clone(),
            wire: None,
        },
    );
    // Capability calls require an effectful body (same rule as local ones).
    if !ctx.effectful {
        ctx.errors.push(CompileError::new(
            "bynk.effect.capability_in_pure_context",
            method.span,
            format!(
                "capability `{consumed}.{cap}` can only be called inside an effectful body (one returning `Effect[T]`)"
            ),
        ));
    }
    // The capability must be declared in this handler/provider's `given`.
    // The local deps key is the capability's simple name.
    if !ctx.caps.given_remaining.contains(cap) {
        let mut err = CompileError::new(
            "bynk.given.undeclared_capability",
            receiver.span,
            format!("capability `{consumed}.{cap}` is used but not listed in the `given` clause"),
        )
        .with_note(format!(
            "add `{consumed}.{cap}` to the handler's `given` clause so the dependency surface is visible at the declaration site"
        ));
        // v0.26 (ADR 0054): the one-click counterpart of the note — the
        // clause entry is the qualified form the user writes (`B.Cap`).
        if let Some((span, insert)) = given_insertion_edit(
            &ctx.caps.given_entries,
            ctx.caps.given_anchor,
            &format!("{consumed}.{cap}"),
        ) {
            err = err.with_suggestion(
                format!("add `{consumed}.{cap}` to the `given` clause"),
                vec![(span, insert)],
                Applicability::MachineApplicable,
            );
        }
        ctx.errors.push(err);
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    ctx.caps.given_used.insert(cap.to_string());

    let info = &ctx.input.cross_context;
    let op = info
        .consumed_capabilities
        .get(consumed)
        .and_then(|caps| caps.get(cap))
        .and_then(|c| c.ops.iter().find(|o| o.name == method.name))
        .cloned();
    let Some(op) = op else {
        ctx.errors.push(CompileError::new(
            "bynk.capability.unknown_operation",
            method.span,
            format!(
                "capability `{consumed}.{cap}` has no operation named `{}`",
                method.name
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    // v0.36 (ADR 0069, slice 2): a cross-context op call references the op,
    // recorded already-qualified into the providing unit (where the op is
    // declared), mirroring the cross-context capability reference.
    ctx.refs.record_in_unit(
        method.span,
        SymbolKind::CapabilityOp,
        &format!("{cap}.{}", method.name),
        consumed,
    );
    if op.params.len() != args.len() {
        ctx.errors.push(CompileError::new(
            "bynk.capability.op_arity",
            method.span,
            format!(
                "capability operation `{consumed}.{cap}.{}` expects {} argument(s), but {} were given",
                method.name,
                op.params.len(),
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }

    // Resolve parameter / return types in the consumed context's namespace.
    let consumed_types = info
        .consumed_types
        .get(consumed)
        .cloned()
        .unwrap_or_default();
    // #926: resolve the op's own type parameter(s) — declared in the
    // *consumed* context's namespace but instantiated from an explicit type
    // argument named in the *calling* context's own namespace (same split
    // `resolve_type_ref`/`resolve_expr_type_ref` already draw for
    // params/return vs. call-site type args in the local-capability path,
    // `check_static_call`). Explicit only, never inferred.
    let vars: HashSet<String> = op.type_params.iter().cloned().collect();
    let mut subst: HashMap<String, TyId> = HashMap::new();
    if !op.type_params.is_empty() || !type_args.is_empty() {
        if type_args.is_empty() {
            ctx.errors.push(
                CompileError::new(
                    "bynk.generics.uninferable_type_arg",
                    method.span,
                    format!(
                        "capability operation `{consumed}.{cap}.{}` takes a type parameter, but none of its arguments determine it",
                        method.name
                    ),
                )
                .with_note(format!(
                    "a qualified cross-context call takes its type arguments explicitly: `{consumed}.{cap}.{}[T](…)`",
                    method.name
                )),
            );
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        }
        if type_args.len() != op.type_params.len() {
            ctx.errors.push(CompileError::new(
                "bynk.generics.type_arg_mismatch",
                method.span,
                format!(
                    "capability operation `{consumed}.{cap}.{}` takes {} type argument(s), but {} were given",
                    method.name,
                    op.type_params.len(),
                    type_args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        }
        for (tp, ta) in op.type_params.iter().zip(type_args) {
            let ty = resolve_expr_type_ref(ta, ctx)?;
            subst.insert(tp.clone(), ty);
        }
    }
    // Slice 3: each `Codable` type parameter takes a codable type, and the call
    // records how it encodes the parameters that mention one.
    if !op.codable_params.is_empty() {
        if !check_op_codable_args(
            &format!("{consumed}.{cap}.{}", method.name),
            method.span,
            &op.codable_params,
            &subst,
            ctx,
        ) {
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        }
        let patterns: Vec<TyId> = op
            .params
            .iter()
            .map(|(_, tr)| {
                resolve_type_ref_in(tr, &consumed_types, &vars, tys).unwrap_or(tys.intern(Ty::Unit))
            })
            .collect();
        let ret_pattern = resolve_type_ref_in(&op.return_type, &consumed_types, &vars, tys)
            .unwrap_or(tys.intern(Ty::Unit));
        if let Some(wire) = op_encoding(
            &op.type_params,
            &op.codable_params,
            &patterns,
            ret_pattern,
            &subst,
            tys,
        ) {
            ctx.callees.insert(
                expr_id,
                Callee::CrossCap {
                    unit: consumed.to_string(),
                    cap: cap.to_string(),
                    op: method.name.clone(),
                    wire: Some(wire),
                },
            );
        }
    }
    // #1941: a value this caller holds may be of a consumed context's
    // unexported type, which only the projection resolves.
    let (caller_types, ambiguous) = projected_types(&ctx.input.types, &info.consumed_types, tys);
    let mut all_ok = true;
    for (i, ((pname, ptype_ref), arg)) in op.params.iter().zip(args.iter()).enumerate() {
        record_param_hint(ctx.hints, pname, arg);
        let param_ty = resolve_type_ref_in(ptype_ref, &consumed_types, &vars, tys)
            .unwrap_or(tys.intern(Ty::Unit));
        let param_ty = substitute(param_ty, &subst, tys);
        // #2025: against the parameter's type; see `type_cross_context_arg`.
        let (arg_ty, crosses) =
            type_cross_context_arg(arg, param_ty, &caller_types, &consumed_types, consumed, ctx);
        let Some(arg_ty) = arg_ty else {
            all_ok = false;
            continue;
        };
        if !crosses {
            let mut err = CompileError::new(
                "bynk.boundary.structural_mismatch",
                arg.span,
                format!(
                    "cross-context argument {} to `{consumed}.{cap}.{}` has type `{}`, but parameter `{pname}` expects `{}`",
                    i + 1,
                    method.name,
                    arg_ty.display(tys),
                    param_ty.display(tys),
                ),
            );
            if let Some(note) = ambiguous_projection_note(arg_ty, &ambiguous, tys) {
                err = err.with_note(note);
            }
            ctx.errors.push(err);
            all_ok = false;
        }
    }
    if !all_ok {
        return None;
    }
    let raw_ret = resolve_type_ref_in(&op.return_type, &consumed_types, &vars, tys)
        .unwrap_or(tys.intern(Ty::Unit));
    let raw_ret = substitute(raw_ret, &subst, tys);
    if let Some(name) = rebrand_conflict(
        raw_ret,
        &consumed_types,
        &ctx.input.types,
        &caller_types,
        tys,
    ) {
        ctx.errors.push(rebranded_return_mismatch(
            &format!("{consumed}.{cap}.{}", method.name),
            &name,
            consumed,
            span,
            ctx,
        ));
        return None;
    }
    Some(rebrand_return_type(raw_ret, &ctx.input.types, tys))
}

/// #1941: a returned value takes the caller's type of the same name (§6.5).
/// When that type has a different shape, the value is not one of it: the
/// caller's own `Box` would otherwise type a field read of the provider's.
fn rebranded_return_mismatch(
    callee: &str,
    name: &str,
    consumed: &str,
    span: Span,
    ctx: &Ctx,
) -> CompileError {
    let caller = ctx
        .input
        .cross_context
        .self_context
        .as_deref()
        .unwrap_or("?");
    CompileError::new(
        "bynk.boundary.structural_mismatch",
        span,
        format!(
            "cross-context call `{callee}` returns `{consumed}`'s type `{name}`, but `{name}` in \
             `{caller}` is a different type"
        ),
    )
    .with_note(
        "a returned value takes the receiving context's type of the same name, so the two \
         must be structurally compatible; rename one of them",
    )
}

/// #2025, #2039, #2072: type one cross-context argument against its
/// parameter's type, returning the type and whether the argument crosses the
/// boundary.
///
/// A literal at a refined position in the parameter's type is admitted
/// against that type's own declaration, from the consumed context's table.
/// This covers a literal passed directly (`t.pay.charge(250)`) and one
/// nested in an `Option`/`Result` payload, a list element, or a field of a
/// record literal, a generic one included (`t.pay.maybe(Some(250))`,
/// `Page { items: [250] }`). It is checked against the callee's predicate,
/// whether or not the caller `uses` the declaring commons and whatever the
/// caller binds under that name. Such a literal is the callee's type by
/// construction, so it needs no structural check; neither does a `None` or
/// an empty list, which take the parameter's type as their hint.
///
/// Every other part of the argument is a *leaf*, typed in the caller's own
/// scope with its position's type as the hint, and the structural check
/// decides whether each leaf crosses. A shorthand record field (`{ cents }`)
/// is a leaf too, checked by its binding's type. An argument with no literal
/// or empty constructor in it is a single leaf, so it is checked whole, as
/// before; a leaf `type_of` did not record falls back to the whole argument
/// too.
fn type_cross_context_arg(
    arg: &Expr,
    param_ty: TyId,
    caller_types: &HashMap<String, Arc<TypeDecl>>,
    consumed_types: &HashMap<String, Arc<TypeDecl>>,
    consumed: &str,
    ctx: &mut Ctx,
) -> (Option<TyId>, bool) {
    let tys = ctx.tys;
    let mut found = CrossingPositions::default();
    collect_crossing_positions(arg, param_ty, caller_types, consumed_types, tys, &mut found);
    let CrossingPositions {
        literals,
        mut leaves,
        bindings,
    } = found;
    for (lit, pos) in literals {
        let before = ctx.errors.len();
        match admit_refined_literal_in(lit, Some(pos), consumed_types, ctx) {
            Some(ty) => {
                for err in &mut ctx.errors[before..] {
                    err.notes.push(format!(
                        "`{}` is the type at this position of the parameter, as `{consumed}` declares it",
                        ty.display(tys)
                    ));
                }
                ctx.admitted_literals.insert(lit.id, ty);
            }
            // Not a literal of the refined type's base: an ordinary leaf.
            None => leaves.push((lit, pos)),
        }
    }
    let arg_ty = type_of(arg, Some(param_ty), ctx);
    let Some(ty) = arg_ty else {
        return (None, false);
    };
    let whole = || structurally_compatible(ty, param_ty, caller_types, consumed_types, tys);
    let leaves_cross = leaves.iter().all(|(leaf, pos)| {
        // A leaf `type_of` did not record is checked through the whole
        // argument instead.
        match ctx.expr_types.get(&leaf.id) {
            Some(t) => structurally_compatible(t.ty, *pos, caller_types, consumed_types, tys),
            None => whole(),
        }
    });
    let bindings_cross = bindings.iter().all(|(name, pos)| match ctx.lookup(name) {
        Some(t) => structurally_compatible(t, *pos, caller_types, consumed_types, tys),
        None => whole(),
    });
    (arg_ty, leaves_cross && bindings_cross)
}

/// #2039: what [`collect_crossing_positions`] finds in a cross-context
/// argument, each paired with the type at its position, named in the
/// consumed context's table.
#[derive(Default)]
struct CrossingPositions<'e> {
    /// Literals at refined positions, admitted against the callee.
    literals: Vec<(&'e Expr, TyId)>,
    /// Parts the structural check still has to decide.
    leaves: Vec<(&'e Expr, TyId)>,
    /// #2072: shorthand record fields, by the binding each one reads.
    bindings: Vec<(&'e str, TyId)>,
}

/// #2039: walk a cross-context argument alongside its parameter's type,
/// collecting the literals at refined positions and the leaves the
/// structural check still has to decide.
///
/// The walk follows `Some`/`Ok`/`Err` payloads, list elements, and the
/// fields of a record literal whose type is the very declaration the
/// callee's parameter names (the same commons type). #2072: for a generic
/// record, its type arguments are substituted into the field types, and a
/// shorthand field is collected by its binding. Anything else is a leaf.
fn collect_crossing_positions<'e>(
    e: &'e Expr,
    pos: TyId,
    caller_types: &HashMap<String, Arc<TypeDecl>>,
    consumed_types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
    found: &mut CrossingPositions<'e>,
) {
    let node = tys.get(pos);
    let walk = |inner: &'e Expr, at: TyId, found: &mut CrossingPositions<'e>| {
        collect_crossing_positions(inner, at, caller_types, consumed_types, tys, found)
    };
    // A compile-time literal, negated ones (`-3`) included.
    if const_literal(e).is_some()
        && matches!(
            &*node,
            Ty::Named {
                kind: NamedKind::Refined(_),
                ..
            }
        )
    {
        return found.literals.push((e, pos));
    }
    match (&e.kind, &*node) {
        (ExprKind::Paren(inner), _) => walk(inner, pos, found),
        // Typed from the hint: the parameter's own type at this position.
        (ExprKind::None, Ty::Option(_)) => {}
        (ExprKind::ListLit(items), Ty::List(_)) if items.is_empty() => {}
        // #2043: `List.empty()` / `Map.empty()`, likewise.
        (
            ExprKind::MethodCall {
                receiver,
                method,
                type_args,
                args,
            },
            Ty::List(_) | Ty::Map(..) | Ty::Set(_),
        ) if method.name == "empty"
            && type_args.is_empty()
            && args.is_empty()
            && matches!(
                &receiver.kind,
                ExprKind::Ident(id)
                    if id.name == match &*node {
                        Ty::List(_) => "List",
                        Ty::Set(_) => "Set",
                        _ => "Map",
                    }
            ) => {}
        (ExprKind::Some(inner), Ty::Option(t)) => walk(inner, *t, found),
        (ExprKind::Ok(inner), Ty::Result(t, _)) => walk(inner, *t, found),
        (ExprKind::Err(inner), Ty::Result(_, t)) => walk(inner, *t, found),
        (ExprKind::ListLit(items), Ty::List(t)) => {
            for item in items {
                walk(item, *t, found);
            }
        }
        (
            ExprKind::RecordConstruction { type_name, fields },
            Ty::Named {
                name,
                kind: NamedKind::Record,
                args,
            },
        ) if type_name.name == *name => {
            let Some(field_positions) =
                record_field_positions(name, args, fields, caller_types, consumed_types, tys)
            else {
                return found.leaves.push((e, pos));
            };
            for (f, field_ty) in field_positions {
                match &f.value {
                    Some(value) => walk(value, field_ty, found),
                    // #2072: a shorthand field reads the same-named binding.
                    None => found.bindings.push((f.name.name.as_str(), field_ty)),
                }
            }
        }
        _ => found.leaves.push((e, pos)),
    }
}

/// #2039, #2072: the type of each field a record literal sets, in the
/// consumed context's table, when the literal's type is the very
/// declaration the callee names (the same commons type). A generic record's
/// type arguments are substituted into its field types. `None` when the
/// record is not the callee's declaration, or a field is not one it
/// declares: the record is then checked whole.
fn record_field_positions<'f>(
    name: &str,
    args: &[TyId],
    fields: &'f [FieldInit],
    caller_types: &HashMap<String, Arc<TypeDecl>>,
    consumed_types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> Option<Vec<(&'f FieldInit, TyId)>> {
    let decl = consumed_types.get(name)?;
    if !caller_types.get(name).is_some_and(|c| Arc::ptr_eq(c, decl)) {
        return None;
    }
    let TypeBody::Record(record) = &decl.body else {
        return None;
    };
    if decl.type_params.len() != args.len() {
        return None;
    }
    let vars: HashSet<String> = decl
        .type_params
        .iter()
        .map(|p| p.name.name.clone())
        .collect();
    let subst: HashMap<String, TyId> = decl
        .type_params
        .iter()
        .map(|p| p.name.name.clone())
        .zip(args.iter().copied())
        .collect();
    fields
        .iter()
        .map(|f| {
            let declared = record.fields.iter().find(|d| d.name.name == f.name.name)?;
            let ty = resolve_type_ref_in(&declared.type_ref, consumed_types, &vars, tys)?;
            Some((f, substitute(ty, &subst, tys)))
        })
        .collect()
}

/// #2073: a consumed context's service whose signature reaches a type that
/// context consumes from a third (`shop.ledger.isDomestic(o: Order)`, `Order`
/// from `shop.order`) crosses the boundary as that third context's type. The
/// caller names it, and builds its codecs, through the declaring context, so
/// it must consume that context too; otherwise `bynk.consumes.signature_type_unconsumed`.
fn check_signature_owners_consumed(
    consumed: &str,
    service: &crate::resolver::CrossContextService,
    method: &Ident,
    ctx: &mut Ctx,
) -> bool {
    let info = &ctx.input.cross_context;
    let Some(owners) = info.consumed_type_owners.get(consumed) else {
        return false;
    };
    let types = info.consumed_types.get(consumed);
    let mut seen: HashSet<String> = HashSet::new();
    let mut stack: Vec<String> = Vec::new();
    let vars = HashSet::new();
    for (_, t) in &service.params {
        crate::project_model::type_ref_names(t, &vars, &mut stack);
    }
    crate::project_model::type_ref_names(&service.return_type, &vars, &mut stack);
    let mut missing: std::collections::BTreeMap<String, String> = Default::default();
    while let Some(n) = stack.pop() {
        if !seen.insert(n.clone()) {
            continue;
        }
        if let Some(owner) = owners.get(&n)
            && info.self_context.as_deref() != Some(owner.as_str())
            && !info.consumed_contexts.contains(owner)
        {
            // The declaration's own name, not the key `contract_types_for`
            // may have qualified it under (`shop_base__Region`).
            let declared = n
                .strip_prefix(&format!("{}__", owner.replace('.', "_")))
                .unwrap_or(&n)
                .to_string();
            missing.entry(owner.clone()).or_insert(declared);
        }
        if let Some(d) = types.and_then(|t| t.get(&n)) {
            stack.extend(crate::project_model::type_decl_ref_names(d));
        }
    }
    let refused = !missing.is_empty();
    for (owner, ty) in missing {
        ctx.errors.push(
            CompileError::new(
                "bynk.consumes.signature_type_unconsumed",
                method.span,
                format!(
                    "`{consumed}.{}` takes or returns `{ty}`, declared in `{owner}`, but this context does not consume `{owner}`",
                    method.name
                ),
            )
            .with_note(format!(
                "add `consumes {owner}`: a value of `{ty}` crosses this call as `{owner}`'s type"
            )),
        );
    }
    refused
}

fn check_cross_context_call(
    receiver: &Expr,
    consumed: &str,
    method: &Ident,
    args: &[Expr],
    span: Span,
    // P6.0 (#1139): the outer `MethodCall` expression's own identity.
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    ctx.callees.insert(
        expr_id,
        Callee::Cross {
            unit: consumed.to_string(),
            service: method.name.clone(),
        },
    );
    // The consuming context must be effectful at this call site (services
    // and agent handlers are; pure free fns are not).
    if !ctx.effectful {
        ctx.errors.push(
            CompileError::new(
                "bynk.effect.cross_context_in_pure_context",
                method.span,
                format!(
                    "cross-context service call `{}.{}` can only be made inside an effectful body (one returning `Effect[T]`)",
                    consumed, method.name
                ),
            )
            .with_label(receiver.span, "consumed context prefix"),
        );
    }
    let info = &ctx.input.cross_context;
    let Some(svcs) = info.consumed_services.get(consumed) else {
        ctx.errors.push(
            CompileError::new(
                "bynk.consumes.unknown_context",
                receiver.span,
                format!("context `{consumed}` is not in scope here"),
            )
            .with_note(
                "add a `consumes` clause for the target context at the top of the consuming context",
            ),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    let Some(service) = svcs.get(&method.name).cloned() else {
        ctx.errors.push(
            CompileError::new(
                "bynk.consumes.unknown_service",
                method.span,
                format!(
                    "context `{consumed}` has no service named `{}`",
                    method.name
                ),
            )
            .with_note(
                "cross-context calls require an `on call` service handler in the consumed context",
            ),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    ctx.refs
        .record_in_unit(method.span, SymbolKind::Service, &method.name, consumed);
    // A refused signature's arguments would only repeat it as a mismatch.
    let unconsumed = check_signature_owners_consumed(consumed, &service, method, ctx);

    if service.params.len() != args.len() {
        ctx.errors.push(
            CompileError::new(
                "bynk.consumes.service_arity",
                method.span,
                format!(
                    "cross-context service `{consumed}.{}` expects {} argument(s), but {} were given",
                    method.name,
                    service.params.len(),
                    args.len()
                ),
            )
            // Finding #46: `service` belongs to the *consumed*
            // context — a different unit/file than this call, almost
            // always. A label would risk underlining unrelated text
            // there; a note carries the information safely (per-label
            // file identity is a Wave 8 follow-up).
            .with_note("service declared here"),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }

    // Resolve the consumed-context types so we can describe parameter shapes.
    let consumed_types = info
        .consumed_types
        .get(consumed)
        .cloned()
        .unwrap_or_default();

    // #1941: as for a capability operation, the caller's table plus the
    // consumed contexts' types it holds values of without naming.
    let (caller_types, ambiguous) = projected_types(&ctx.input.types, &info.consumed_types, tys);

    // Walk each argument, checking structural compatibility (Phase 4).
    let mut all_ok = true;
    for (i, ((pname, ptype_ref), arg)) in service.params.iter().zip(args.iter()).enumerate() {
        record_param_hint(ctx.hints, pname, arg);
        let param_ty =
            resolve_type_ref(ptype_ref, &consumed_types, tys).unwrap_or(tys.intern(Ty::Unit));
        // Type-check the argument in the caller's context (#2025: against
        // the parameter's type; see `type_cross_context_arg`).
        let (arg_ty, crosses) =
            type_cross_context_arg(arg, param_ty, &caller_types, &consumed_types, consumed, ctx);
        let Some(arg_ty) = arg_ty else {
            all_ok = false;
            continue;
        };
        if !crosses && !unconsumed {
            let mut err = CompileError::new(
                    "bynk.boundary.structural_mismatch",
                    arg.span,
                    format!(
                        "cross-context argument {} to `{consumed}.{}` has type `{}` in `{}`, but parameter `{pname}` expects `{}` in `{}`",
                        i + 1,
                        method.name,
                        arg_ty.display(tys),
                        ctx.input
                            .cross_context
                            .self_context
                            .as_deref()
                            .unwrap_or("?"),
                        param_ty.display(tys),
                        consumed,
                    ),
                )
                // Finding #46: same cross-unit provenance as above.
                .with_note("service declared here")
                .with_note(
                    "values crossing a context boundary must have structurally compatible types (same commons-derived type, or identical record/sum shape)",
                );
            if let Some(note) = ambiguous_projection_note(arg_ty, &ambiguous, tys) {
                err = err.with_note(note);
            }
            ctx.errors.push(err);
            all_ok = false;
        }
    }
    if !all_ok {
        return None;
    }

    // Return type rebrand: project the consumed context's return type into
    // the calling context's namespace by renaming named types whose unqualified
    // name appears in the caller's type table (v0.6 §4.5).
    let raw_ret = resolve_type_ref(&service.return_type, &consumed_types, tys)
        .unwrap_or(tys.intern(Ty::Unit));
    if let Some(name) = rebrand_conflict(
        raw_ret,
        &consumed_types,
        &ctx.input.types,
        &caller_types,
        tys,
    ) {
        ctx.errors.push(rebranded_return_mismatch(
            &format!("{consumed}.{}", method.name),
            &name,
            consumed,
            span,
            ctx,
        ));
        return None;
    }
    Some(rebrand_return_type(raw_ret, &ctx.input.types, tys))
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    /// A single-file `commons demo` project.
    fn commons(body: &str) -> crate::testkit::Analysed {
        analyse(&[("demo.bynk", &format!("commons demo\n\n{body}"))])
    }

    /// A single-file `context demo` project.
    fn context(body: &str) -> crate::testkit::Analysed {
        analyse(&[("demo.bynk", &format!("context demo\n\n{body}"))])
    }

    const CLOCK: &str = "capability Clock {\n  fn now() -> Effect[Int]\n}\n\n\
                         provides Clock = FixedClock {\n  fn now() -> Effect[Int] {\n    Effect.pure(0)\n  }\n}\n";

    #[test]
    fn a_call_to_a_declared_fn_has_its_return_type() {
        let a = commons("fn double(x: Int) -> Int { x * 2 }\n\nfn four() -> Int { double(2) }\n");
        a.assert_clean();
        assert_eq!(a.type_at("demo.bynk", "double(2)"), "Int");
    }

    #[test]
    fn an_argument_of_the_wrong_type_is_a_mismatch() {
        let a = commons(
            "fn double(x: Int) -> Int { x * 2 }\n\nfn bad(s: String) -> Int { double(s) }\n",
        );
        a.assert_reports("bynk.types.argument_mismatch");
    }

    #[test]
    fn a_fn_called_with_too_many_arguments_is_an_arity_error() {
        let a = commons("fn double(x: Int) -> Int { x * 2 }\n\nfn bad() -> Int { double(1, 2) }\n");
        a.assert_reports("bynk.resolve.arity_mismatch");
    }

    #[test]
    fn a_fn_value_called_with_too_many_arguments_is_a_call_arity_error() {
        let a = commons("fn f(g: (Int) -> Int) -> Int { g(1, 2) }\n");
        a.assert_reports("bynk.types.call_arity");
    }

    #[test]
    fn a_fn_value_call_has_the_value_s_return_type() {
        let a = commons("fn f(g: (Int) -> String) -> String { g(1) }\n");
        a.assert_clean();
        assert_eq!(a.type_at("demo.bynk", "g(1)"), "String");
    }

    #[test]
    fn a_generic_call_infers_its_type_argument_from_the_argument() {
        let a = commons("fn id[A](x: A) -> A { x }\n\nfn s() -> String { id(\"s\") }\n");
        a.assert_clean();
        assert_eq!(a.type_at("demo.bynk", "id(\"s\")"), "String");
    }

    #[test]
    fn a_generic_call_binding_one_type_argument_two_ways_is_a_conflict() {
        let a =
            commons("fn both[A](x: A, y: A) -> A { x }\n\nfn bad() -> Int { both(1, \"two\") }\n");
        a.assert_reports("bynk.generics.type_arg_mismatch");
    }

    #[test]
    fn a_type_argument_no_parameter_mentions_is_uninferable() {
        let a = commons(
            "fn first_of[A](x: Int) -> A { identity_helper(x) }\n\n\
             fn identity_helper[A](x: A) -> A { x }\n\n\
             fn misuse() -> Int { first_of(5) }\n",
        );
        a.assert_reports("bynk.generics.uninferable_type_arg");
    }

    #[test]
    fn calling_an_undeclared_name_is_an_unknown_function() {
        let a = commons("fn bad() -> Int { nope(1) }\n");
        a.assert_reports("bynk.resolve.unknown_function");
    }

    #[test]
    fn calling_a_record_type_like_a_function_is_rejected() {
        let a = commons("type P = { a: Int }\n\nfn f() -> P { P(1) }\n");
        a.assert_reports("bynk.resolve.type_as_function");
    }

    #[test]
    fn a_capability_op_call_has_the_op_s_return_type() {
        let a = context(&format!(
            "{CLOCK}\nservice probe {{\n  on call() -> Effect[Int] given Clock {{\n    Clock.now()\n  }}\n}}\n"
        ));
        a.assert_clean();
        assert_eq!(a.type_at("demo.bynk", "Clock.now()"), "Effect[Int]");
    }

    #[test]
    fn a_capability_op_called_with_an_extra_argument_is_an_arity_error() {
        let a = context(&format!(
            "{CLOCK}\nservice probe {{\n  on call() -> Effect[Int] given Clock {{\n    Clock.now(1)\n  }}\n}}\n"
        ));
        a.assert_reports("bynk.capability.op_arity");
    }

    #[test]
    fn a_capability_called_inside_a_pure_lambda_is_rejected() {
        let a = context(&format!(
            "{CLOCK}\nservice probe {{\n  on call() -> Effect[Int] given Clock {{\n    \
             let f: (Int) -> Int = (n) => Clock.now()\n    Effect.pure(1)\n  }}\n}}\n"
        ));
        a.assert_reports("bynk.effect.capability_in_pure_context");
    }

    #[test]
    fn an_unknown_cell_op_is_rejected() {
        let a = context(
            "agent Counter {\n  key id: String\n  store n: Cell[Int] = 0\n\n  \
             on call bad() -> Effect[()] {\n    let _ <- n.foo()\n    Effect.pure(())\n  }\n}\n",
        );
        a.assert_reports("bynk.store.unknown_op");
    }

    #[test]
    fn a_cell_update_is_an_effect_of_unit() {
        let a = context(
            "agent Counter {\n  key id: String\n  store n: Cell[Int] = 0\n\n  \
             on call bump() -> Effect[()] {\n    n.update((c) => c + 1)\n  }\n}\n",
        );
        a.assert_clean();
        assert_eq!(
            a.type_at("demo.bynk", "n.update((c) => c + 1)"),
            "Effect[()]"
        );
    }

    #[test]
    fn a_cell_update_whose_function_returns_another_type_is_a_mismatch() {
        let a = context(
            "agent Counter {\n  key id: String\n  store n: Cell[Int] = 0\n\n  \
             on call bad() -> Effect[()] {\n    n.update((c) => \"x\")\n  }\n}\n",
        );
        a.assert_reports("bynk.types.lambda_mismatch");
    }

    #[test]
    fn a_cell_update_with_no_function_is_a_call_arity_error() {
        let a = context(
            "agent Counter {\n  key id: String\n  store n: Cell[Int] = 0\n\n  \
             on call bad() -> Effect[()] {\n    n.update()\n  }\n}\n",
        );
        a.assert_reports("bynk.types.call_arity");
    }

    /// #1941: a provider context with an unexported `Box` (and its field's
    /// `Amount`), a service returning one and a service taking one back, and
    /// a capability operation doing the same.
    const SHOP: &str = "context shop\n\nexports capability { Store }\n\n\
                        type Amount = { cents: Int }\n\ntype Box = { a: Amount }\n\n\
                        capability Store {\n  fn get() -> Effect[Box]\n  fn put(b: Box) -> Effect[Int]\n}\n\n\
                        provides Store = Shelf {\n  fn get() -> Effect[Box] {\n    Effect.pure(Box { a: Amount { cents: 1 } })\n  }\n\n  \
                        fn put(b: Box) -> Effect[Int] {\n    Effect.pure(b.a.cents)\n  }\n}\n\n\
                        service quote {\n  on call() -> Effect[Box] {\n    Effect.pure(Box { a: Amount { cents: 1 } })\n  }\n}\n\n\
                        service take {\n  on call(b: Box) -> Effect[Int] {\n    Effect.pure(b.a.cents)\n  }\n}\n";

    /// A `till` context consuming `shop`, declaring `decls`, with `body` as
    /// a service handler's body (given `shop.Store` when it calls it).
    fn till(decls: &str, body: &str) -> String {
        let given = if body.contains("Store") {
            " given shop.Store"
        } else {
            ""
        };
        format!(
            "context till\n\nconsumes shop\n\n{decls}service total {{\n  \
             on call() -> Effect[Int]{given} {{\n{body}\n  }}\n}}\n"
        )
    }

    #[test]
    fn an_unexported_value_passes_back_to_its_service() {
        let a = analyse(&[
            ("shop.bynk", SHOP),
            (
                "till.bynk",
                &till("", "    let b <- shop.quote()\n    shop.take(b)"),
            ),
        ]);
        a.assert_clean();
    }

    #[test]
    fn an_unexported_value_passes_back_to_its_capability() {
        let a = analyse(&[
            ("shop.bynk", SHOP),
            (
                "till.bynk",
                &till("", "    let b <- shop.Store.get()\n    shop.Store.put(b)"),
            ),
        ]);
        a.assert_clean();
    }

    #[test]
    fn a_caller_type_of_another_shape_cannot_receive_a_service_return() {
        let a = analyse(&[
            ("shop.bynk", SHOP),
            (
                "till.bynk",
                &till(
                    "type Box = { label: String }\n\n",
                    "    let b <- shop.quote()\n    Effect.pure(1)",
                ),
            ),
        ]);
        a.assert_reports("bynk.boundary.structural_mismatch");
    }

    #[test]
    fn a_caller_type_of_another_shape_cannot_receive_a_capability_return() {
        let a = analyse(&[
            ("shop.bynk", SHOP),
            (
                "till.bynk",
                &till(
                    "type Box = { label: String }\n\n",
                    "    let b <- shop.Store.get()\n    Effect.pure(1)",
                ),
            ),
        ]);
        a.assert_reports("bynk.boundary.structural_mismatch");
    }

    #[test]
    fn a_caller_type_of_the_same_shape_receives_a_return() {
        let a = analyse(&[
            ("shop.bynk", SHOP),
            (
                "till.bynk",
                &till(
                    "type Coins = { cents: Int }\n\ntype Box = { a: Coins }\n\n",
                    "    let b <- shop.quote()\n    let c <- shop.Store.get()\n    \
                     Effect.pure(b.a.cents + c.a.cents)",
                ),
            ),
        ]);
        a.assert_clean();
    }

    #[test]
    fn an_ambiguous_unexported_name_explains_its_rejection() {
        let other = "context depot\n\ntype Box = { label: String }\n\n\
                     service quote {\n  on call() -> Effect[Box] {\n    Effect.pure(Box { label: \"x\" })\n  }\n}\n";
        let a = analyse(&[
            ("shop.bynk", SHOP),
            ("depot.bynk", other),
            (
                "till.bynk",
                "context till\n\nconsumes shop\n\nconsumes depot\n\n\
                 service total {\n  on call() -> Effect[Int] {\n    \
                 let b <- shop.quote()\n    shop.take(b)\n  }\n}\n",
            ),
        ]);
        let err = a
            .analysis
            .errors
            .iter()
            .find(|e| e.error.category == "bynk.boundary.structural_mismatch")
            .expect("the argument is rejected");
        assert!(
            err.error
                .notes
                .iter()
                .any(|n| n.contains("more than one consumed context declares a type `Box`")),
            "{:?}",
            err.error.notes
        );
    }

    #[test]
    fn a_return_the_caller_does_not_rebrand_is_not_checked_against_a_projection() {
        // Review of #1949: `shop` and `depot` declare the same-shaped `Box`, so
        // the projection holds one of them (`depot`'s, in name order). `till`
        // declares no `Box`, so `quote`'s `Box` is not rebranded and needs
        // no check, however `depot`'s `Box.a` would resolve against `till`'s
        // own, unrelated `Amount`.
        let pair = |name: &str, svc: &str| {
            format!(
                "context {name}\n\ntype Amount = {{ cents: Int }}\n\ntype Box = {{ a: Amount }}\n\n{svc}"
            )
        };
        let shop = pair(
            "shop",
            "service quote {\n  on call() -> Effect[Box] {\n    Effect.pure(Box { a: Amount { cents: 1 } })\n  }\n}\n",
        );
        let depot = pair(
            "depot",
            "service store {\n  on call(b: Box) -> Effect[Int] {\n    Effect.pure(b.a.cents)\n  }\n}\n",
        );
        let a = analyse(&[
            ("shop.bynk", &shop),
            ("depot.bynk", &depot),
            (
                "till.bynk",
                "context till\n\nconsumes shop\n\nconsumes depot\n\n\
                 type Amount = { dollars: Int }\n\n\
                 fn mine() -> Amount {\n  Amount { dollars: 1 }\n}\n\n\
                 service total {\n  on call() -> Effect[Int] {\n    \
                 let b <- shop.quote()\n    Effect.pure(1)\n  }\n}\n",
            ),
        ]);
        a.assert_clean();
    }

    /// Review of #1949: a provider whose `Box` is returned inside each
    /// container a cross-context return can carry, for a caller whose own
    /// `Box` has another shape.
    fn caller_box_differs_inside(ret: &str, value: &str) {
        let shop = format!(
            "context shop\n\ntype Box = {{ a: Int }}\n\n\
             service quote {{\n  on call() -> Effect[{ret}] {{\n    Effect.pure({value})\n  }}\n}}\n"
        );
        let a = analyse(&[
            ("shop.bynk", &shop),
            (
                "till.bynk",
                "context till\n\nconsumes shop\n\ntype Box = { label: String }\n\n\
                 service total {\n  on call() -> Effect[Int] {\n    \
                 let b <- shop.quote()\n    Effect.pure(1)\n  }\n}\n",
            ),
        ]);
        a.assert_reports("bynk.boundary.structural_mismatch");
    }

    #[test]
    fn a_differing_caller_type_is_found_in_a_result() {
        caller_box_differs_inside("Result[Box, String]", "Ok(Box { a: 1 })");
    }

    #[test]
    fn a_differing_caller_type_is_found_in_a_map_value() {
        caller_box_differs_inside("Map[String, Box]", "Map.empty()");
    }

    #[test]
    fn a_differing_caller_type_is_found_in_an_option() {
        caller_box_differs_inside("Option[Box]", "None");
    }

    #[test]
    fn a_differing_caller_type_is_found_in_a_list() {
        caller_box_differs_inside("List[Box]", "[]");
    }

    #[test]
    fn a_differing_caller_type_is_found_in_an_http_result() {
        caller_box_differs_inside("HttpResult[Box]", "Ok(Box { a: 1 })");
    }

    #[test]
    fn agreeing_declarations_project_in_context_name_order() {
        // Review of #1949: `depot`'s `Box` names `Coins` and `shop`'s names
        // `Amount`, of the same shape, so the two agree and either may be
        // projected. `till`'s own `Amount` differs: `shop`'s `Box` would
        // read its field as `till`'s, and `depot`'s would not. The projection
        // takes contexts in name order, so `depot`'s is the one, on every run.
        let a = analyse(&[
            (
                "shop.bynk",
                "context shop\n\ntype Amount = { cents: Int }\n\ntype Box = { a: Amount }\n\n\
                 service quote {\n  on call() -> Effect[Box] {\n    \
                 Effect.pure(Box { a: Amount { cents: 1 } })\n  }\n}\n",
            ),
            (
                "depot.bynk",
                "context depot\n\ntype Coins = { cents: Int }\n\ntype Box = { a: Coins }\n\n\
                 service store {\n  on call(b: Box) -> Effect[Int] {\n    Effect.pure(b.a.cents)\n  }\n}\n",
            ),
            (
                "till.bynk",
                "context till\n\nconsumes shop\n\nconsumes depot\n\n\
                 type Amount = { dollars: Int }\n\n\
                 fn mine() -> Amount {\n  Amount { dollars: 1 }\n}\n\n\
                 service total {\n  on call() -> Effect[Int] {\n    \
                 let b <- shop.quote()\n    depot.store(b)\n  }\n}\n",
            ),
        ]);
        a.assert_clean();
    }
}
