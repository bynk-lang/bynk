//! #4246: the lowerings of `filterMap`, `takeWhile`, `skipWhile` and the predicate form of
//! `count` on a `Query`. A child of `query` so that [`Arm`] and the thunk wrappers stay private
//! to it, and `query.rs` does not grow.

use bynk_ts::{TsArrowBody, TsBinaryOp, TsExpr, TsParam};

use super::{Arm, each, q, terminal_node, thunk_node};
use crate::ts_build::{
    binary, block_arrow, call, cond_expr, const_typed, ident, member, method_call, not_expr,
    num_lit, ret, str_lit,
};

impl Arm<'_> {
    /// `filterMap`: `__q.flatMap((__x) => { const __o = (f)(__x); return __o.tag === "Some" ?
    /// [__o.value] : []; })`, a deferred thunk like `map`.
    pub(super) fn filter_map(self) -> TsExpr {
        self.built(|o, [f]| {
            let keep = block_arrow(
                vec![TsParam::new("__x", None)],
                false,
                vec![
                    const_typed("__o", None, call(f, vec![ident("__x")])),
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
            let body = method_call(q(), "flatMap", vec![keep]);
            thunk_node(o, false, TsArrowBody::Expr(Box::new(body)))
        })
    }

    /// `takeWhile` and `skipWhile`: the prefix ends at the first element the predicate rejects,
    /// `length` when it holds for every one. A deferred thunk like `take` and `skip`; the
    /// source is read whole, then cut, so the cost is the read the query already paid.
    pub(super) fn while_prefix(self, take: bool) -> TsExpr {
        self.built(|o, [p]| {
            let rejects = method_call(
                q(),
                "findIndex",
                vec![crate::ts_build::arrow(
                    vec![TsParam::new("__x", None)],
                    not_expr(call(p, vec![ident("__x")])),
                )],
            );
            let end = cond_expr(
                binary(TsBinaryOp::LessThan, ident("__i"), num_lit("0")),
                member(q(), "length"),
                ident("__i"),
            );
            let slice = if take {
                method_call(q(), "slice", vec![num_lit("0"), end])
            } else {
                method_call(q(), "slice", vec![end])
            };
            let body = vec![const_typed("__i", None, rejects), ret(slice)];
            thunk_node(o, false, TsArrowBody::Multiline(body))
        })
    }

    /// `count(p)`: the elements `p` holds for, counted without a builder: `__q.filter(…).length`.
    pub(super) fn count_where(self) -> TsExpr {
        self.built(|o, [p]| {
            let kept = method_call(q(), "filter", vec![each(p)]);
            terminal_node(o, member(kept, "length"))
        })
    }
}
