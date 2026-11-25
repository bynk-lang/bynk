//! #3969: the pure error-recovery half of `Result`, matching what an
//! `Effect[Result]` already had (`flatMapErr`) and `Result.ok()`.
//!
//! - `orElse(f: E -> Result[T, F]) -> Result[T, F]`: `r` if it is `Ok`, else
//!   `f(e)`, run only on `Err`. `f` takes the error, so it can inspect what
//!   failed and change the error type, as `flatMapErr` does.
//! - `err() -> Option[E]`: the `Err` value, dropping the success.
//!
//! Kept apart from `kernels.rs` so that file does not grow.

use super::kernels::check_kernel_fn_arg;
use super::*;
use crate::kernel_op::ResultOp;

pub(super) fn check_result_more(
    op: ResultOp,
    method: &Ident,
    args: &[Expr],
    ok: TyId,
    err: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let wanted = if op == ResultOp::OrElse { 1 } else { 0 };
    if args.len() != wanted {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`Result.{}` takes {wanted} argument{}, got {}",
                method.name,
                if wanted == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    if op == ResultOp::Err {
        return Some(tys.intern(Ty::Option(err)));
    }
    let ret = check_kernel_fn_arg(&args[0], vec![err], "the `Result.orElse` function", ctx)?;
    match &*tys.get(ret) {
        Ty::Result(t2, f) => {
            let (t2, f) = (*t2, *f);
            // One way only, as `Option.orElse`: the fallback's success value must be usable
            // where the receiver's is expected. The call is typed `Result[ok, F]`, so a
            // wider success type (an `Int` for a refined `Int`) would skip the refinement.
            if !compatible(t2, ok, tys) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.combinator_return_mismatch",
                    args[0].span,
                    format!(
                        "the `Result.orElse` function's success type `{}` does not match the receiver's `{}`",
                        t2.display(tys),
                        ok.display(tys)
                    ),
                ));
                return None;
            }
            Some(tys.intern(Ty::Result(ok, f)))
        }
        _ => {
            ctx.errors.push(
                CompileError::new(
                    "bynk.types.combinator_return_mismatch",
                    args[0].span,
                    format!(
                        "the `Result.orElse` function must return a `Result`, but returns `{}`",
                        ret.display(tys)
                    ),
                )
                .with_note("recover with `Ok(value)` or fail with a different `Err(...)`"),
            );
            None
        }
    }
}
