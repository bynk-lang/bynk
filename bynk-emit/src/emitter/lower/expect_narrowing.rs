//! #3775: an `expect` that compares a read against a literal narrows that read in TypeScript.
//!
//! `expect a == 1500` lowers to `if (!(a === 1500)) throw …`, so for the statements after it
//! TypeScript has `a` as the literal type `1500`, and a later `expect a == 7` is a TS2367 error
//! even though the Bynk is valid. The lowering therefore widens such a read to its base type in
//! every later comparison, so no check relies on narrowing.
//!
//! #4520: the same goes for an `if` or a literal `match`: inside `if a == 7 { … }` TypeScript has
//! `a` as `7`, so `a == 8` there is TS2367. A comparison widens a read that an enclosing condition
//! or arm narrowed. Only the narrowing TypeScript really does is tracked (`==` narrows the `then`
//! branch, `!=` the `else`; a boolean narrows both ways), so a comparison that needs no cast keeps
//! its text.

use std::collections::HashSet;

use super::super::lower_ctx::LowerCtx;

/// The TypeScript base type a literal operand has, when it is a number, string or boolean literal.
fn literal_base(e: &bynk_ts::TsExpr) -> Option<&'static str> {
    // A negative number (`-1`) is a negation of a literal, which TypeScript also narrows to.
    if let bynk_ts::TsExpr::Unary {
        op: bynk_ts::TsUnaryOp::Neg,
        expr,
    } = e
    {
        return literal_base(expr).filter(|b| *b == "number");
    }
    let bynk_ts::TsExpr::Lit(lit) = e else {
        return None;
    };
    match lit {
        bynk_ts::TsLit::Num(_) => Some("number"),
        bynk_ts::TsLit::Str(_) => Some("string"),
        bynk_ts::TsLit::Bool(_) => Some("boolean"),
        bynk_ts::TsLit::Null | bynk_ts::TsLit::Raw(_) | bynk_ts::TsLit::BigInt(_) => None,
    }
}

/// A name for a plain read (`a`, `r.x`) that TypeScript narrows by control flow.
fn read_key(e: &bynk_ts::TsExpr) -> Option<String> {
    if let bynk_ts::TsExpr::Ident(n) = e {
        return Some(n.clone());
    }
    let bynk_ts::TsExpr::Member { object, property } = e else {
        return None;
    };
    read_key(object).map(|o| [o.as_str(), ".", property.as_str()].concat())
}

/// The reads a condition narrows: while it holds and while it does not.
#[derive(Default)]
struct Keys {
    when_true: HashSet<String>,
    when_false: HashSet<String>,
}

/// What a comparison `lv <op> rv` of a read against a literal narrows. A boolean narrows whichever
/// way the test goes, but `a == 7` narrows `a` to `7` only when it holds, and `a != 7` only when
/// it does not (a number or a string minus one literal is as wide as before).
fn leaf_keys(equality: bool, lv: &bynk_ts::TsExpr, rv: &bynk_ts::TsExpr) -> Keys {
    let mut keys = Keys::default();
    let Some((key, base, _)) = narrowing_target(lv, rv) else {
        return keys;
    };
    let (narrowed, other) = if equality {
        (&mut keys.when_true, &mut keys.when_false)
    } else {
        (&mut keys.when_false, &mut keys.when_true)
    };
    narrowed.insert(key.clone());
    if base == "boolean" {
        other.insert(key);
    }
    keys
}

/// The read, the base type of the literal it is compared with, and whether the read is the left
/// operand, for a comparison of one against the other.
fn narrowing_target(
    lv: &bynk_ts::TsExpr,
    rv: &bynk_ts::TsExpr,
) -> Option<(String, &'static str, bool)> {
    let (read, base, read_is_left) = match (literal_base(lv), literal_base(rv)) {
        (None, Some(base)) => (lv, base, true),
        (Some(base), None) => (rv, base, false),
        _ => return None,
    };
    read_key(read).map(|key| (key, base, read_is_left))
}

/// A comparison `lv <op> rv` with its read widened to the literal's base type when an enclosing
/// condition, `match` arm or earlier statement left that read narrowed. Records nothing.
pub(in crate::emitter) fn widen_narrowed(
    lv: bynk_ts::TsExpr,
    rv: bynk_ts::TsExpr,
    cx: &LowerCtx,
) -> (bynk_ts::TsExpr, bynk_ts::TsExpr) {
    if cx.narrowed_reads.is_empty() {
        return (lv, rv);
    }
    let Some((key, base, read_is_left)) = narrowing_target(&lv, &rv) else {
        return (lv, rv);
    };
    if !cx.narrowed_reads.contains(&key) {
        return (lv, rv);
    }
    let wide = |e| crate::ts_build::as_expr(e, bynk_ts::TsType::named(base));
    if read_is_left {
        (wide(lv), rv)
    } else {
        (lv, wide(rv))
    }
}

/// #3775: after the `expect` comparison `lv <op> rv` passes, its read is narrowed. Call it once the
/// comparison is lowered, which is what widens a read an earlier one narrowed.
pub(in crate::emitter) fn record_narrowing(
    equality: bool,
    lv: &bynk_ts::TsExpr,
    rv: &bynk_ts::TsExpr,
    cx: &mut LowerCtx,
) {
    cx.narrowed_reads
        .extend(leaf_keys(equality, lv, rv).when_true);
}

/// A condition with the comparisons of a conjunction or disjunction widened left to right, and
/// the reads it narrows. The right operand of `&&` is evaluated while the left holds, and of `||`
/// while it does not.
fn walk(cond: bynk_ts::TsExpr, cx: &mut LowerCtx) -> (bynk_ts::TsExpr, Keys) {
    use bynk_ts::{TsExpr, TsUnaryOp};
    if let Some(key) = read_key(&cond) {
        let keys = Keys {
            when_true: HashSet::from([key.clone()]),
            when_false: HashSet::from([key]),
        };
        return (cond, keys);
    }
    if let TsExpr::Binary { op, left, right } = cond {
        return walk_binary(op, *left, *right, cx);
    }
    if let TsExpr::Unary {
        op: TsUnaryOp::Not,
        expr,
    } = cond
    {
        let (inner, keys) = walk(*expr, cx);
        let negated = Keys {
            when_true: keys.when_false,
            when_false: keys.when_true,
        };
        let not = TsExpr::Unary {
            op: TsUnaryOp::Not,
            expr: Box::new(inner),
        };
        return (not, negated);
    }
    if let TsExpr::Paren(inner) = cond {
        let (inner, keys) = walk(*inner, cx);
        return (TsExpr::Paren(Box::new(inner)), keys);
    }
    (cond, Keys::default())
}

/// [`walk`] for a binary operator: `&&` and `||` (the right operand sees what the left proved),
/// and `===` and `!==` (a comparison of a read against a literal); any other is left as it is.
fn walk_binary(
    op: bynk_ts::TsBinaryOp,
    left: bynk_ts::TsExpr,
    right: bynk_ts::TsExpr,
    cx: &mut LowerCtx,
) -> (bynk_ts::TsExpr, Keys) {
    use bynk_ts::{TsBinaryOp, TsExpr};
    let rebuilt = |left, right| TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    };
    if matches!(op, TsBinaryOp::And | TsBinaryOp::Or) {
        let conjunction = op == TsBinaryOp::And;
        let (left, lk) = walk(left, cx);
        let outer = cx.narrowed_reads.clone();
        let proved = if conjunction {
            &lk.when_true
        } else {
            &lk.when_false
        };
        cx.narrowed_reads.extend(proved.iter().cloned());
        let (right, rk) = walk(right, cx);
        cx.narrowed_reads = outer;
        let both = |a: &HashSet<String>, b: &HashSet<String>| a.intersection(b).cloned().collect();
        let either = |a: &HashSet<String>, b: &HashSet<String>| a.union(b).cloned().collect();
        let keys = if conjunction {
            Keys {
                when_true: either(&lk.when_true, &rk.when_true),
                when_false: both(&lk.when_false, &rk.when_false),
            }
        } else {
            Keys {
                when_true: both(&lk.when_true, &rk.when_true),
                when_false: either(&lk.when_false, &rk.when_false),
            }
        };
        return (rebuilt(left, right), keys);
    }
    if matches!(op, TsBinaryOp::StrictEq | TsBinaryOp::StrictNotEq) {
        let keys = leaf_keys(op == TsBinaryOp::StrictEq, &left, &right);
        let (left, right) = widen_narrowed(left, right, cx);
        return (rebuilt(left, right), keys);
    }
    (rebuilt(left, right), Keys::default())
}

/// A whole `expect` condition that is not a single comparison (`expect b`, `expect a == 1 && ok`):
/// widened where an earlier conjunct narrowed a read, and what it narrows is recorded, since it
/// passed.
pub(in crate::emitter) fn narrowing_proof_cond(
    cond: bynk_ts::TsExpr,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    let (cond, keys) = walk(cond, cx);
    cx.narrowed_reads.extend(keys.when_true);
    cond
}

/// The narrowing of an `if`: what holds on entry, and what the `else` branch has.
pub(in crate::emitter) struct Branches {
    outer: HashSet<String>,
    in_else: HashSet<String>,
}

/// #4520: `cond` guards an `if`. Returns it with its own later comparisons widened, and leaves
/// `cx` as the `then` branch sees it; [`Branches::else_branch`] switches to the `else`, and
/// [`Branches::leave`] ends the narrowing.
pub(in crate::emitter) fn enter_branches(
    cond: bynk_ts::TsExpr,
    cx: &mut LowerCtx,
) -> (bynk_ts::TsExpr, Branches) {
    let outer = cx.narrowed_reads.clone();
    let (cond, keys) = walk(cond, cx);
    let mut in_else = outer.clone();
    in_else.extend(keys.when_false);
    cx.narrowed_reads.extend(keys.when_true);
    (cond, Branches { outer, in_else })
}

impl Branches {
    /// The `else` branch's narrowing replaces the `then` branch's.
    pub(in crate::emitter) fn else_branch(&self, cx: &mut LowerCtx) {
        cx.narrowed_reads = self.in_else.clone();
    }

    /// Forget what the `if` narrowed.
    pub(in crate::emitter) fn leave(self, cx: &mut LowerCtx) {
        cx.narrowed_reads = self.outer;
    }
}

/// #4520: inside the arms of a `switch` on a read, TypeScript has the read narrowed to the arm's
/// literal. Returns what to hand [`Branches::leave`]-style restoration after the arms.
pub(in crate::emitter) fn enter_arms(
    scrutinee: &bynk_ts::TsExpr,
    cx: &mut LowerCtx,
) -> HashSet<String> {
    let outer = cx.narrowed_reads.clone();
    if let Some(key) = read_key(scrutinee) {
        cx.narrowed_reads.insert(key);
    }
    outer
}

/// Restore what [`enter_arms`] returned.
pub(in crate::emitter) fn leave_arms(outer: HashSet<String>, cx: &mut LowerCtx) {
    cx.narrowed_reads = outer;
}
