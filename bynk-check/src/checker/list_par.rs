//! The `List` effect iterators that run a step per element and either discard
//! the outcome (`forEach`, `parForEach`) or keep the values (`parTraverse`).
//! #3967 gave them one grid of names; kept apart from `kernels.rs` so that file
//! does not grow.

use super::calls::check_arg;
use super::kernels::check_kernel_fn_arg;
use super::*;
use crate::kernel_op::ListOp;

/// `forEach(f: T -> Effect[()]) -> Effect[()]` runs `f` for each element in
/// order (v0.146, ADR 0170); `parForEach` runs them concurrently (v0.147, ADR
/// 0171); `parTraverse(f: T -> Effect[U]) -> Effect[List[U]]` runs them
/// concurrently and keeps the values in input order (#3967). Each runs an
/// effectful function value, so each is confined to effectful contexts.
pub(super) fn check_effect_iter(
    method: &Ident,
    op: ListOp,
    args: &[Expr],
    elem: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`List.{}` takes 1 argument, got {}",
                method.name,
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let what = format!("the `List.{}` function", method.name);
    let unit = tys.intern(Ty::Unit);
    let result = if op == ListOp::ParTraverse {
        let ret = check_kernel_fn_arg(&args[0], vec![elem], &what, ctx)?;
        let Ty::Effect(inner) = &*tys.get(ret) else {
            ctx.errors.push(CompileError::new(
                "bynk.types.combinator_return_mismatch",
                args[0].span,
                format!(
                    "the `List.parTraverse` function must return `Effect[U]`, but returns `{}`",
                    ret.display(tys)
                ),
            ));
            return None;
        };
        let inner = *inner;
        if inner == unit {
            warn_unit_par_traverse(method, ctx);
        }
        tys.intern(Ty::List(inner))
    } else {
        let f = Ty::Fn {
            params: vec![elem],
            ret: tys.intern(Ty::Effect(unit)),
        };
        check_arg(&args[0], tys.intern(f), &what, ctx);
        unit
    };
    if !ctx.effectful {
        ctx.errors.push(
            CompileError::new(
                "bynk.effect.fn_value_in_pure_context",
                span,
                format!(
                    "`List.{}` runs an effectful function and cannot be called in a pure context",
                    method.name
                ),
            )
            .with_note(
                "effectful function values may only be called where the enclosing body is effectful (its return type is an Effect)",
            ),
        );
    }
    Some(tys.intern(Ty::Effect(result)))
}

/// #3967: until now `parTraverse` meant "run concurrently, return `()`". A step
/// that returns `Effect[()]` still type-checks under the new signature (the call
/// is `Effect[List[()]]`), so the old reading is flagged here, with the fix.
fn warn_unit_par_traverse(method: &Ident, ctx: &mut Ctx) {
    // The `…With` form (#3968) renames to its own `…With` sibling, keeping the options.
    let (from, to) = if method.name == "parTraverseWith" {
        ("parTraverseWith", "parForEachWith")
    } else {
        ("parTraverse", "parForEach")
    };
    ctx.errors.push(
        CompileError::new(
            "bynk.list.par_traverse_unit",
            method.span,
            format!(
                "`List.{from}` now returns the steps' values; a step that returns `()` is `{to}`"
            ),
        )
        .with_note(format!(
            "`{to}` runs the steps concurrently and returns `()`, as `{from}` did before the traversal names lined up"
        ))
        .with_suggestion(
            format!("rename to `{to}`"),
            vec![(method.span, to.to_string())],
            Applicability::MachineApplicable,
        ),
    );
}
