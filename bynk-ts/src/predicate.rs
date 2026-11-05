//! The `PredKind` → TypeScript runtime-check mapping and the TypeScript string-literal
//! escaper it depends on. Moved here from `bynk-emit`'s `emitter` (crate-hygiene A2,
//! `design/tracks/crate-hygiene.md` §5) so the language server can build the same
//! condition without depending on the emitter; behaviour is unchanged.

use crate::program::{TsBinaryOp, TsExpr, TsLit};
use bynk_syntax::ast::{BaseType, PredKind};

/// Escapes a string for embedding in a TypeScript double-quoted string literal.
/// The one definition: the printer's `TsLit::Str`, `bynk-emit`'s `escape_ts_string`
/// and `bynk-check`'s wire defaults all splice into generated TypeScript and must
/// never disagree.
pub fn escape_ts_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            // #2619: `\0`, other control characters and the JS line terminators
            // U+2028/U+2029 print as `\u{…}` escapes (valid in strings and templates).
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => {
                out.push_str(&format!("\\u{{{:X}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// #2622: a coefficient as a signed `bigint` literal body: no leading zeros
/// (`007n` is not valid TypeScript) and no negative zero.
pub fn decimal_signed_coef(coef: &str) -> String {
    let (neg, mag) = match coef.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, coef),
    };
    let mag = mag.trim_start_matches('0');
    let mag = if mag.is_empty() { "0" } else { mag };
    let sign = if neg && mag != "0" { "-" } else { "" };
    format!("{sign}{mag}")
}

/// #661 (Decision D)/#70 review: the one `PredKind` → runtime-check mapping,
/// shared by the owner-side check (`emit::emit_pred_check`, over a `value`
/// binding) and the boundary-side inline check
/// (`serialisation::emit_inline_pred_check`, over a `json` binding) — the
/// two used to hand-roll this mapping independently, pinned identical only by
/// a comment, so amending one (e.g. the `Matches` regex's `^(?:…)$` anchoring)
/// could silently drift from the other. `receiver` is the bound name the
/// generated condition reads (`value` or `json`); the returned message is the
/// same either side of the boundary by construction.
///
/// #1471: `cond` is a real [`crate::TsExpr`], not opaque text — the
/// blocker (`crate::TsBinaryOp` had no `>=`/`<=`) is resolved by that
/// enum's own new `GreaterThanEq`/`LessThanEq` variants. Both real callers
/// wrap the returned expression in their own `Unary::Not`/`Paren` unchanged;
/// only this function's own arms changed, from `format!`ing condition text
/// to building the equivalent node tree.
///
/// Review of #1336: both real callers (`emit::emit_pred_check`,
/// `serialisation::emit_inline_pred_check`) splice the returned `message`
/// straight into a TypeScript string literal **unescaped** — safe today only
/// because every arm's own message is either static English text or already
/// `escape_ts_literal`-escaped (the `Matches` arm's own pattern). A future arm
/// returning raw, unescaped text (a predicate carrying a user-supplied string
/// operand, say) would emit a malformed or injectable literal at both call
/// sites, and nothing in the existing fixture corpus would catch it. This
/// invariant must match at every arm added here, not just the ones that exist
/// today — return plain text or already-`escape_ts_literal`-escaped text only.
/// `msg` stays exactly this opaque `String`, unchanged by #1471: only the
/// `cond` side of the pair became a real node (see the `Matches` arm below
/// for why the message keeps its own already-escaped copy of the pattern
/// separate from the condition's raw, unescaped one).
pub fn pred_condition_and_message(pred: &PredKind, receiver: &str) -> (crate::TsExpr, String) {
    pred_condition_and_message_for(pred, receiver, None)
}

/// #2622: the condition a `Decimal` value must satisfy for `pred`, over the
/// `{ coefficient, scale }` object (no runtime import). #2955: built as a real
/// [`TsExpr`] like every other arm (#1471), not `format!`-ed text; the explicit
/// [`TsExpr::Paren`]s reproduce the pre-#2955 text byte for byte.
fn decimal_pred_condition(pred: &PredKind, recv: &str) -> Option<(TsExpr, String)> {
    let coefficient = || decimal_member(recv, "coefficient");
    match pred {
        PredKind::NonNegative => Some((
            binary(TsBinaryOp::GreaterThanEq, coefficient(), bigint("0")),
            "must be non-negative".to_string(),
        )),
        PredKind::Positive => Some((
            binary(TsBinaryOp::GreaterThan, coefficient(), bigint("0")),
            "must be positive".to_string(),
        )),
        PredKind::InRangeD(a, b) => Some((
            decimal_range_expr(recv, a, b),
            format!(
                "must be in range [{}, {}]",
                bynk_syntax::decimal::render(&a.coef, a.scale),
                bynk_syntax::decimal::render(&b.coef, b.scale)
            ),
        )),
        _ => None,
    }
}

fn binary(op: TsBinaryOp, left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn paren(e: TsExpr) -> TsExpr {
    TsExpr::Paren(Box::new(e))
}

/// A `bigint` literal: `digits` plus the `n` suffix.
fn bigint(digits: &str) -> TsExpr {
    TsExpr::Lit(TsLit::Num(format!("{digits}n")))
}

/// `(recv).property`.
fn decimal_member(recv: &str, property: &str) -> TsExpr {
    TsExpr::Member {
        object: Box::new(paren(TsExpr::Ident(recv.to_string()))),
        property: property.to_string(),
    }
}

/// `10n ** globalThis.BigInt(exponent)`.
fn pow10(exponent: TsExpr) -> TsExpr {
    binary(
        TsBinaryOp::Exp,
        bigint("10"),
        TsExpr::Call {
            callee: Box::new(TsExpr::Ident("globalThis.BigInt".to_string())),
            args: vec![exponent],
        },
    )
}

/// `lo <= recv <= hi` over `Decimal` objects, comparing at a common scale with
/// `bigint` arithmetic so the check needs no helper import.
fn decimal_range_expr(
    recv: &str,
    lo: &bynk_syntax::ast::DecimalBound,
    hi: &bynk_syntax::ast::DecimalBound,
) -> TsExpr {
    let bound = |op, b: &bynk_syntax::ast::DecimalBound| {
        let scaled_recv = binary(
            TsBinaryOp::Mul,
            decimal_member(recv, "coefficient"),
            pow10(TsExpr::Lit(TsLit::Num(b.scale.to_string()))),
        );
        let scaled_bound = binary(
            TsBinaryOp::Mul,
            paren(bigint(&decimal_signed_coef(&b.coef))),
            pow10(decimal_member(recv, "scale")),
        );
        paren(binary(op, scaled_recv, scaled_bound))
    };
    paren(binary(
        TsBinaryOp::And,
        bound(TsBinaryOp::GreaterThanEq, lo),
        bound(TsBinaryOp::LessThanEq, hi),
    ))
}

/// [`decimal_range_expr`] printed — for `bynk-emit`'s `lower.rs`, which still
/// assembles its refinement guard from text.
pub fn decimal_range_condition(
    recv: &str,
    lo: &bynk_syntax::ast::DecimalBound,
    hi: &bynk_syntax::ast::DecimalBound,
) -> String {
    crate::print_expr(&decimal_range_expr(recv, lo, hi))
}

/// [`pred_condition_and_message`] knowing the receiver's base type: a
/// `Decimal` is an object, so its numeric predicates are not `recv > 0`.
pub fn pred_condition_and_message_for(
    pred: &PredKind,
    receiver: &str,
    base: Option<BaseType>,
) -> (crate::TsExpr, String) {
    if (base == Some(BaseType::Decimal) || matches!(pred, PredKind::InRangeD(..)))
        && let Some((cond, msg)) = decimal_pred_condition(pred, receiver)
    {
        return (cond, msg);
    }

    let recv = || TsExpr::Ident(receiver.to_string());
    let recv_length = || TsExpr::Member {
        object: Box::new(recv()),
        property: "length".to_string(),
    };
    let num = |n: String| TsExpr::Lit(TsLit::Num(n));
    let cmp = |op, left, right| TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    };

    match pred {
        PredKind::NonNegative => (
            cmp(TsBinaryOp::GreaterThanEq, recv(), num("0".to_string())),
            "must be non-negative".to_string(),
        ),
        PredKind::Positive => (
            cmp(TsBinaryOp::GreaterThan, recv(), num("0".to_string())),
            "must be positive".to_string(),
        ),
        PredKind::InRange(a, b) => {
            let (a, b) = (a.value, b.value);
            (
                cmp(
                    TsBinaryOp::And,
                    cmp(TsBinaryOp::GreaterThanEq, recv(), num(a.to_string())),
                    cmp(TsBinaryOp::LessThanEq, recv(), num(b.to_string())),
                ),
                format!("must be in range [{a}, {b}]"),
            )
        }
        PredKind::InRangeF(a, b) => {
            let (a, b) = (&a.lexeme, &b.lexeme);
            (
                cmp(
                    TsBinaryOp::And,
                    cmp(TsBinaryOp::GreaterThanEq, recv(), num(a.clone())),
                    cmp(TsBinaryOp::LessThanEq, recv(), num(b.clone())),
                ),
                format!("must be in range [{a}, {b}]"),
            )
        }
        // Handled above (a `Decimal` base); not reachable for another base.
        PredKind::InRangeD(..) => unreachable!("decimal predicates return early"),
        PredKind::NonEmpty => (
            cmp(TsBinaryOp::GreaterThan, recv_length(), num("0".to_string())),
            "must be non-empty".to_string(),
        ),
        // #2867: the runtime helper owns the calendar; callers `note_predicate`.
        PredKind::IsoDate => (
            TsExpr::Call {
                callee: Box::new(TsExpr::Ident("__bynkIsIsoDate".to_string())),
                args: vec![recv()],
            },
            "must be a calendar date (YYYY-MM-DD)".to_string(),
        ),
        PredKind::MinLength(n) => (
            cmp(TsBinaryOp::GreaterThanEq, recv_length(), num(n.to_string())),
            format!("length must be at least {n}"),
        ),
        PredKind::MaxLength(n) => (
            cmp(TsBinaryOp::LessThanEq, recv_length(), num(n.to_string())),
            format!("length must be at most {n}"),
        ),
        PredKind::Length(n) => (
            cmp(TsBinaryOp::StrictEq, recv_length(), num(n.to_string())),
            format!("length must be exactly {n}"),
        ),
        PredKind::Matches(pat) => {
            // The condition's own `RegExp` source uses `pat` raw, not
            // `escaped` — `TsLit::Str`'s printer already applies the exact
            // same escaping as `escape_ts_literal`
            // (`bynk_check::wire_default::escape_ts_literal`, kept
            // deliberately identical — see `bynk-ts/src/printer.rs`'s own
            // `render_lit`), so escaping `pat` here too would double-escape
            // it, same bug class `msg`'s own doc above already argues
            // against. `escaped` is still needed for `msg`, which is opaque
            // text spliced with no further escaping.
            let escaped = escape_ts_literal(pat);
            let pattern = cmp(
                TsBinaryOp::Add,
                cmp(
                    TsBinaryOp::Add,
                    TsExpr::Lit(TsLit::Str("^(?:".to_string())),
                    TsExpr::Lit(TsLit::Str(pat.clone())),
                ),
                TsExpr::Lit(TsLit::Str(")$".to_string())),
            );
            let regexp = TsExpr::New {
                callee: Box::new(TsExpr::Ident("globalThis.RegExp".to_string())),
                args: vec![pattern],
            };
            let test_call = TsExpr::Call {
                callee: Box::new(TsExpr::Member {
                    object: Box::new(regexp),
                    property: "test".to_string(),
                }),
                args: vec![recv()],
            };
            (test_call, format!("must match /{escaped}/"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{decimal_range_condition, decimal_signed_coef, pred_condition_and_message_for};
    use bynk_syntax::ast::{BaseType, DecimalBound, PredKind};
    use bynk_syntax::span::Span;

    fn bound(coef: &str, scale: u32) -> DecimalBound {
        DecimalBound {
            coef: coef.to_string(),
            scale,
            lexeme: String::new(),
            span: Span::new(0, 0),
        }
    }

    /// #2955: the text the `Decimal` arms `format!`-ed before they became
    /// `TsExpr` nodes — kept as the oracle so the node tree stays byte-identical.
    fn legacy_range(recv: &str, lo: &DecimalBound, hi: &DecimalBound) -> String {
        let bound = |op: &str, b: &DecimalBound| {
            let coef = decimal_signed_coef(&b.coef);
            format!(
                "(({recv}).coefficient * 10n ** globalThis.BigInt({}) {op} ({coef}n) * 10n ** globalThis.BigInt(({recv}).scale))",
                b.scale
            )
        };
        format!("({} && {})", bound(">=", lo), bound("<=", hi))
    }

    fn decimal_cond(pred: &PredKind, recv: &str) -> String {
        let (cond, _) = pred_condition_and_message_for(pred, recv, Some(BaseType::Decimal));
        crate::print_expr(&cond)
    }

    #[test]
    fn decimal_sign_predicates_print_the_legacy_text() {
        for recv in ["value", "json", "__v", "json[\"amount\"]"] {
            assert_eq!(
                decimal_cond(&PredKind::NonNegative, recv),
                format!("({recv}).coefficient >= 0n")
            );
            assert_eq!(
                decimal_cond(&PredKind::Positive, recv),
                format!("({recv}).coefficient > 0n")
            );
        }
    }

    #[test]
    fn decimal_range_prints_the_legacy_text() {
        let cases = [
            (bound("0", 0), bound("100", 0)),
            (bound("-12", 2), bound("007", 1)),
            (bound("-0", 3), bound("-000", 0)),
            (bound("-1999", 3), bound("25", 5)),
        ];
        for recv in ["value", "json", "__v"] {
            for (lo, hi) in &cases {
                let want = legacy_range(recv, lo, hi);
                assert_eq!(decimal_range_condition(recv, lo, hi), want);
                let pred = PredKind::InRangeD(lo.clone(), hi.clone());
                assert_eq!(decimal_cond(&pred, recv), want);
                // `InRangeD` takes the `Decimal` path even with no base given.
                let (cond, _) = pred_condition_and_message_for(&pred, recv, None);
                assert_eq!(crate::print_expr(&cond), want);
            }
        }
    }

    #[test]
    fn decimal_coefficients_drop_leading_zeros_and_negative_zero() {
        assert_eq!(decimal_signed_coef("007"), "7");
        assert_eq!(decimal_signed_coef("-0"), "0");
        assert_eq!(decimal_signed_coef("-000"), "0");
        assert_eq!(decimal_signed_coef("-12"), "-12");
        assert_eq!(decimal_signed_coef("0"), "0");
    }
}
