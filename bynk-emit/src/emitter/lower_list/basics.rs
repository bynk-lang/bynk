//! #3962: lowerings for `reverse`, `find`, `contains`, `last`, `lastOrElse`,
//! `isEmpty` and the keyless `sort`/`min`/`max`. Kept apart so `lower_list.rs`
//! does not grow. (`traverse` is `traverseAll`'s lowering.)

use bynk_check::checker::TyId;
use bynk_check::kernel_op::ListOp;
use bynk_ts::{TsBinaryOp, TsExpr, TsParam, TsType};

use super::{
    ListArm, bare_comparator, iife, iife_block, min_max_node, sort_node, sort_plain_node, xs_param,
};
use crate::emitter::lower::Pre;
use crate::emitter::lower::equality_helper;
use crate::emitter::lower_ctx::LowerCtx;
use crate::ts_build::{
    arrow, binary, call, const_typed, ident, member, method_call, num_lit, paren, ret, spread,
};
use bynk_syntax::ast::BaseType;

/// `__xs[__xs.length - 1]`.
fn last_item() -> TsExpr {
    TsExpr::Index {
        object: Box::new(ident("__xs")),
        index: Box::new(binary(
            TsBinaryOp::Sub,
            member(ident("__xs"), "length"),
            num_lit("1".to_string()),
        )),
    }
}

/// `__xs.length > 0`.
fn non_empty() -> TsExpr {
    binary(
        TsBinaryOp::GreaterThan,
        member(ident("__xs"), "length"),
        num_lit("0".to_string()),
    )
}

/// `(__v) => __v`: the key of a keyless ordering.
fn identity(a: &ListArm) -> TsExpr {
    arrow(
        vec![TsParam::new("__v", Some(a.elem.clone()))],
        ident("__v"),
    )
}

/// The element's comparator when it is not a plain `Int`/`String`/`Duration`/
/// `Instant` (the same rule as a key's, `kernel_key_comparator`).
fn element_comparator(elem: TyId, cx: &LowerCtx) -> Option<TsExpr> {
    let tys = cx.ty_render();
    let plain = matches!(
        elem.base(tys.types()),
        Some(BaseType::Int | BaseType::String | BaseType::Duration | BaseType::Instant)
    );
    (!plain).then(|| cx.comparator(elem))
}

pub(super) fn lower_list_basic(
    a: &ListArm,
    op: ListOp,
    elem: TyId,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> Option<TsExpr> {
    let args = a.args;
    Some(match (op, args) {
        (ListOp::Reverse, []) => {
            let recv = pre.lower_node(a.receiver, cx);
            let body = method_call(
                TsExpr::array(vec![spread(ident("__xs"))]),
                "reverse",
                vec![],
            );
            iife(vec![xs_param(&a.elem)], body, vec![recv])
        }
        (ListOp::IsEmpty, []) => {
            let recv = pre.lower_node(a.receiver, cx);
            binary(
                TsBinaryOp::StrictEq,
                member(paren(recv), "length"),
                num_lit("0".to_string()),
            )
        }
        (ListOp::Last, []) => {
            let recv = pre.lower_node(a.receiver, cx);
            let body = TsExpr::Conditional {
                test: Box::new(non_empty()),
                consequent: Box::new(call(ident("Some"), vec![last_item()])),
                alternate: Box::new(ident("None")),
            };
            iife(vec![xs_param(&a.elem)], body, vec![recv])
        }
        (ListOp::LastOrElse, [default]) => {
            let recv = pre.lower_node(a.receiver, cx);
            let default = pre.lower_node(default, cx);
            let body = TsExpr::Conditional {
                test: Box::new(non_empty()),
                consequent: Box::new(last_item()),
                alternate: Box::new(ident("__d")),
            };
            iife(
                vec![xs_param(&a.elem), TsParam::new("__d", Some(a.elem.clone()))],
                body,
                vec![recv, default],
            )
        }
        (ListOp::Find, [p]) => {
            let recv = pre.lower_node(a.receiver, cx);
            let p = pre.lower_node(p, cx);
            // `findIndex`, not `find`: an element that is itself `undefined` is
            // still found.
            let body = vec![
                const_typed(
                    "__i",
                    None,
                    method_call(ident("__xs"), "findIndex", vec![ident("__p")]),
                ),
                ret(TsExpr::Conditional {
                    test: Box::new(binary(
                        TsBinaryOp::GreaterThanEq,
                        ident("__i"),
                        num_lit("0".to_string()),
                    )),
                    consequent: Box::new(call(
                        ident("Some"),
                        vec![TsExpr::Index {
                            object: Box::new(ident("__xs")),
                            index: Box::new(ident("__i")),
                        }],
                    )),
                    alternate: Box::new(ident("None")),
                }),
            ];
            iife_block(
                vec![xs_param(&a.elem), TsParam::new("__p", None)],
                false,
                body,
                vec![recv, p],
            )
        }
        (ListOp::Contains, [item]) => {
            let helper = equality_helper(Some(elem), cx.commons().tys(), cx);
            let recv = pre.lower_node(a.receiver, cx);
            let item = pre.lower_node(item, cx);
            let same = match helper {
                Some(h) => call(ident(h), vec![ident("__y"), ident("__v")]),
                None => binary(TsBinaryOp::StrictEq, ident("__y"), ident("__v")),
            };
            let body = method_call(
                ident("__xs"),
                "some",
                vec![arrow(vec![TsParam::new("__y", Some(a.elem.clone()))], same)],
            );
            iife(
                vec![xs_param(&a.elem), TsParam::new("__v", Some(a.elem.clone()))],
                body,
                vec![recv, item],
            )
        }
        (ListOp::Sort, []) => {
            let cmp = element_comparator(elem, cx);
            let recv = pre.lower_node(a.receiver, cx);
            match cmp {
                None => sort_plain_node(a, false, recv, identity(a)),
                Some(cmp) => {
                    typed_comparator(a, cx, cmp, |c| sort_node(a, false, c, recv, identity(a)))
                }
            }
        }
        (ListOp::Min | ListOp::Max, []) => {
            let cmp_op = if op == ListOp::Min {
                TsBinaryOp::LessThan
            } else {
                TsBinaryOp::GreaterThan
            };
            let cmp = element_comparator(elem, cx);
            let recv = pre.lower_node(a.receiver, cx);
            match cmp {
                None => min_max_node(a, cmp_op, None, recv, identity(a)),
                Some(cmp) => typed_comparator(a, cx, cmp, |c| {
                    min_max_node(a, cmp_op, Some(c.clone()), recv, identity(a))
                }),
            }
        }
        _ => return None,
    })
}

/// Bind a built comparator as `(__cmp: __BynkCmp<E>) => body(__cmp)`, so the
/// element type reaches the comparator: an unannotated parameter would infer
/// `__BynkCmp<unknown>` for a recursive type's `__bynkCmpFix`. A bare comparator
/// is used as it is.
fn typed_comparator(
    a: &ListArm,
    cx: &LowerCtx,
    cmp: TsExpr,
    body: impl FnOnce(&TsExpr) -> TsExpr,
) -> TsExpr {
    if bare_comparator(&cmp).is_some() {
        return body(&cmp);
    }
    cx.runtime_use().note_cmp("__BynkCmp");
    let ty = TsType::named_with_args("__BynkCmp", vec![a.elem.clone()]);
    iife(
        vec![TsParam::new("__cmp", Some(ty))],
        body(&ident("__cmp")),
        vec![cmp],
    )
}
