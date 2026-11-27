//! #4046: `PgNumeric` and `MySqlDecimal` take a `Decimal`, not the `String` they took before.
//!
//! A call that still passes a string gets the ordinary `variant_payload_mismatch` plus the
//! fix: a string literal that is a plain decimal number becomes the `Decimal` literal, and any
//! other string gets a note on `Decimal.parse`.

use bynk_syntax::ast::{Expr, ExprKind};
use bynk_syntax::error::{Applicability, CompileError};

use super::{TyId, Types};

/// The variants whose payload changed from a `String` to a `Decimal`.
fn is_decimal_param(variant: &str) -> bool {
    matches!(variant, "PgNumeric" | "MySqlDecimal")
}

/// Whether `text` is a plain decimal number: an optional `-`, digits, and an optional fraction.
fn is_decimal_text(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    let (whole, fraction) = digits.split_once('.').unwrap_or((digits, "0"));
    !whole.is_empty()
        && !fraction.is_empty()
        && whole.bytes().all(|b| b.is_ascii_digit())
        && fraction.bytes().all(|b| b.is_ascii_digit())
}

/// `err` with the migration fix for a string passed to `PgNumeric` or `MySqlDecimal`, or `err`
/// unchanged for any other variant or argument type.
pub(crate) fn with_decimal_fix(
    err: CompileError,
    variant: &str,
    expected: &str,
    actual: &str,
    arg: &Expr,
) -> CompileError {
    if !is_decimal_param(variant) || expected != "Decimal" || actual != "String" {
        return err;
    }
    match &arg.kind {
        ExprKind::StrLit(text) if is_decimal_text(text) => err.with_suggestion(
            format!("write the `Decimal` literal `{text}d`"),
            vec![(arg.span, format!("{text}d"))],
            Applicability::MachineApplicable,
        ),
        _ => err.with_note(format!(
            "`{variant}` takes a `Decimal` since #4046: parse the text with `Decimal.parse(s)`, which gives an `Option[Decimal]`"
        )),
    }
}

/// Report `bynk.types.variant_payload_mismatch` for argument `i` (0-based) of `variant`. This
/// lives here, out of line, so the recursive expression checker that calls it keeps a small
/// stack frame (a 64-term chain is checked on a 2 MiB test thread).
#[inline(never)]
pub(crate) fn push_payload_mismatch(
    errors: &mut Vec<CompileError>,
    (arg, i, variant, field): (&Expr, usize, &str, &str),
    (actual, expected): (TyId, TyId),
    tys: &Types,
) {
    let err = CompileError::new(
        "bynk.types.variant_payload_mismatch",
        arg.span,
        format!(
            "argument {} to variant `{variant}` has type `{}`, but field `{field}` expects `{}`",
            i + 1,
            actual.display(tys),
            expected.display(tys)
        ),
    );
    errors.push(with_decimal_fix(
        err,
        variant,
        &expected.display(tys),
        &actual.display(tys),
        arg,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_decimal_text_is_recognised() {
        for ok in ["12.50", "-3", "0", "7.0", "-0.05"] {
            assert!(is_decimal_text(ok), "{ok}");
        }
        for bad in [
            "", "-", "1.", ".5", "1e3", "1_000", "1,5", "abc", "--1", "1.2.3",
        ] {
            assert!(!is_decimal_text(bad), "{bad}");
        }
    }

    #[test]
    fn only_the_two_changed_variants_are_decimal_params() {
        assert!(is_decimal_param("PgNumeric"));
        assert!(is_decimal_param("MySqlDecimal"));
        assert!(!is_decimal_param("PgText"));
    }
}
