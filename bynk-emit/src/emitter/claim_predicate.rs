//! A refinement actor's claim predicate, as a TypeScript boolean expression tree.
//!
//! The scheme is verified first (a failure there is 401); a failed predicate is 403, checked
//! against the verified claims. The predicate itself is `bynk_check::actors::ClaimPredicate`;
//! this module builds the tree, which `bynk-check` cannot (it does not depend on `bynk-ts`).

use bynk_check::actors::ClaimPredicate;
use bynk_ts::TsExpr;

use crate::ts_build::{
    and_expr, call, ident, index, member, not_expr, or_expr, paren, str_lit, strict_eq,
};

/// The expression `pred` lowers to over `claims`, the verified claims object
/// (`Record<string, unknown>`): `globalThis.Boolean(claims["n"])` for `hasClaim`,
/// `(claims["n"] === "v")` for `claimEquals`, and a parenthesised `&&`, `||` or `!` over the
/// operands otherwise. The brackets are explicit [`TsExpr::Paren`]s, so the text does not depend
/// on the printer's own operand rules.
pub(crate) fn claim_predicate_node(pred: &ClaimPredicate, claims: &TsExpr) -> TsExpr {
    let claim = |name: &str| index(claims.clone(), str_lit(name));
    match pred {
        ClaimPredicate::HasClaim(name) => {
            call(member(ident("globalThis"), "Boolean"), vec![claim(name)])
        }
        ClaimPredicate::ClaimEquals(name, value) => {
            paren(strict_eq(claim(name), str_lit(value.as_str())))
        }
        ClaimPredicate::And(l, r) => paren(and_expr(
            claim_predicate_node(l, claims),
            claim_predicate_node(r, claims),
        )),
        ClaimPredicate::Or(l, r) => paren(or_expr(
            claim_predicate_node(l, claims),
            claim_predicate_node(r, claims),
        )),
        ClaimPredicate::Not(inner) => paren(not_expr(claim_predicate_node(inner, claims))),
    }
}

/// `!(<predicate>)`: the guard a refinement actor's 403 branch tests.
pub(crate) fn claim_predicate_denied(pred: &ClaimPredicate) -> TsExpr {
    let claims = member(member(ident("__claims"), "value"), "claims");
    not_expr(paren(claim_predicate_node(pred, &claims)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_ts::print_expr;

    fn has(n: &str) -> Box<ClaimPredicate> {
        Box::new(ClaimPredicate::HasClaim(n.to_string()))
    }

    #[test]
    fn every_predicate_form_prints_with_its_brackets() {
        let pred = ClaimPredicate::And(
            Box::new(ClaimPredicate::ClaimEquals("role".into(), "admin".into())),
            Box::new(ClaimPredicate::Or(
                has("a"),
                Box::new(ClaimPredicate::Not(has("b"))),
            )),
        );
        assert_eq!(
            print_expr(&claim_predicate_denied(&pred)),
            "!(((__claims.value.claims[\"role\"] === \"admin\") && (globalThis.Boolean(__claims.value.claims[\"a\"]) || (!globalThis.Boolean(__claims.value.claims[\"b\"])))))"
        );
    }

    #[test]
    fn a_quote_and_a_backslash_in_a_claim_are_escaped() {
        let pred = ClaimPredicate::ClaimEquals("a\"b".into(), "c\\d".into());
        assert_eq!(
            print_expr(&claim_predicate_denied(&pred)),
            "!((__claims.value.claims[\"a\\\"b\"] === \"c\\\\d\"))"
        );
    }
}
