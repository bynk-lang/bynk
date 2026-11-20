//! A record field's declared default, as a TypeScript node (back-half paydown 1A.31, #3732).
//!
//! `bynk-check` decides what a default is (`field_default_value`, type-directed) and returns it
//! as plain data; this turns that data into the in-memory value, so a record literal that omits
//! a defaulted field stays a node. Each tag is `as const`, so an unannotated `const x = { … }`
//! in a test body does not widen `tag` to `string` (#3404). The value is structural, so it needs
//! no import into the module that builds the record.

use bynk_check::wire_default::DefaultValue;
use bynk_ts::{TsExpr, TsLit, TsObjectEntry};

/// How a refined value is branded as its type: given the type's name and the base value's node,
/// the branded node (#3759). The caller decides, because the spelling depends on where the record
/// is built (`lower_expr` brands an admitted refined literal the same way).
pub(super) type Brand<'a> = &'a dyn Fn(&str, TsExpr) -> TsExpr;

/// The node for a default value.
pub(super) fn default_value_node(value: &DefaultValue, brand: Brand<'_>) -> TsExpr {
    let node = |v: &DefaultValue| default_value_node(v, brand);
    let entries = |fields: &[(String, DefaultValue)]| {
        fields
            .iter()
            .map(|(key, v)| TsObjectEntry::prop(key.clone(), node(v)))
            .collect()
    };
    match value {
        DefaultValue::Num(digits) => TsExpr::Lit(TsLit::Num(digits.clone())),
        DefaultValue::Str(text) => TsExpr::Lit(TsLit::Str(text.clone())),
        DefaultValue::Bool(b) => TsExpr::Lit(TsLit::Bool(*b)),
        // #3801: the in-memory decimal, the node a `Decimal` literal lowers to.
        DefaultValue::Decimal { coefficient, scale } => {
            super::lower::decimal_lit(coefficient, *scale)
        }
        DefaultValue::Refined { name, value } => brand(name, node(value)),
        DefaultValue::Record(fields) => object(entries(fields)),
        DefaultValue::Sum { variant, fields } => tagged(variant, entries(fields)),
        DefaultValue::Some(v) => payload("Some", "value", node(v)),
        DefaultValue::None => tagged("None", vec![]),
        DefaultValue::Ok(v) => payload("Ok", "value", node(v)),
        DefaultValue::Err(v) => payload("Err", "error", node(v)),
        DefaultValue::List(items) => TsExpr::Array {
            items: items.iter().map(node).collect(),
            multiline: false,
        },
        DefaultValue::EmptyMap => empty_collection("Map"),
        DefaultValue::EmptySet => empty_collection("Set"),
    }
}

/// A one-line object literal.
fn object(entries: Vec<TsObjectEntry>) -> TsExpr {
    TsExpr::object_entries(entries)
}

/// `{ tag: "<variant>" as const, …rest }`.
fn tagged(variant: &str, rest: Vec<TsObjectEntry>) -> TsExpr {
    let tag = TsExpr::AsConst(Box::new(TsExpr::Lit(TsLit::Str(variant.to_string()))));
    let mut entries = vec![TsObjectEntry::prop("tag".to_string(), tag)];
    entries.extend(rest);
    object(entries)
}

/// `{ tag: "<variant>" as const, <key>: <value> }`.
fn payload(variant: &str, key: &str, value: TsExpr) -> TsExpr {
    tagged(variant, vec![TsObjectEntry::prop(key.to_string(), value)])
}

/// `new globalThis.<collection>()`: through `globalThis`, so a type of the same name in scope
/// cannot capture it.
fn empty_collection(collection: &str) -> TsExpr {
    TsExpr::New {
        callee: Box::new(TsExpr::Member {
            object: Box::new(TsExpr::Ident("globalThis".to_string())),
            property: collection.to_string(),
        }),
        args: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_ts::print_expr;

    fn num(s: &str) -> DefaultValue {
        DefaultValue::Num(s.to_string())
    }
    fn text(s: &str) -> DefaultValue {
        DefaultValue::Str(s.to_string())
    }
    fn some(v: DefaultValue) -> DefaultValue {
        DefaultValue::Some(Box::new(v))
    }
    fn dec(coefficient: &str, scale: u32) -> DefaultValue {
        DefaultValue::Decimal {
            coefficient: coefficient.to_string(),
            scale,
        }
    }
    fn refined(name: &str, v: DefaultValue) -> DefaultValue {
        DefaultValue::Refined {
            name: name.to_string(),
            value: Box::new(v),
        }
    }
    /// The production brand: `(value as Name)`.
    fn print(value: &DefaultValue) -> String {
        let brand = |name: &str, raw: TsExpr| {
            crate::emitter::ts_types::unchecked_construct(name, raw, false)
        };
        print_expr(&default_value_node(value, &brand))
    }

    /// Every case: the tree, and the text the deleted `lower_field_default_value` printed for it
    /// (captured before it was deleted; `bynk-check`'s tests pair the same text with the same trees),
    /// apart from the `Decimal` (#3801) and refined (#3759) rows, which are the corrected text.
    fn cases() -> Vec<(DefaultValue, &'static str)> {
        let sum = |v: &str, f: Vec<(&str, DefaultValue)>| DefaultValue::Sum {
            variant: v.to_string(),
            fields: f.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        };
        let record = |f: Vec<(&str, DefaultValue)>| {
            DefaultValue::Record(f.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        };
        vec![
            (num("5"), "5"),
            (num("-5"), "-5"),
            (num("1.5"), "1.5"),
            (num("1e3"), "1e3"),
            (num("-2.5"), "-2.5"),
            (num("300000"), "300000"),
            (DefaultValue::Bool(true), "true"),
            (DefaultValue::Bool(false), "false"),
            (text("x"), "\"x\""),
            (text("a\"b\n"), "\"a\\\"b\\n\""),
            (text(""), "\"\""),
            (text("0.00"), "\"0.00\""),
            // #3801: a `Decimal` is the in-memory `{ coefficient, scale }`.
            (dec("000", 2), "{ coefficient: 0n, scale: 2 }"),
            (dec("-0", 1), "{ coefficient: 0n, scale: 1 }"),
            (dec("-150", 2), "{ coefficient: -150n, scale: 2 }"),
            // #3759: a refined value is branded by the caller's brand.
            (refined("Age", num("3")), "(3 as Age)"),
            (
                DefaultValue::List(vec![refined("Age", num("1"))]),
                "[(1 as Age)]",
            ),
            (
                record(vec![("n", num("7")), ("s", text("i"))]),
                "{ n: 7, s: \"i\" }",
            ),
            (sum("Free", vec![]), "{ tag: \"Free\" as const }"),
            (
                sum("Rect", vec![("w", num("1")), ("h", num("-2"))]),
                "{ tag: \"Rect\" as const, w: 1, h: -2 }",
            ),
            (some(num("1")), "{ tag: \"Some\" as const, value: 1 }"),
            (DefaultValue::None, "{ tag: \"None\" as const }"),
            (
                some(DefaultValue::None),
                "{ tag: \"Some\" as const, value: { tag: \"None\" as const } }",
            ),
            (
                DefaultValue::Ok(Box::new(num("1"))),
                "{ tag: \"Ok\" as const, value: 1 }",
            ),
            (
                DefaultValue::Err(Box::new(text("e"))),
                "{ tag: \"Err\" as const, error: \"e\" }",
            ),
            (DefaultValue::List(vec![]), "[]"),
            (DefaultValue::List(vec![num("1"), num("2")]), "[1, 2]"),
            (
                DefaultValue::List(vec![DefaultValue::None, some(num("2"))]),
                "[{ tag: \"None\" as const }, { tag: \"Some\" as const, value: 2 }]",
            ),
            (DefaultValue::EmptyMap, "new globalThis.Map()"),
            (DefaultValue::EmptySet, "new globalThis.Set()"),
            (
                record(vec![("value", sum("Has", vec![("value", text("s"))]))]),
                "{ value: { tag: \"Has\" as const, value: \"s\" } }",
            ),
        ]
    }

    /// Every variant prints byte for byte what the text renderer wrote.
    #[test]
    fn a_node_prints_the_text_the_string_renderer_wrote() {
        for (value, old) in cases() {
            assert_eq!(print(&value), old, "{value:?}");
        }
    }

    /// Decision C1: a record with no fields prints `{}` on the node path, where the deleted
    /// renderer wrote `{  }` (`format!("{{ {} }}", "")`). A field-less record type is legal
    /// (`type Events = { }`), so this is reachable as a default; whether any fixture reaches it is
    /// reported in the PR (it would be a group (b) change).
    #[test]
    fn an_empty_record_prints_as_braces() {
        assert_eq!(print(&DefaultValue::Record(vec![])), "{}");
    }

    /// Decision C2: single-line spacing inside the braces and `as const` inside a property.
    #[test]
    fn single_line_objects_keep_their_spacing() {
        let nested = some(DefaultValue::None);
        assert_eq!(
            print(&nested),
            "{ tag: \"Some\" as const, value: { tag: \"None\" as const } }"
        );
    }

    /// Decision C3: the sign is in the text, so a negative number is one literal, never `-(-x)`.
    #[test]
    fn a_negative_number_is_one_literal() {
        assert_eq!(print(&num("-5")), "-5");
        let in_list = DefaultValue::List(vec![num("-1"), num("-2")]);
        assert_eq!(print(&in_list), "[-1, -2]");
    }

    /// Decision C4: a float lexeme prints as written.
    #[test]
    fn a_float_lexeme_prints_as_written() {
        for lexeme in ["1e3", "5", "0.5", "-2.5e-3"] {
            assert_eq!(print(&num(lexeme)), lexeme);
        }
    }
}
