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
