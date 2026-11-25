//! #3963: `List.filterMap`, `zipWith`, `takeWhile`, `skipWhile` and the predicate
//! form of `count`. Kept apart from `kernels.rs` so that file does not grow.

use super::calls::check_arg;
use super::kernels::check_kernel_fn_arg;
use super::*;
use crate::kernel_op::ListOp;

/// Type one of these methods. `count` reaches here only with its one predicate
/// argument; the bare `count()` is `list_basics`'.
pub(super) fn check_list_more(
    method: &Ident,
    op: ListOp,
    args: &[Expr],
    elem: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let name = method.name.as_str();
    let expect = if op == ListOp::ZipWith { 2 } else { 1 };
    if args.len() != expect {
        // `count` is the only one with two arities: none, or a predicate.
        let takes = if op == ListOp::Count {
            "0 arguments, or 1 (a predicate)".to_string()
        } else {
            format!("{expect} argument{}", if expect == 1 { "" } else { "s" })
        };
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!("`List.{name}` takes {takes}, got {}", args.len()),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let bool_ty = tys.intern(Ty::Base(BaseType::Bool));
    let predicate = |what: &str, ctx: &mut Ctx| {
        let p = Ty::Fn {
            params: vec![elem],
            ret: bool_ty,
        };
        check_arg(&args[0], tys.intern(p), what, ctx);
    };
    match op {
        ListOp::TakeWhile | ListOp::SkipWhile => {
            predicate(&format!("the `List.{name}` predicate"), ctx);
            Some(tys.intern(Ty::List(elem)))
        }
        ListOp::Count => {
            predicate("the `List.count` predicate", ctx);
            Some(tys.intern(Ty::Base(BaseType::Int)))
        }
        ListOp::FilterMap => {
            let ret =
                check_kernel_fn_arg(&args[0], vec![elem], "the `List.filterMap` function", ctx)?;
            let Ty::Option(inner) = &*tys.get(ret) else {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.combinator_return_mismatch",
                        args[0].span,
                        format!(
                            "the `List.filterMap` function must return `Option[U]`, but returns `{}`",
                            ret.display(tys)
                        ),
                    )
                    .with_note("return `Some(value)` to keep an element and `None` to drop it"),
                );
                return None;
            };
            Some(tys.intern(Ty::List(*inner)))
        }
        ListOp::ZipWith => {
            let other = type_of(&args[0], None, ctx)?;
            let Ty::List(other_elem) = &*tys.get(other) else {
                ctx.errors.push(CompileError::new(
                    "bynk.types.type_mismatch",
                    args[0].span,
                    format!(
                        "the `List.zipWith` list must be a `List`, but this is `{}`",
                        other.display(tys)
                    ),
                ));
                return None;
            };
            let other_elem = *other_elem;
            let v = check_kernel_fn_arg(
                &args[1],
                vec![elem, other_elem],
                "the `List.zipWith` function",
                ctx,
            )?;
            Some(tys.intern(Ty::List(v)))
        }
        _ => unreachable!("only these arms are routed here"),
    }
}
