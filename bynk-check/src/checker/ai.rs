//! #4099: the checker's half of `Ai.generateValue[T]`.
//!
//! `T` is the shape the model is asked for, so the compiler writes `T`'s JSON Schema into the call
//! ([`crate::ai_schema`]) and the emitter lowers the call over the binding's JSON-mode generation,
//! decoding the reply as `T` at the call site. `generateValue` is not a declared operation of
//! `Ai`, like `AnalyticsSql.select`: nothing but the compiler can supply the schema.

use super::*;

/// Whether `Ai.generateValue` is the first-party `Ai` capability's typed generation.
pub(crate) fn is_generate_value(cap: &str, method: &str, ctx: &Ctx) -> bool {
    cap == "Ai"
        && method == "generateValue"
        && ctx
            .input
            .cross_context
            .flattened_caps
            .get(cap)
            .is_some_and(|unit| unit == crate::firstparty::CLOUDFLARE_UNIT)
}

/// The typed operation of a capability call, or `None` when the call is not one. The inner
/// `Option` is the call's type, `None` when it is refused.
pub(crate) fn check_typed(
    cap: &str,
    method: &Ident,
    (type_args, args): (&[TypeRef], &[Expr]),
    (span, expr_id): (Span, ExprId),
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    is_generate_value(cap, &method.name, ctx)
        .then(|| check_generate_value(cap, method, type_args, args, span, expr_id, ctx))
}

fn named(ctx: &Ctx, name: &str, kind: NamedKind) -> Option<TyId> {
    ctx.input.types.contains_key(name).then(|| {
        ctx.tys.intern(Ty::Named {
            name: name.to_string(),
            kind,
            args: Vec::new(),
        })
    })
}

fn check_generate_value(
    cap: &str,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    super::calls::refuse_experimental("bynk.cloudflare.Ai.generateValue", method.span, ctx);
    let name = format!("{cap}.{}", method.name);
    let request_ty = named(ctx, "GenerateRequest", NamedKind::Record);
    let bail = |ctx: &mut Ctx| {
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        None
    };
    if type_args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.generics.type_arg_mismatch",
            span,
            format!(
                "`{name}` takes exactly one type argument (the shape of the reply), got {}",
                type_args.len()
            ),
        ));
        return bail(ctx);
    }
    if args.len() != 2 {
        ctx.errors.push(
            CompileError::new(
                "bynk.capability.op_arity",
                span,
                format!(
                    "capability operation `{name}` expects 2 argument(s), but {} were given",
                    args.len()
                ),
            )
            .with_note(
                "pass the model and the request: `Ai.generateValue[Triage](\"@cf/meta/llama-3.3-70b-instruct-fp8-fast\", GenerateRequest { … })`",
            ),
        );
        return bail(ctx);
    }
    let string_ty = tys.intern(Ty::Base(BaseType::String));
    for (i, (arg, expected)) in args.iter().zip([Some(string_ty), request_ty]).enumerate() {
        let Some(expected) = expected else {
            let _ = type_of(arg, None, ctx);
            continue;
        };
        if let Some(actual) = type_of(arg, Some(expected), ctx)
            && !compatible(actual, expected, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                arg.span,
                format!(
                    "argument {} to capability `{name}` has type `{}`, but parameter expects `{}`",
                    i + 1,
                    actual.display(tys),
                    expected.display(tys)
                ),
            ));
        }
    }
    let t = resolve_expr_type_ref(&type_args[0], ctx)?;
    let ai_error = named(ctx, "AiError", NamedKind::Sum)?;
    // The reply is decoded as `T`, so `T` is a type the decoder reads.
    if !codable::check_codable(
        ctx,
        t,
        type_args[0].span(),
        &codable::CodableSite::Op {
            op: &name,
            param: "T",
        },
    ) {
        return None;
    }
    let shown = t.display(tys);
    let request = match crate::ai_schema::request_schema(&type_args[0], &shown, &ctx.input.types) {
        Ok(request) => request,
        Err(why) => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.types.ai_value_shape",
                    type_args[0].span(),
                    format!(
                        "`{name}` asks the model for `{shown}`, but `{}` has no JSON Schema: {}",
                        why.path, why.reason
                    ),
                )
                .with_note(
                    "the model is given `T`'s schema, so `T` must be a type a schema can describe: records, enums, sums, lists, `Option` and refined base types; not a type that contains itself, a generic type or a `Map` with a key that is not a `String`",
                ),
            );
            return None;
        }
    };
    ctx.callees.insert(
        expr_id,
        Callee::AiGenerateValue {
            value: t,
            schema: request.schema,
            wrapped: request.wrapped,
            span: type_args[0].span(),
        },
    );
    let result = tys.intern(Ty::Result(t, ai_error));
    Some(tys.intern(Ty::Effect(result)))
}
