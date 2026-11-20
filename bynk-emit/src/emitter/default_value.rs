//! A record field's declared default, as a TypeScript node (back-half paydown 1A.31, #3732).
//!
//! `bynk-check` decides what a default is (`field_default_value`, type-directed) and returns it
//! as plain data; this turns that data into the in-memory value, so a record literal that omits
//! a defaulted field stays a node. Each tag is `as const`, so an unannotated `const x = { … }`
//! in a test body does not widen `tag` to `string` (#3404). The value is structural, so it needs
//! no import into the module that builds the record.

use bynk_check::wire_default::DefaultValue;
use bynk_ts::{TsExpr, TsLit, TsObjectEntry};

/// The node for a default value.
pub(super) fn default_value_node(value: &DefaultValue) -> TsExpr {
    match value {
        DefaultValue::Num(digits) => TsExpr::Lit(TsLit::Num(digits.clone())),
        DefaultValue::Str(text) => TsExpr::Lit(TsLit::Str(text.clone())),
        DefaultValue::Bool(b) => TsExpr::Lit(TsLit::Bool(*b)),
        DefaultValue::Record(fields) => object(
            fields
                .iter()
                .map(|(key, v)| TsObjectEntry::Prop(key.clone(), default_value_node(v)))
                .collect(),
        ),
        DefaultValue::Sum { variant, fields } => tagged(
            variant,
            fields
                .iter()
                .map(|(key, v)| TsObjectEntry::Prop(key.clone(), default_value_node(v)))
                .collect(),
        ),
        DefaultValue::Some(v) => payload("Some", "value", v),
        DefaultValue::None => tagged("None", vec![]),
        DefaultValue::Ok(v) => payload("Ok", "value", v),
        DefaultValue::Err(v) => payload("Err", "error", v),
        DefaultValue::List(items) => TsExpr::Array {
            items: items.iter().map(default_value_node).collect(),
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
    let mut entries = vec![TsObjectEntry::Prop("tag".to_string(), tag)];
    entries.extend(rest);
    object(entries)
}

/// `{ tag: "<variant>" as const, <key>: <value> }`.
fn payload(variant: &str, key: &str, value: &DefaultValue) -> TsExpr {
    tagged(
        variant,
        vec![TsObjectEntry::Prop(
            key.to_string(),
            default_value_node(value),
        )],
    )
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

    /// Every case: the tree, and the text the deleted `lower_field_default_value` printed for it
    /// (captured before it was deleted; `bynk-check`'s tests pair the same text with the same trees).
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
            assert_eq!(print_expr(&default_value_node(&value)), old, "{value:?}");
        }
    }

    /// Decision C1: a record with no fields prints `{}` on the node path, where the deleted
    /// renderer wrote `{  }` (`format!("{{ {} }}", "")`). A field-less record type is legal
    /// (`type Events = { }`), so this is reachable as a default; whether any fixture reaches it is
    /// reported in the PR (it would be a group (b) change).
    #[test]
    fn an_empty_record_prints_as_braces() {
        assert_eq!(
            print_expr(&default_value_node(&DefaultValue::Record(vec![]))),
            "{}"
        );
    }

    /// Decision C2: single-line spacing inside the braces and `as const` inside a property.
    #[test]
    fn single_line_objects_keep_their_spacing() {
        let nested = some(DefaultValue::None);
        assert_eq!(
            print_expr(&default_value_node(&nested)),
            "{ tag: \"Some\" as const, value: { tag: \"None\" as const } }"
        );
    }

    /// Decision C3: the sign is in the text, so a negative number is one literal, never `-(-x)`.
    #[test]
    fn a_negative_number_is_one_literal() {
        assert_eq!(print_expr(&default_value_node(&num("-5"))), "-5");
        let in_list = DefaultValue::List(vec![num("-1"), num("-2")]);
        assert_eq!(print_expr(&default_value_node(&in_list)), "[-1, -2]");
    }

    /// Decision C4: a float lexeme prints as written.
    #[test]
    fn a_float_lexeme_prints_as_written() {
        for lexeme in ["1e3", "5", "0.5", "-2.5e-3"] {
            assert_eq!(print_expr(&default_value_node(&num(lexeme))), lexeme);
        }
    }
}
