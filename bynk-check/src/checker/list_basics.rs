//! #3962: the `List` methods `bynk.list` used to be the only home of
//! (`reverse`, `find`, `traverse`, keyless `sort`/`min`/`max`) and the three
//! everyday ones that existed nowhere (`contains`, `last`/`lastOrElse`,
//! `isEmpty`). Kept apart from `kernels.rs` so that file does not grow.

use super::calls::check_arg;
use super::kernels::check_kernel_fn_arg;
use super::ordering::{OrdSite, check_ord};
use super::*;
use crate::kernel_op::ListOp;

/// Type one of these methods (`first`, `firstOrElse` and `count` moved in with
/// them, to keep `check_list_kernel_method` under its length ceiling).
pub(super) fn check_list_basic(
    method: &Ident,
    op: ListOp,
    args: &[Expr],
    elem: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let name = method.name.as_str();
    let expect = match op {
        ListOp::Reverse | ListOp::Last | ListOp::IsEmpty | ListOp::Sort => 0,
        ListOp::Min | ListOp::Max | ListOp::First | ListOp::Count => 0,
        ListOp::Find
        | ListOp::Contains
        | ListOp::LastOrElse
        | ListOp::FirstOrElse
        | ListOp::Traverse => 1,
        _ => unreachable!("`handles` admits only these"),
    };
    if args.len() != expect {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`List.{name}` takes {expect} argument{}, got {}",
                if expect == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let list = tys.intern(Ty::List(elem));
    let bool_ty = tys.intern(Ty::Base(BaseType::Bool));
    match op {
        ListOp::Reverse => Some(list),
        ListOp::IsEmpty => Some(bool_ty),
        ListOp::Last => Some(tys.intern(Ty::Option(elem))),
        ListOp::First => Some(tys.intern(Ty::Option(elem))),
        ListOp::Count => Some(tys.intern(Ty::Base(BaseType::Int))),
        ListOp::FirstOrElse => {
            check_arg(&args[0], elem, "the `List.firstOrElse` fallback", ctx);
            Some(elem)
        }
        ListOp::LastOrElse => {
            check_arg(&args[0], elem, "the `List.lastOrElse` fallback", ctx);
            Some(elem)
        }
        ListOp::Find => {
            let p = Ty::Fn {
                params: vec![elem],
                ret: bool_ty,
            };
            check_arg(&args[0], tys.intern(p), "the `List.find` predicate", ctx);
            Some(tys.intern(Ty::Option(elem)))
        }
        ListOp::Contains => {
            check_arg(&args[0], elem, "the `List.contains` item", ctx);
            require_comparable(elem, span, ctx);
            Some(bool_ty)
        }
        ListOp::Sort => {
            check_ord(ctx, elem, span, &OrdSite::Element("List.sort"));
            Some(list)
        }
        ListOp::Min | ListOp::Max => {
            check_ord(ctx, elem, span, &OrdSite::Element(&format!("List.{name}")));
            Some(tys.intern(Ty::Option(elem)))
        }
        ListOp::Traverse => {
            let ret =
                check_kernel_fn_arg(&args[0], vec![elem], "the `List.traverse` function", ctx)?;
            let Ty::Effect(inner) = &*tys.get(ret) else {
                ctx.errors.push(CompileError::new(
                    "bynk.types.combinator_return_mismatch",
                    args[0].span,
                    format!(
                        "the `List.traverse` function must return `Effect[U]`, but returns `{}`",
                        ret.display(tys)
                    ),
                ));
                return None;
            };
            let inner = *inner;
            if !ctx.effectful {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.effect.fn_value_in_pure_context",
                        span,
                        "`List.traverse` runs an effectful function and cannot be called in a pure context",
                    )
                    .with_note(
                        "effectful function values may only be called where the enclosing body is effectful (its return type is an Effect)",
                    ),
                );
            }
            Some(tys.intern(Ty::Effect(tys.intern(Ty::List(inner)))))
        }
        _ => unreachable!("`handles` admits only these"),
    }
}

/// `contains` compares by `==`, so its element carries the same requirement:
/// an equality-supporting type, and `Eq` on a type parameter.
fn require_comparable(ty: TyId, span: Span, ctx: &mut Ctx) {
    use super::equality::{explain, not_comparable, record_compared, require_eq};
    let tys = ctx.tys;
    record_compared(ty, tys);
    if !require_eq(ctx, ty, span, "`List.contains` compares by `==`") {
        return;
    }
    if let Some(blocker) = not_comparable(ty, ctx) {
        let top = matches!(
            &*tys.get(ty),
            Ty::Stream(_)
                | Ty::Connection(_)
                | Ty::TestConnection(_)
                | Ty::Fn { .. }
                | Ty::Effect(_)
                | Ty::Query(_)
        );
        let (code, why) = explain(blocker, top);
        ctx.errors.push(CompileError::new(
            code,
            span,
            format!(
                "`List.contains` needs an element type with value equality, but `{}` has none — {why}",
                ty.display(tys)
            ),
        ));
    }
}
