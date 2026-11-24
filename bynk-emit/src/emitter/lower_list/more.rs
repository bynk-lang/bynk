//! #3963: lowerings for `filterMap`, `zipWith`, `takeWhile`, `skipWhile` and the
//! predicate form of `count`. Kept apart so `lower_list.rs` does not grow.

use bynk_check::kernel_op::ListOp;
use bynk_ts::{TsBinaryOp, TsExpr, TsParam};

use super::{ListArm, iife, iife_block, xs_param};
use crate::emitter::lower::Pre;
use crate::emitter::lower_ctx::LowerCtx;
use crate::ts_build::{
    arrow, binary, block_arrow, call, cond_expr, const_typed, ident, member, method_call, not_expr,
    num_lit, ret, str_lit,
};

/// `__xs.findIndex((__x) => !__p(__x))`, the index where the prefix ends, or
/// `-1` when `p` holds for every element.
fn prefix_end() -> TsExpr {
    method_call(
        ident("__xs"),
        "findIndex",
        vec![arrow(
            vec![TsParam::new("__x", None)],
            not_expr(call(ident("__p"), vec![ident("__x")])),
        )],
    )
}

/// `__i < 0 ? __xs.length : __i`: where the prefix ends, `length` when it does not.
fn end_or_length() -> TsExpr {
    cond_expr(
        binary(TsBinaryOp::LessThan, ident("__i"), num_lit("0")),
        member(ident("__xs"), "length"),
        ident("__i"),
    )
}

pub(super) fn lower_list_more(
    a: &ListArm,
    op: ListOp,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> TsExpr {
    let recv = pre.lower_node(a.receiver, cx);
    match (op, a.args) {
        (ListOp::TakeWhile | ListOp::SkipWhile, [p]) => {
            let p = pre.lower_node(p, cx);
            let slice = if op == ListOp::TakeWhile {
                method_call(ident("__xs"), "slice", vec![num_lit("0"), end_or_length()])
            } else {
                method_call(ident("__xs"), "slice", vec![end_or_length()])
            };
            iife_block(
                vec![xs_param(&a.elem), TsParam::new("__p", None)],
                false,
                vec![const_typed("__i", None, prefix_end()), ret(slice)],
                vec![recv, p],
            )
        }
        (ListOp::Count, [p]) => {
            // Counts without building the filtered list.
            let p = pre.lower_node(p, cx);
            let step = arrow(
                vec![TsParam::new("__n", None), TsParam::new("__x", None)],
                cond_expr(
                    call(ident("__p"), vec![ident("__x")]),
                    binary(TsBinaryOp::Add, ident("__n"), num_lit("1")),
                    ident("__n"),
                ),
            );
            let body = method_call(ident("__xs"), "reduce", vec![step, num_lit("0")]);
            iife(
                vec![xs_param(&a.elem), TsParam::new("__p", None)],
                body,
                vec![recv, p],
            )
        }
        (ListOp::FilterMap, [f]) => {
            let f = pre.lower_node(f, cx);
            let keep = block_arrow(
                vec![TsParam::new("__x", None)],
                false,
                vec![
                    const_typed("__o", None, call(ident("__f"), vec![ident("__x")])),
                    ret(cond_expr(
                        binary(
                            TsBinaryOp::StrictEq,
                            member(ident("__o"), "tag"),
                            str_lit("Some"),
                        ),
                        TsExpr::array(vec![member(ident("__o"), "value")]),
                        TsExpr::array(vec![]),
                    )),
                ],
            );
            let body = method_call(ident("__xs"), "flatMap", vec![keep]);
            iife(
                vec![xs_param(&a.elem), TsParam::new("__f", None)],
                body,
                vec![recv, f],
            )
        }
        (ListOp::ZipWith, [other, f]) => {
            let other = pre.lower_node(other, cx);
            let f = pre.lower_node(f, cx);
            // `slice(0, ys.length)` stops at the shorter list.
            let pair = arrow(
                vec![TsParam::new("__x", None), TsParam::new("__i", None)],
                call(
                    ident("__f"),
                    vec![
                        ident("__x"),
                        TsExpr::Index {
                            object: Box::new(ident("__ys")),
                            index: Box::new(ident("__i")),
                        },
                    ],
                ),
            );
            let body = method_call(
                method_call(
                    ident("__xs"),
                    "slice",
                    vec![num_lit("0"), member(ident("__ys"), "length")],
                ),
                "map",
                vec![pair],
            );
            iife(
                vec![
                    xs_param(&a.elem),
                    TsParam::new("__ys", None),
                    TsParam::new("__f", None),
                ],
                body,
                vec![recv, other, f],
            )
        }
        _ => unreachable!("only these arms are routed here"),
    }
}
