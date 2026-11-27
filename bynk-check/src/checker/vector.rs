//! #4101: the checker's half of `Vectorize.queryValue[M]` and `getByIdsValue[M]`.
//!
//! Both are lowered over the text operations (`query`, `getByIds`): the call site reads each
//! match's metadata text as `M` on its own, so a vector stored by an older shape of `M` is an
//! `Err` for that one match and the rest still read. They are not declared operations of
//! `Vectorize`, as `Analytics.writeValue` is not one of `Analytics`: there is no per-item policy
//! a declared typed read could say.

use super::*;

/// Whether `method` is one of the two typed reads of a derived first-party `Vectorize` instance.
pub(crate) fn is_vector_read(derived_from: Option<&(String, String)>, method: &str) -> bool {
    matches!(method, "queryValue" | "getByIdsValue")
        && matches!(derived_from, Some((unit, base)) if unit == crate::firstparty::CLOUDFLARE_UNIT && base == "Vectorize")
}

fn named(ctx: &Ctx, name: &str, kind: NamedKind, args: Vec<TyId>) -> Option<TyId> {
    ctx.input.types.contains_key(name).then(|| {
        ctx.tys.intern(Ty::Named {
            name: name.to_string(),
            kind,
            args,
        })
    })
}

/// The typed read of a capability call, or `None` when the call is not one. The inner `Option` is
/// the call's type, `None` when it is refused.
pub(crate) fn check_typed(
    cap: &str,
    derived_from: Option<&(String, String)>,
    method: &Ident,
    (type_args, args): (&[TypeRef], &[Expr]),
    (span, expr_id): (Span, ExprId),
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    is_vector_read(derived_from, &method.name)
        .then(|| check_read(cap, method, type_args, args, span, expr_id, ctx))
}

fn check_read(
    cap: &str,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    super::calls::refuse_experimental(
        &format!("bynk.cloudflare.Vectorize.{}", method.name),
        method.span,
        ctx,
    );
    let name = format!("{cap}.{}", method.name);
    let by_ids = method.name == "getByIdsValue";
    let float_list = tys.intern(Ty::List(tys.intern(Ty::Base(BaseType::Float))));
    let string_list = tys.intern(Ty::List(tys.intern(Ty::Base(BaseType::String))));
    let options = named(ctx, "QueryOptions", NamedKind::Record, Vec::new());
    let expected: Vec<Option<TyId>> = if by_ids {
        vec![Some(string_list)]
    } else {
        vec![Some(float_list), options]
    };
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
                "`{name}` takes exactly one type argument (the metadata type), got {}",
                type_args.len()
            ),
        ));
        return bail(ctx);
    }
    if args.len() != expected.len() {
        ctx.errors.push(CompileError::new(
            "bynk.capability.op_arity",
            span,
            format!(
                "capability operation `{name}` expects {} argument(s), but {} were given",
                expected.len(),
                args.len()
            ),
        ));
        return bail(ctx);
    }
    for (i, (arg, want)) in args.iter().zip(&expected).enumerate() {
        let Some(want) = *want else {
            let _ = type_of(arg, None, ctx);
            continue;
        };
        if let Some(actual) = type_of(arg, Some(want), ctx)
            && !compatible(actual, want, tys)
        {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                arg.span,
                format!(
                    "argument {} to capability `{name}` has type `{}`, but parameter expects `{}`",
                    i + 1,
                    actual.display(tys),
                    want.display(tys)
                ),
            ));
        }
    }
    let meta = resolve_expr_type_ref(&type_args[0], ctx)?;
    if !codable::check_codable(
        ctx,
        meta,
        type_args[0].span(),
        &codable::CodableSite::Op {
            op: &name,
            param: "M",
        },
    ) {
        return None;
    }
    let item = if by_ids {
        "VectorReadOf"
    } else {
        "VectorMatchOf"
    };
    let item = named(ctx, item, NamedKind::Record, vec![meta])?;
    let vector_error = named(ctx, "VectorError", NamedKind::Sum, Vec::new())?;
    ctx.callees.insert(
        expr_id,
        Callee::VectorValueRead {
            cap: cap.to_string(),
            meta,
            by_ids,
        },
    );
    let result = tys.intern(Ty::Result(tys.intern(Ty::List(item)), vector_error));
    Some(tys.intern(Ty::Effect(result)))
}
