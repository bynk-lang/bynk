//! Renders the checker's target-neutral [`ZeroValue`] as TypeScript.
//!
//! The checker decides whether a type has a zero value; this module is the only
//! place that knows how the target spells one (#3165).

use std::collections::HashMap;
use std::sync::Arc;

use bynk_check::checker::ZeroValue;
use bynk_syntax::ast::{Refinement, TypeDecl, TypeRef};
use bynk_ts::{TsExpr, TsLit};

/// The TypeScript zero-value expression for `type_ref` (with an optional inline
/// field refinement), or `None` if the type is not zeroable.
pub(super) fn zero_value_ts(
    type_ref: &TypeRef,
    inline: Option<&Refinement>,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<String> {
    bynk_check::checker::zero_value(type_ref, inline, types)
        .map(|z| bynk_ts::print_expr(&zero_expr(&z)))
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
