//! #4177: `parTraverse` on a `Query` (a store or held `Map`'s broadcast form) is
//! `parForEach`. #3967 gave the `List` names one grid, and `parTraverse` there now
//! returns the values; the query form returns `()`, so it takes the `parForEach`
//! name too, and the old spelling is an error that carries the rename.

use super::*;

/// Report `Query.parTraverse` as renamed, with a machine-applicable fix.
pub(super) fn report_query_par_traverse(method: &Ident, ctx: &mut Ctx) {
    ctx.errors.push(
        CompileError::new(
            "bynk.query.par_traverse_renamed",
            method.span,
            "`Query.parTraverse` is now `parForEach`: it runs the steps concurrently and returns `()`",
        )
        .with_note(
            "`parTraverse` names the traversal that returns the steps' values, as on a `List` (#3967); a step that runs only for its effect is `parForEach`",
        )
        .with_suggestion(
            "rename to `parForEach`",
            vec![(method.span, "parForEach".to_string())],
            Applicability::MachineApplicable,
        ),
    );
}
