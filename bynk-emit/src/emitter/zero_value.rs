//! Renders the checker's target-neutral [`ZeroValue`] as TypeScript.
//!
//! The checker decides whether a type has a zero value; this module is the only
//! place that knows how the target spells one (#3165).

use bynk_check::checker::ZeroValue;
use bynk_ts::{TsExpr, TsLit};

/// The TypeScript text of a zero value.
pub(super) fn render_zero_value(z: &ZeroValue) -> String {
    bynk_ts::print_expr(&zero_expr(z))
}

fn zero_expr(z: &ZeroValue) -> TsExpr {
    match z {
        ZeroValue::Number => TsExpr::Lit(TsLit::Num("0".to_string())),
        ZeroValue::False => TsExpr::Lit(TsLit::Bool(false)),
        ZeroValue::EmptyString => TsExpr::Lit(TsLit::Str(String::new())),
        // An empty octet sequence, erased to an empty `Uint8Array` (ADR 0142).
        ZeroValue::EmptyBytes => TsExpr::New {
            callee: Box::new(TsExpr::Member {
                object: Box::new(TsExpr::Ident("globalThis".to_string())),
                property: "Uint8Array".to_string(),
            }),
            args: Vec::new(),
        },
        ZeroValue::None => TsExpr::Ident("None".to_string()),
        ZeroValue::Record(fields) => TsExpr::object(
            fields
                .iter()
                .map(|(name, z)| (name.clone(), zero_expr(z)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(z: ZeroValue) -> String {
        bynk_ts::print_expr(&zero_expr(&z))
    }

    #[test]
    fn scalars_render_as_before() {
        assert_eq!(render(ZeroValue::Number), "0");
        assert_eq!(render(ZeroValue::False), "false");
        assert_eq!(render(ZeroValue::EmptyString), "\"\"");
        assert_eq!(render(ZeroValue::EmptyBytes), "new globalThis.Uint8Array()");
        assert_eq!(render(ZeroValue::None), "None");
    }

    #[test]
    fn records_render_as_before() {
        let z = ZeroValue::Record(vec![
            ("n".to_string(), ZeroValue::Number),
            (
                "inner".to_string(),
                ZeroValue::Record(vec![("s".to_string(), ZeroValue::EmptyString)]),
            ),
        ]);
        assert_eq!(render(z), "{ n: 0, inner: { s: \"\" } }");
    }
}
