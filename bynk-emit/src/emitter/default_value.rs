//! A record field's declared default, as a TypeScript node (back-half paydown 1A.31, #3732).
//!
//! `bynk-check` decides what a default is (`field_default_value`, type-directed) and returns it
//! as plain data; this turns that data into the in-memory value, so a record literal that omits
//! a defaulted field stays a node. Each tag is `as const`, so an unannotated `const x = { … }`
//! in a test body does not widen `tag` to `string` (#3404). The value is structural, so it needs
//! no import into the module that builds the record.

use bynk_check::wire_default::DefaultValue;
use bynk_ts::{TsExpr, TsLit, TsObjectEntry, TsType, TsTypeMember};

/// The node for a default value going into a position of TS type `slot`.
///
/// #3759: a refined value is its base value cast to the type of its own position, `(5 as
/// Review["rating"])`, so it is branded without naming the refined type, which the module that
/// builds the record may not import. Each position's type is indexed from `slot`: a field by its
/// key, a list element by `number`, a variant's payload through `Extract<…, { tag: "V" }>`. A slot
/// of `any` (test scaffolding, where a declared type is not nameable) stays `any` all the way down.
/// `None` leaves a refined value unbranded (a value with none needs no slot).
pub(super) fn default_value_node(value: &DefaultValue, slot: Option<&TsType>) -> TsExpr {
    let entries = |fields: &[(String, DefaultValue)], within: Option<TsType>| {
        fields
            .iter()
            .map(|(key, v)| {
                let at = within
                    .as_ref()
                    .map(|t| index(t, TsType::StringLiteral(key.clone())));
                TsObjectEntry::prop(key.clone(), default_value_node(v, at.as_ref()))
            })
            .collect()
    };
    let variant = |tag: &str| slot.map(|t| extract(t, tag));
    match value {
        DefaultValue::Num(digits) => TsExpr::Lit(TsLit::Num(digits.clone())),
        DefaultValue::Str(text) => TsExpr::Lit(TsLit::Str(text.clone())),
        DefaultValue::Bool(b) => TsExpr::Lit(TsLit::Bool(*b)),
        // #3801: the in-memory decimal, the node a `Decimal` literal lowers to.
        DefaultValue::Decimal { coefficient, scale } => {
            super::lower::decimal_lit(coefficient, *scale)
        }
        DefaultValue::Refined { value, .. } => {
            let base = default_value_node(value, None);
            match slot {
                Some(ty) => TsExpr::Paren(Box::new(TsExpr::As {
                    expr: Box::new(base),
                    ty: ty.clone(),
                })),
                None => base,
            }
        }
        DefaultValue::Record(fields) => object(entries(fields, slot.cloned())),
        DefaultValue::Sum { variant: v, fields } => tagged(v, entries(fields, variant(v))),
        DefaultValue::Some(v) => payload("Some", "value", v, variant("Some")),
        DefaultValue::None => tagged("None", vec![]),
        DefaultValue::Ok(v) => payload("Ok", "value", v, variant("Ok")),
        DefaultValue::Err(v) => payload("Err", "error", v, variant("Err")),
        DefaultValue::List(items) => {
            let at = slot.map(|t| index(t, TsType::named("number")));
            TsExpr::Array {
                items: items
                    .iter()
                    .map(|v| default_value_node(v, at.as_ref()))
                    .collect(),
                multiline: false,
            }
        }
        DefaultValue::EmptyMap => empty_collection("Map"),
        DefaultValue::EmptySet => empty_collection("Set"),
    }
}

/// Whether `ty` is `any`, whose every part is `any` again.
fn is_any(ty: &TsType) -> bool {
    matches!(ty, TsType::Named { name, type_args } if name == "any" && type_args.is_empty())
}

/// `T[K]`.
fn index(ty: &TsType, key: TsType) -> TsType {
    if is_any(ty) {
        return ty.clone();
    }
    TsType::Indexed {
        object: Box::new(ty.clone()),
        index: Box::new(key),
    }
}

/// `Extract<T, { tag: "<variant>" }>`: the member of a tagged union that is `variant`.
fn extract(ty: &TsType, variant: &str) -> TsType {
    if is_any(ty) {
        return ty.clone();
    }
    let tag = TsType::Object(vec![TsTypeMember::prop(
        "tag",
        TsType::StringLiteral(variant.to_string()),
    )]);
    TsType::named_with_args("Extract", vec![ty.clone(), tag])
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

/// `{ tag: "<variant>" as const, <key>: <value> }`, the value going into `within[key]`.
fn payload(variant: &str, key: &str, value: &DefaultValue, within: Option<TsType>) -> TsExpr {
    let at = within.map(|t| index(&t, TsType::StringLiteral(key.to_string())));
    tagged(
        variant,
        vec![TsObjectEntry::prop(
            key.to_string(),
            default_value_node(value, at.as_ref()),
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
    fn print(value: &DefaultValue) -> String {
        print_expr(&default_value_node(value, None))
    }

    /// Every case: the tree, and the text the deleted `lower_field_default_value` printed for it
    /// (captured before it was deleted; `bynk-check`'s tests pair the same text with the same trees),
    /// apart from the `Decimal` rows (#3801), which are the corrected text. With no slot, a refined
    /// value prints as its base value (#3759).
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
            // #3759: a refined value is its base value; the caller brands the whole field.
            (refined("Age", num("3")), "3"),
            (DefaultValue::List(vec![refined("Age", num("1"))]), "[1]"),
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

    /// #3759: each refined leaf is cast to its own position's type, indexed from the slot, so the
    /// refined type itself is never named.
    #[test]
    fn a_refined_leaf_is_cast_to_its_position_type() {
        let slot = TsType::Indexed {
            object: Box::new(TsType::named("Review")),
            index: Box::new(TsType::StringLiteral("f".to_string())),
        };
        let r = |n: &str| refined("Rating", num(n));
        let sum = DefaultValue::Sum {
            variant: "Rated".to_string(),
            fields: vec![("score".to_string(), r("2"))],
        };
        for (value, want) in [
            (r("5"), "(5 as Review[\"f\"])"),
            (
                DefaultValue::List(vec![r("3")]),
                "[(3 as Review[\"f\"][number])]",
            ),
            (
                some(r("5")),
                "{ tag: \"Some\" as const, value: (5 as Extract<Review[\"f\"], { tag: \"Some\" }>[\"value\"]) }",
            ),
            (
                DefaultValue::Err(Box::new(r("1"))),
                "{ tag: \"Err\" as const, error: (1 as Extract<Review[\"f\"], { tag: \"Err\" }>[\"error\"]) }",
            ),
            (
                sum,
                "{ tag: \"Rated\" as const, score: (2 as Extract<Review[\"f\"], { tag: \"Rated\" }>[\"score\"]) }",
            ),
            (
                DefaultValue::Record(vec![("s".to_string(), r("4")), ("n".to_string(), num("1"))]),
                "{ s: (4 as Review[\"f\"][\"s\"]), n: 1 }",
            ),
        ] {
            assert_eq!(print_expr(&default_value_node(&value, Some(&slot))), want);
        }
    }

    /// #3759: in test scaffolding the slot is `any`, and every part of `any` is `any`.
    #[test]
    fn an_any_slot_stays_any() {
        let value = DefaultValue::List(vec![some(refined("Rating", num("3")))]);
        assert_eq!(
            print_expr(&default_value_node(&value, Some(&TsType::named("any")))),
            "[{ tag: \"Some\" as const, value: (3 as any) }]"
        );
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
