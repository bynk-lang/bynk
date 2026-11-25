//! #4177: `parTraverse` on a `Query` (a store or held `Map`'s broadcast form) is
//! `parForEach`. #3967 gave the `List` names one grid, and `parTraverse` there now
//! returns the values; the query form returns `()`, so it takes the `parForEach`
//! name too, and the old spelling is an error that carries the rename.

use super::calls::check_arg;
use super::*;
use crate::kernel_op::QueryOp;

/// v0.107 (slice 4): `forEach` runs the effectful function over each element **in
/// turn**; `parForEach` runs them **concurrently** (the broadcast form: a slow or
/// half-dead connection must not head-of-line-block the rest). Same type; the
/// difference is sequential-vs-parallel lowering. The call is checked the same
/// whatever the spelling, so one wrong name raises one error.
pub(super) fn check_step(method: &Ident, args: &[Expr], elem: TyId, ctx: &mut Ctx) -> TyId {
    let tys = ctx.tys;
    let unit = tys.intern(Ty::Unit);
    let step = Ty::Fn {
        params: vec![elem],
        ret: tys.intern(Ty::Effect(unit)),
    };
    check_arg(
        &args[0],
        tys.intern(step),
        &format!("the `Query.{}` function", method.name),
        ctx,
    );
    tys.intern(Ty::Effect(unit))
}

/// Report the retired `parTraverse` spelling, if that is what `op` is.
pub(super) fn report_old_name(op: QueryOp, method: &Ident, ctx: &mut Ctx) {
    if op == QueryOp::ParTraverse {
        report_query_par_traverse(method, ctx);
    }
}

/// Report `Query.parTraverse` as renamed, with a machine-applicable fix.
fn report_query_par_traverse(method: &Ident, ctx: &mut Ctx) {
    ctx.errors.push(
        CompileError::new(
            "bynk.query.par_traverse_renamed",
            method.span,
            "`Query.parTraverse` is now `parForEach`: it runs the steps concurrently and returns `()`",
        )
        .with_note(
            "`parTraverse` names the traversal that returns the steps' values, as on a `List` (#3967); a step that runs only for its effect is `parForEach`. To get the values back, `collect()` the query, then `parTraverse` the list",
        )
        .with_suggestion(
            "rename to `parForEach`",
            vec![(method.span, "parForEach".to_string())],
            Applicability::MachineApplicable,
        ),
    );
}
