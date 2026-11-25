//! #3965: `map` and `andThen` on every `Effect[T]`, as the spec already described.
//!
//! - `e.map(f: T -> U) -> Effect[U]`: `f` applied to the effect's value.
//! - `e.andThen(f: T -> Effect[U]) -> Effect[U]`: a further effect, run after `e`.
//!
//! They build an effect rather than run one, so they are legal in a pure helper, like the
//! `Effect[Result]` combinators (ADR 0176). On an `Effect[Result[T, E]]` they act on the whole
//! `Result`; `mapOk`/`flatMapOk` keep acting on the success side.

use super::kernels::check_kernel_fn_arg;
use super::*;

/// Type `e.map(f)` or `e.andThen(f)` on an `Effect[inner]`.
pub(super) fn check_map_and_then(
    method: &Ident,
    args: &[Expr],
    inner: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`Effect.{}` takes 1 argument, got {}",
                method.name,
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let what = format!("the `Effect.{}` function", method.name);
    let ret = check_kernel_fn_arg(&args[0], vec![inner], &what, ctx)?;
    let returned_effect = match &*tys.get(ret) {
        Ty::Effect(u) => Some(*u),
        _ => None,
    };
    if method.name == "andThen" {
        let Some(u) = returned_effect else {
            ctx.errors.push(
                CompileError::new(
                    "bynk.types.combinator_return_mismatch",
                    args[0].span,
                    format!(
                        "the `Effect.andThen` function must return an `Effect`, but returns `{}`",
                        ret.display(tys)
                    ),
                )
                .with_note("to transform the value without a further effect, use `map`"),
            );
            return None;
        };
        return Some(tys.intern(Ty::Effect(u)));
    }
    if returned_effect.is_some() {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.combinator_return_mismatch",
                args[0].span,
                format!(
                    "the `Effect.map` function returns `{}`, which would nest one effect in another",
                    ret.display(tys)
                ),
            )
            .with_note("to follow with a further effect, use `andThen`"),
        );
        return None;
    }
    Some(tys.intern(Ty::Effect(ret)))
}

/// A method an `Effect` does not have: `bynk.types.method_not_found`, naming the ones it does.
pub(super) fn report_missing_method(method: &Ident, args: &[Expr], ctx: &mut Ctx) {
    ctx.errors.push(CompileError::new(
        "bynk.types.method_not_found",
        method.span,
        format!(
            "the built-in `Effect` type has no method `{}` — the kernel is `map`, `andThen`, \
             `within`, `race`, `detach`",
            method.name
        ),
    ));
    for a in args {
        let _ = type_of(a, None, ctx);
    }
}
