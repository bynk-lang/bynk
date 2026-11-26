//! #4389: the `HttpResult` variants that answer a range or a conditional read:
//! `PartialContent(body, contentType, range)` (206) and the bodyless
//! `NotModified` (304).
//!
//! `PartialContent` takes a `Stream[Bytes]` body, a `String` content-type and a
//! `ContentRange` (the `bynk` record naming the part's first and last byte and the
//! total). Both variants answer a range or a conditional read, which only a `GET`
//! route serves, so they are refused on any other method.

use super::{Ctx, Ty, TyId, kernels};
use crate::checker::{compatible, named_ty, type_of};
use bynk_syntax::ast::{BaseType, Expr, HttpVariant};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

/// The `bynk` `ContentRange` record: the caller's own, or, in a system suite that sees only the
/// context it tests, the embedded one.
fn content_range_ty(ctx: &Ctx) -> Option<TyId> {
    let found = ctx
        .input
        .types
        .get("ContentRange")
        .filter(|d| crate::context_checks::is_std_bynk_type("ContentRange", &ctx.input.types, d))
        .cloned()
        .or_else(|| kernels::std_decl("ContentRange"))?;
    Some(named_ty(&found, ctx.tys))
}

/// `PartialContent` and `NotModified` are answers to a `GET`: a route of another method that
/// names one is refused, at the variant.
pub(super) fn refuse_off_get(span: Span, variant: HttpVariant, ctx: &mut Ctx) {
    if ctx.http_non_get && matches!(variant.name, "PartialContent" | "NotModified") {
        ctx.errors.push(
            CompileError::new(
                "bynk.http.status_needs_get",
                span,
                format!(
                    "`HttpResult.{}` ({}) answers a range or conditional read, which only a `GET` route serves",
                    variant.name, variant.status
                ),
            )
            .with_note(
                "a `HEAD` request is answered by the `GET` route; put this answer in a `GET` handler",
            ),
        );
    }
}

/// `PartialContent(body, contentType, range)`: the argument types and the result type.
pub(super) fn check_partial_content(
    span: Span,
    variant: HttpVariant,
    args: &[Expr],
    expected_t: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if args.len() != 3 {
        ctx.errors.push(CompileError::new(
            "bynk.types.variant_arity",
            span,
            format!(
                "`HttpResult.{}` expects 3 arguments (a `Stream[Bytes]` body, a `String` content-type and a `ContentRange`), but {} were given",
                variant.name,
                args.len(),
            ),
        ));
        return None;
    }
    let bytes = tys.intern(Ty::Base(BaseType::Bytes));
    let stream = tys.intern(Ty::Stream(bytes));
    let string = tys.intern(Ty::Base(BaseType::String));
    let Some(range) = content_range_ty(ctx) else {
        ctx.errors.push(CompileError::new(
            "bynk.types.argument_mismatch",
            span,
            "`ContentRange` is not in scope here: add it to this file's `consumes bynk { … }`",
        ));
        return None;
    };
    for (arg, (want, what)) in args.iter().zip([
        (stream, "a `Stream[Bytes]` body"),
        (string, "a `String` content-type"),
        (range, "a `ContentRange`"),
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

/// `ContentRange` as a bound `PartialContent`'s `range` field is typed: the embedded declaration.
pub(crate) fn content_range_for_binding(tys: &super::Types) -> TyId {
    match kernels::std_decl("ContentRange") {
        Some(decl) => named_ty(&decl, tys),
        None => tys.intern(Ty::Unit),
    }
}
