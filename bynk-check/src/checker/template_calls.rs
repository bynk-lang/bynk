//! Typed HTML (#3927, slice 1): a template's name in an expression.
//!
//! A template with no fields (`template footer = "embeds/footer.html"`) is a
//! value of type `Html`. A template with fields is used as `shell.fill(fields)`,
//! which takes the whole fields record and is `Html`. The template's markup
//! and placeholders are checked once, against its file
//! (`crate::templates::phase_templates`); here only the call is.

use super::*;

/// `bynk.html`'s `Html`, when the unit has it in scope.
pub(crate) fn html_ty(ctx: &Ctx) -> Option<TyId> {
    ctx.input
        .types
        .values()
        .find(|d| crate::templates::is_html_decl(d))
        .map(|d| crate::checker::named_ty(d, ctx.tys))
}

/// The type of a template's name used as a value, or `None` when `id` names no
/// template (or a local shadows it).
pub(crate) fn check_template_ident(id: &Ident, ctx: &mut Ctx) -> Option<Option<TyId>> {
    let decl = crate::templates::template_named(id, &ctx.input.embeds)?.clone();
    ctx.refs.record(id.span, SymbolKind::Embed, &id.name);
    if decl.template.as_ref().is_some_and(|t| t.fields.is_some()) {
        ctx.errors.push(
            CompileError::new(
                "bynk.template.fill_required",
                id.span,
                format!("template `{}` has fields, so it is used as `{}.fill(fields)`", id.name, id.name),
            )
            .with_note("`fill` takes the whole fields record and gives the page as `Html`; a template with no fields is `Html` itself"),
        );
        return Some(None);
    }
    Some(html_ty(ctx))
}

/// Type `shell.fill(fields)` (or any member of a template), or `None` when the
/// receiver is not a template's name.
pub(crate) fn check_fill_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let ExprKind::Ident(id) = &receiver.kind else {
        return None;
    };
    if ctx.lookup(id.name.as_str()).is_some() {
        return None;
    }
    let decl = crate::templates::template_named(id, &ctx.input.embeds)?.clone();
    let fields = decl.template.as_ref().and_then(|t| t.fields.clone());
    // A template with no fields is an `Html` value: its methods are `Html`'s.
    let fields = fields?;
    ctx.refs.record(id.span, SymbolKind::Embed, &id.name);
    if method.name != "fill" {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_not_found",
            method.span,
            format!(
                "template `{}` has no member `{}`: it has `fill(fields)`",
                id.name, method.name
            ),
        ));
        return Some(give_up(args, ctx));
    }
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`{}.fill` takes 1 argument (the fields record), got {}",
                id.name,
                args.len()
            ),
        ));
        return Some(give_up(args, ctx));
    }
    ctx.callees.insert(
        expr_id,
        Callee::TemplateFill {
            template: id.name.clone(),
        },
    );
    if let Some(want) = crate::checker::resolve_type_ref(&fields, &ctx.input.types, ctx.tys) {
        super::calls::check_arg(&args[0], want, &format!("`{}.fill`'s fields", id.name), ctx);
    } else {
        let _ = type_of(&args[0], None, ctx);
    }
    Some(html_ty(ctx))
}

fn give_up(args: &[Expr], ctx: &mut Ctx) -> Option<TyId> {
    for a in args {
        let _ = type_of(a, None, ctx);
    }
    None
}

/// `bynk.html.no_constructor`: `Html.of(s)` outside `bynk.html`. `Html` is opaque, and an opaque
/// type's `of` is open to every unit, but `Html` has no predicate, so its `of` would accept any
/// string: a raw door to unescaped markup that nothing lists or audits. Markup comes from a
/// template (and, with the literals, from `html"…"` and the one audited `Html.unsafe`). `None`
/// when the call is not `Html.of` on `bynk.html`'s `Html`, or is inside `bynk.html` itself.
pub(crate) fn refuse_html_of(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let ExprKind::Ident(id) = &receiver.kind else {
        return None;
    };
    if method.name != "of"
        || ctx.lookup(id.name.as_str()).is_some()
        || !crate::templates::is_html_type(&id.name, &ctx.input.types)
        || ctx.input.is_local_type(&id.name)
    {
        return None;
    }
    ctx.errors.push(
        CompileError::new(
            crate::templates::NO_CONSTRUCTOR,
            method.span,
            format!("`{}` has no `of` constructor: no string becomes markup unchecked", id.name),
        )
        .with_note("markup comes from a `template`'s `fill`, or a template with no fields; pass text as a `String` field and the template escapes it for where it sits"),
    );
    Some(give_up(args, ctx))
}
