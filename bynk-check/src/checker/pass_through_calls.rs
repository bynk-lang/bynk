//! #3752 (pass-through HTTP, slice 3): the reads on the `bynk` unit's
//! `InboundRequest` (`method`, `headers`, `query`) and `Forwarded` (`status`,
//! `header`). Each type is found by its declaration, as `Secret` is.

use super::calls::{check_arg, record_kernel};
use super::{Ctx, Ty, TyId, type_of};
use crate::checker::ExprId;
use crate::http_pass_through::{PassThroughType, pass_through_type};
use crate::kernel_op::{ForwardedOp, InboundRequestOp, KernelOp};
use bynk_syntax::ast::{BaseType, Expr, Ident};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

/// A method call on an `InboundRequest` or a `Forwarded`, or `None` when the
/// receiver is neither.
pub(super) fn check_pass_through_call(
    recv_ty: TyId,
    method: &Ident,
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let Ty::Named { name, .. } = &*ctx.tys.get(recv_ty) else {
        return None;
    };
    let kind = pass_through_type(name, &ctx.input.types)?;
    let tys = ctx.tys;
    let string = tys.intern(Ty::Base(BaseType::String));
    let (op, params, ret, has) = match kind {
        PassThroughType::InboundRequest => {
            let op = InboundRequestOp::from_name(&method.name);
            let ret = match op {
                Some(InboundRequestOp::Headers) => Some(tys.intern(Ty::RequestHeaders)),
                Some(InboundRequestOp::Method | InboundRequestOp::Query) => Some(string),
                None => None,
            };
            (
                op.map(KernelOp::InboundRequest),
                vec![],
                ret,
                "`method()`, `headers()` and `query()`",
            )
        }
        PassThroughType::Forwarded => {
            let op = ForwardedOp::from_name(&method.name);
            let (params, ret) = match op {
                Some(ForwardedOp::Status) => (vec![], Some(tys.intern(Ty::Base(BaseType::Int)))),
                Some(ForwardedOp::Header) => (vec![string], Some(tys.intern(Ty::Option(string)))),
                None => (vec![], None),
            };
            (
                op.map(KernelOp::Forwarded),
                params,
                ret,
                "`status()` and `header(name)`",
            )
        }
        PassThroughType::PassThrough | PassThroughType::UpstreamError => return None,
    };
    let Some(ret) = ret else {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_not_found",
            method.span,
            format!("`{name}` has no method `{}` — it has {has}", method.name),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return Some(None);
    };
    if args.len() != params.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`{name}.{}` takes {} argument{}, got {}",
                method.name,
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return Some(None);
    }
    for (a, p) in args.iter().zip(&params) {
        check_arg(
            a,
            *p,
            &format!("the `{name}.{}` argument", method.name),
            ctx,
        );
    }
    Some(record_kernel(ctx, expr_id, recv_ty, op, method, Some(ret)))
}

/// #3752 (D20): `bynk.test.upstream_exchange_outside_suite` for a call of an
/// `Upstream` instance's `exchange`, the network step beneath `forward`'s policy,
/// anywhere but a suite. A program forwards with `forward`, so its policy (the path
/// checks, the origin check) always runs.
/// A call of an operation of capability `cap`: its reference recorded, and
/// [`refuse_exchange_outside_suite`] for an `Upstream` instance's `exchange`.
pub(super) fn record_capability_call(
    cap: &super::CapabilityInfo,
    type_name: &Ident,
    method: &Ident,
    ctx: &mut Ctx,
) {
    super::calls::record_capability_ref(type_name.span, &type_name.name, ctx);
    refuse_exchange_outside_suite(cap, method, ctx);
}

/// `Type.method(…)` on a declared type: `Forwarded.of(…)`, a suite's upstream answer
/// ([`check_forwarded_of`]), or else an ordinary static call.
#[allow(clippy::too_many_arguments)]
pub(super) fn static_or_seam(
    type_name: &Ident,
    method: &Ident,
    type_args: &[bynk_syntax::ast::TypeRef],
    args: &[Expr],
    span: Span,
    expected: Option<TyId>,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    if pass_through_type(&type_name.name, &ctx.input.types) == Some(PassThroughType::Forwarded) {
        return check_forwarded_of(type_name, method, args, span, expr_id, ctx);
    }
    super::calls::check_static_call(
        type_name, method, type_args, args, span, expected, expr_id, ctx,
    )
}

fn refuse_exchange_outside_suite(cap: &super::CapabilityInfo, method: &Ident, ctx: &mut Ctx) {
    let upstream = cap
        .derived_from
        .as_ref()
        .is_some_and(|(unit, base)| unit == bynk_std::BYNK_UNIT && base == "Upstream");
    if method.name != "exchange" || !upstream || ctx.in_test_body || ctx.test_values {
        return;
    }
    ctx.errors.push(
        CompileError::new(
            "bynk.test.upstream_exchange_outside_suite",
            method.span,
            format!(
                "`{}.exchange` is the test seam beneath `forward`, which only a suite reaches",
                cap.name
            ),
        )
        .with_note(format!(
            "forward the request with `{}.forward(request, path)`, which checks the path and the origin before anything is sent",
            cap.name
        )),
    );
}

/// #3752 (D20): `Forwarded.of(status, headerLines, body)`, a suite's stand-in for an
/// upstream's answer: what a stub of `exchange` returns. Only a suite builds one
/// (`bynk.test.forwarded_outside_suite`), so a program can never fabricate an
/// upstream response.
fn check_forwarded_of(
    type_name: &Ident,
    method: &Ident,
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    use crate::kernel_op::{ForwardedStatic, StaticOp};
    if !ctx.in_test_body && !ctx.test_values {
        ctx.errors.push(
            CompileError::new(
                "bynk.test.forwarded_outside_suite",
                span,
                "`Forwarded.of` builds an upstream's answer for a suite; a program gets one only from `Upstream.forward`",
            )
            .with_note("a `Forwarded` holds what the declared origin sent, so a handler can't fabricate one"),
        );
    }
    let Some(op) = ForwardedStatic::from_name(&method.name) else {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_not_found",
            method.span,
            format!(
                "`Forwarded` has no static `{}` — it has `of(status, headerLines, body)`",
                method.name
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    ctx.callees
        .insert(expr_id, super::Callee::intrinsic(StaticOp::Forwarded(op)));
    let tys = ctx.tys;
    let int = tys.intern(Ty::Base(BaseType::Int));
    let lines = tys.intern(Ty::List(tys.intern(Ty::Base(BaseType::String))));
    let body = tys.intern(Ty::Stream(tys.intern(Ty::Base(BaseType::Bytes))));
    if args.len() != 3 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`Forwarded.of` takes 3 arguments (status, headerLines, body), got {}",
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    for (a, (p, what)) in args
        .iter()
        .zip([(int, "status"), (lines, "headerLines"), (body, "body")])
    {
        check_arg(a, p, what, ctx);
    }
    crate::checker::resolve_type_ref(
        &bynk_syntax::ast::TypeRef::Named(type_name.clone()),
        &ctx.input.types,
        tys,
    )
}
