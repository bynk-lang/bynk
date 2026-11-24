//! #3968: the bounded concurrent forms, `parForEachWith`, `parTraverseWith`,
//! `parTraverseAllWith` and `parTraverseTryWith`. Each is its plain form with
//! `__bynkSettleAll` replaced by the runtime's sliding pool,
//! `__bynkSettlePool(thunks, options.limit)`. Kept apart so `lower_list.rs` does
//! not grow.

use bynk_check::kernel_op::ListOp;
use bynk_ts::{TsExpr, TsType};

use super::{
    ListArm, bail_on_err, iife_block, list_ok_elem, push_value, settle_thunks, take, xs_param,
};
use crate::emitter::lower::Pre;
use crate::emitter::lower_ctx::LowerCtx;
use crate::ts_build::{
    arrow_with, block, bool_lit, call, const_typed, expr_stmt, for_of, ident, member, paren, ret,
};

/// `await __bynkSettlePool(thunks, limit[, true])`.
fn await_pool(thunks: TsExpr, limit: TsExpr, stop_on_err: bool) -> TsExpr {
    let mut args = vec![thunks, limit];
    if stop_on_err {
        args.push(bool_lit(true));
    }
    TsExpr::Await(Box::new(call(ident("__bynkSettlePool"), args)))
}

/// Lower one `…With` call over the lowered receiver, step and options.
pub(super) fn lower_pool(
    a: &ListArm,
    op: ListOp,
    f: &bynk_syntax::ast::Expr,
    options: &bynk_syntax::ast::Expr,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> TsExpr {
    let tys = cx.ty_render();
    // The `Try` form's output array element, read before the operands lower.
    let ok_elem = (op == ListOp::ParTraverseTryWith).then(|| {
        list_ok_elem(
            cx.commons().expr_types.get(&a.e.id).map(|te| te.ty),
            tys,
            cx.fault_site(a.e.span),
        )
    });
    let recv = pre.lower_node(a.receiver, cx);
    let f = pre.lower_node(f, cx);
    let options = pre.lower_node(options, cx);
    let n = vec![recv, f, options];
    let [recv, f, options] = take(n);
    // `(options).limit`: a literal, a constant or a computed record all read alike.
    let limit = member(paren(options), "limit");
    let thunks = settle_thunks(a, f);
    if op == ListOp::ParForEachWith {
        iife_block(
            vec![xs_param(&a.elem)],
            true,
            vec![expr_stmt(await_pool(thunks, limit, false))],
            vec![recv],
        )
    } else if matches!(op, ListOp::ParTraverseWith | ListOp::ParTraverseAllWith) {
        call(
            arrow_with(
                vec![xs_param(&a.elem)],
                true,
                await_pool(thunks, limit, false),
            ),
            vec![recv],
        )
    } else {
        // `ParTraverseTryWith`: the only other `…With` form routed here.
        {
            let u = ok_elem.expect("read above for the Try form");
            // A step the early stop never started leaves no result, so the scan
            // reaches the first `Err` in input order before it could reach one.
            let scan = vec![bail_on_err(), push_value()];
            let body = vec![
                const_typed("__rs", None, await_pool(thunks, limit, true)),
                const_typed("__out", Some(TsType::array(u)), TsExpr::array(vec![])),
                for_of("__r", ident("__rs"), block(scan)),
                ret(call(ident("Ok"), vec![ident("__out")])),
            ];
            iife_block(vec![xs_param(&a.elem)], true, body, vec![recv])
        }
    }
}
