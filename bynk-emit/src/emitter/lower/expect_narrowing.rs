//! #3775: an `expect` that compares a read against a literal narrows that read in TypeScript.
//!
//! `expect a == 1500` lowers to `if (!(a === 1500)) throw …`, so for the statements after it
//! TypeScript has `a` as the literal type `1500`, and a later `expect a == 7` is a TS2367 error
//! even though the Bynk is valid. The lowering therefore widens such a read to its base type in
//! every later comparison, so no check relies on narrowing.

use bynk_syntax::ast::BinOp;

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

/// #3775: `expect a == 1500` throws unless `a === 1500`, so TypeScript narrows `a` to `1500` for
/// the statements after it, and a later `expect a == 7` is a TS2367 error. After one `==` of a
/// read against a literal, the read is widened to the literal's base type (`a as number`) in every
/// later comparison, so the check never relies on narrowing.
pub(in crate::emitter) fn narrowing_proof(
    op: BinOp,
    lv: bynk_ts::TsExpr,
    rv: bynk_ts::TsExpr,
    cx: &mut LowerCtx,
) -> (bynk_ts::TsExpr, bynk_ts::TsExpr) {
    use bynk_ts::TsType;
    if !matches!(op, BinOp::Eq | BinOp::NotEq) {
        return (lv, rv);
    }
    let (read, base, read_is_left) = match (literal_base(&lv), literal_base(&rv)) {
        (None, Some(base)) => (&lv, base, true),
        (Some(base), None) => (&rv, base, false),
        _ => return (lv, rv),
    };
    let Some(key) = read_key(read) else {
        return (lv, rv);
    };
    let seen = cx.expect_narrowed.contains(&key);
    // `b != false` narrows a boolean to `true`, as `b == true` does; for a number or a string a
    // `!=` leaves the type as it was.
    if op == BinOp::Eq || base == "boolean" {
        cx.expect_narrowed.insert(key);
    }
    if !seen {
        return (lv, rv);
    }
    let wide = |e| crate::ts_build::as_expr(e, TsType::named(base));
    if read_is_left {
        (wide(lv), rv)
    } else {
        (lv, wide(rv))
    }
}

/// [`narrowing_proof`] over a whole `expect` condition that is not a single comparison: a bare
/// boolean read (`expect b`, `expect !b`) narrows that read, and the comparisons of a conjunction or
/// disjunction (`expect a == 1 && ok`) narrow left to right, so each leaf is widened if an earlier
/// one narrowed its read.
pub(in crate::emitter) fn narrowing_proof_cond(
    cond: bynk_ts::TsExpr,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    use bynk_ts::{TsBinaryOp, TsExpr, TsUnaryOp};
    if let Some(key) = read_key(&cond) {
        cx.expect_narrowed.insert(key);
        return cond;
    }
    if let TsExpr::Binary { op, left, right } = cond {
        let (left, right) = match op {
            TsBinaryOp::And | TsBinaryOp::Or => {
                let left = narrowing_proof_cond(*left, cx);
                (left, narrowing_proof_cond(*right, cx))
            }
            TsBinaryOp::StrictEq => narrowing_proof(BinOp::Eq, *left, *right, cx),
            TsBinaryOp::StrictNotEq => narrowing_proof(BinOp::NotEq, *left, *right, cx),
            other @ (TsBinaryOp::NullishCoalescing
            | TsBinaryOp::GreaterThan
            | TsBinaryOp::InstanceOf
            | TsBinaryOp::LessThan
            | TsBinaryOp::Add
            | TsBinaryOp::In
            | TsBinaryOp::GreaterThanEq
            | TsBinaryOp::LessThanEq
            | TsBinaryOp::Mul
            | TsBinaryOp::Exp
            | TsBinaryOp::Sub
            | TsBinaryOp::Div) => {
                return TsExpr::Binary {
                    op: other,
                    left,
                    right,
                };
            }
        };
        return TsExpr::Binary {
            op,
            left: Box::new(left),
            right: Box::new(right),
        };
    }
    if let TsExpr::Unary {
        op: TsUnaryOp::Not,
        expr,
    } = cond
    {
        let inner = narrowing_proof_cond(*expr, cx);
        return TsExpr::Unary {
            op: TsUnaryOp::Not,
            expr: Box::new(inner),
        };
    }
    if let TsExpr::Paren(inner) = cond {
        return TsExpr::Paren(Box::new(narrowing_proof_cond(*inner, cx)));
    }
    cond
}
