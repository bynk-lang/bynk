//! #3961: the `Map` and `Set` methods that bring them level with `List`. Each
//! maps to a runtime helper in `keys.ts` that works over both the plain and the
//! composite-keyed representation, except the membership and size forms, which
//! read through `has`/`get`/`size` as every keyed collection overrides them.
//! Kept apart from `kernel_container.rs` so that file does not grow.

use bynk_check::checker::TyId;
use bynk_check::kernel_op::{MapOp, SetOp};
use bynk_ts::{TsBinaryOp, TsExpr, TsParam, TsType};

use super::super::lower::{Lowered, Pre};
use super::super::lower_ctx::LowerCtx;
use super::super::ts_types::ts_ty_to_ts_type;
use super::super::*;
use crate::ts_build::{arrow, as_expr, binary, call, cond_expr, ident, member, num_lit};

/// The receiver and then each argument, lowered in order.
fn operands(pre: &mut Pre, cx: &mut LowerCtx, parts: &[&Expr]) -> Vec<TsExpr> {
    parts.iter().map(|e| pre.lower_node(e, cx)).collect()
}

/// `helper(…kids)`, with the helper noted for the module's runtime import.
fn runtime_call(cx: &mut LowerCtx, helper: &'static str, kids: Vec<TsExpr>) -> TsExpr {
    cx.runtime_use().note_key(helper);
    call(ident(helper), kids)
}

/// `recv.size === 0`.
fn is_empty(recv: TsExpr) -> TsExpr {
    binary(
        TsBinaryOp::StrictEq,
        member(crate::ts_build::paren(recv), "size"),
        num_lit("0".to_string()),
    )
}

/// The `Map` methods #3961 adds; `None` for one it does not own.
pub(in crate::emitter) fn lower_map_more(
    receiver: &Expr,
    op: MapOp,
    args: &[Expr],
    key: TyId,
    val: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let key_t = ts_ty_to_ts_type(key, tys);
    let val_t = ts_ty_to_ts_type(val, tys);
    let map_t = TsType::named_with_args("ReadonlyMap", vec![key_t.clone(), val_t.clone()]);
    let built = match (op, args) {
        (MapOp::IsEmpty, []) => {
            let recv = pre.lower_node(receiver, cx);
            is_empty(recv)
        }
        (MapOp::Contains, [k]) => {
            let kids = operands(&mut pre, cx, &[receiver, k]);
            let [recv, k]: [TsExpr; 2] = kids.try_into().ok()?;
            call(member(crate::ts_build::paren(recv), "has"), vec![k])
        }
        (MapOp::GetOrElse, [k, d]) => {
            let kids = operands(&mut pre, cx, &[receiver, k, d]);
            let body = cond_expr(
                call(member(ident("__m"), "has"), vec![ident("__k")]),
                as_expr(
                    call(member(ident("__m"), "get"), vec![ident("__k")]),
                    val_t.clone(),
                ),
                ident("__d"),
            );
            call(
                arrow(
                    vec![
                        TsParam::new("__m", Some(map_t)),
                        TsParam::new("__k", Some(key_t)),
                        TsParam::new("__d", Some(val_t)),
                    ],
                    body,
                ),
                kids,
            )
        }
        (MapOp::Remove, [k]) => {
            let kids = operands(&mut pre, cx, &[receiver, k]);
            runtime_call(cx, "__bynkMapRemove", kids)
        }
        (MapOp::Entries, []) => {
            let kids = operands(&mut pre, cx, &[receiver]);
            runtime_call(cx, "__bynkMapEntries", kids)
        }
        (MapOp::Filter, [p]) => {
            let kids = operands(&mut pre, cx, &[receiver, p]);
            runtime_call(cx, "__bynkMapFilter", kids)
        }
        (MapOp::MapValues, [f]) => {
            let kids = operands(&mut pre, cx, &[receiver, f]);
            runtime_call(cx, "__bynkMapMapValues", kids)
        }
        (MapOp::Fold, [init, step]) => {
            let kids = operands(&mut pre, cx, &[receiver, init, step]);
            runtime_call(cx, "__bynkMapFold", kids)
        }
        _ => return None,
    };
    Some(pre.finish(built))
}

/// `Map.of(entries)`.
pub(in crate::emitter) fn lower_map_of(args: &[Expr], cx: &mut LowerCtx, pre: &mut Pre) -> TsExpr {
    let kids = operands(pre, cx, &[&args[0]]);
    runtime_call(cx, "__bynkMapOf", kids)
}

/// The `Set` methods #3961 adds; `None` for one it does not own.
pub(in crate::emitter) fn lower_set_more(
    receiver: &Expr,
    op: SetOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let helper = match (op, args.len()) {
        (SetOp::IsEmpty, 0) => {
            let recv = pre.lower_node(receiver, cx);
            return Some(pre.finish(is_empty(recv)));
        }
        (SetOp::Union, 1) => "__bynkSetUnion",
        (SetOp::Intersect, 1) => "__bynkSetIntersect",
        (SetOp::Difference, 1) => "__bynkSetDifference",
        (SetOp::IsSubsetOf, 1) => "__bynkSetIsSubsetOf",
        (SetOp::Filter, 1) => "__bynkSetFilter",
        (SetOp::Map, 1) => "__bynkSetMap",
        (SetOp::Fold, 2) => "__bynkSetFold",
        _ => return None,
    };
    let parts: Vec<&Expr> = std::iter::once(receiver).chain(args.iter()).collect();
    let kids = operands(&mut pre, cx, &parts);
    let built = runtime_call(cx, helper, kids);
    Some(pre.finish(built))
}
