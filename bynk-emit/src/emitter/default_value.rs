//! A record field's declared default, as a TypeScript node (back-half paydown 1A.31, #3732).
//!
//! `bynk-check` decides what a default is (`field_default_value`, type-directed) and returns it
//! as plain data; this turns that data into the in-memory value, so a record literal that omits
//! a defaulted field stays a node. Each tag is `as const`, so an unannotated `const x = { … }`
//! in a test body does not widen `tag` to `string` (#3404). The value is structural, so it needs
//! no import into the module that builds the record.

use bynk_check::wire_default::DefaultValue;
use bynk_ts::{TsExpr, TsLit, TsObjectEntry, TsType, TsTypeMember};

/// Where a default value goes, for a refined value in it (#3759).
#[derive(Clone)]
pub(super) enum Slot {
    /// A position of this TS type: a refined value is cast to it, `(5 as Review["rating"])`, so
    /// it is branded without naming the refined type, which the module that builds the record may
    /// not import. Each part's type is indexed from it: a field by its key, a list element by
    /// `number`, a variant's payload through `Extract<…, { tag: "V" }>`.
    Typed(TsType),
    /// Test scaffolding, where a declared type is not nameable: a refined value is branded as
    /// `unchecked_construct_test_node` brands a refined literal there.
    Scaffold,
    /// No brand (a value with no refined part needs none).
    Unbranded,
}

impl Slot {
    /// The slot of a part of this one: `T[K]`.
    fn index(&self, key: TsType) -> Slot {
        match self {
            Slot::Typed(ty) => Slot::Typed(TsType::Indexed {
                object: Box::new(ty.clone()),
                index: Box::new(key),
            }),
            other @ Slot::Scaffold | other @ Slot::Unbranded => other.clone(),
        }
    }

    /// The slot of a field `key` of this one.
    fn field(&self, key: &str) -> Slot {
        self.index(TsType::StringLiteral(key.to_string()))
    }

    /// The slot of the `variant` member of this tagged union: `Extract<T, { tag: "<variant>" }>`.
    fn variant(&self, variant: &str) -> Slot {
        match self {
            Slot::Typed(ty) => {
                let tag = TsType::Object(vec![TsTypeMember::prop(
                    "tag",
                    TsType::StringLiteral(variant.to_string()),
                )]);
                Slot::Typed(TsType::named_with_args("Extract", vec![ty.clone(), tag]))
            }
            other @ Slot::Scaffold | other @ Slot::Unbranded => other.clone(),
        }
    }
}

/// The node for a default value going into `slot`.
pub(super) fn default_value_node(value: &DefaultValue, slot: &Slot) -> TsExpr {
    let entries = |fields: &[(String, DefaultValue)], within: &Slot| {
        fields
            .iter()
            .map(|(key, v)| {
                TsObjectEntry::prop(key.clone(), default_value_node(v, &within.field(key)))
            })
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
        // #3759, #3842: a refined or an opaque value, branded as its slot is. An opaque type's TS
        // type is a brand alias like a refined one (ADR 0182), so the same cast brands it where the
        // type is not nameable; test scaffolding has no declared type to cast to, so it brands an
        // opaque value through `Name.unsafe(…)` and a refined one with a cast.
        DefaultValue::Branded {
            name,
            value,
            opaque,
        } => {
            let base = default_value_node(value, &Slot::Unbranded);
            match slot {
                Slot::Typed(ty) => TsExpr::Paren(Box::new(TsExpr::As {
                    expr: Box::new(base),
                    ty: ty.clone(),
                })),
                Slot::Scaffold => {
                    super::ts_types::unchecked_construct_test_node(name, base, *opaque)
                }
                Slot::Unbranded => base,
            }
        }
        DefaultValue::Record(fields) => TsExpr::object_entries(entries(fields, slot)),
        DefaultValue::Sum { variant, fields } => {
            tagged(variant, entries(fields, &slot.variant(variant)))
        }
        DefaultValue::Some(v) => payload("Some", "value", v, slot),
        DefaultValue::None => tagged("None", vec![]),
        DefaultValue::Ok(v) => payload("Ok", "value", v, slot),
        DefaultValue::Err(v) => payload("Err", "error", v, slot),
        DefaultValue::List(items) => {
            let at = slot.index(TsType::named("number"));
            TsExpr::Array {
                items: items.iter().map(|v| default_value_node(v, &at)).collect(),
                multiline: false,
            }
        }
        DefaultValue::EmptyMap => empty_collection("Map"),
        DefaultValue::EmptySet => empty_collection("Set"),
    }
}

/// The node for a default value in its **wire** form: the JSON a real wire value of the field
/// would be, which the field's codec decodes when the key is absent (events slice 3a, #972;
/// 1A.40b, #3996). A pure function of the tree:
/// - a base value is its literal, a `Decimal` its canonical text as a string (#2622), and a refined
///   value its base value;
/// - a sum is `{ kind: "V", f: .. }`, an `Option` is `{ kind: "Some", value }` or
///   `{ kind: "None" }`, and a `Result` is `{ kind: "Ok", value }` or `{ kind: "Err", error }`;
/// - a list is `[..]`, an empty `Map` or `Set` is `[]` (both travel as an array), and a record is a
///   plain object.
///
/// Unlike the in-memory form ([`default_value_node`]), the wire form carries `kind`, not `tag`,
/// needs no `as const`, and names no collection class.
pub(super) fn wire_default_node(value: &DefaultValue) -> TsExpr {
    let entries = |fields: &[(String, DefaultValue)]| -> Vec<TsObjectEntry> {
        fields
            .iter()
            .map(|(key, v)| TsObjectEntry::prop(key.clone(), wire_default_node(v)))
            .collect()
    };
    match value {
        DefaultValue::Num(digits) => TsExpr::Lit(TsLit::Num(digits.clone())),
        DefaultValue::Str(text) => TsExpr::Lit(TsLit::Str(text.clone())),
        DefaultValue::Bool(b) => TsExpr::Lit(TsLit::Bool(*b)),
        DefaultValue::Decimal { coefficient, scale } => TsExpr::Lit(TsLit::Str(
            bynk_syntax::decimal::render(coefficient, *scale),
        )),
        DefaultValue::Branded { value, .. } => wire_default_node(value),
        DefaultValue::Record(fields) => TsExpr::object_entries(entries(fields)),
        DefaultValue::Sum { variant, fields } => wire_kind(variant, entries(fields)),
        DefaultValue::Some(v) => wire_kind("Some", vec![wire_prop("value", v)]),
        DefaultValue::None => wire_kind("None", vec![]),
        DefaultValue::Ok(v) => wire_kind("Ok", vec![wire_prop("value", v)]),
        DefaultValue::Err(v) => wire_kind("Err", vec![wire_prop("error", v)]),
        DefaultValue::List(items) => TsExpr::Array {
            items: items.iter().map(wire_default_node).collect(),
            multiline: false,
        },
        DefaultValue::EmptyMap | DefaultValue::EmptySet => TsExpr::Array {
            items: vec![],
            multiline: false,
        },
    }
}

/// `{ kind: "<variant>", …rest }`.
fn wire_kind(variant: &str, rest: Vec<TsObjectEntry>) -> TsExpr {
    let mut entries = vec![TsObjectEntry::prop(
        "kind".to_string(),
        TsExpr::Lit(TsLit::Str(variant.to_string())),
    )];
    entries.extend(rest);
    TsExpr::object_entries(entries)
}

/// `<key>: <wire form of value>`.
fn wire_prop(key: &str, value: &DefaultValue) -> TsObjectEntry {
    TsObjectEntry::prop(key.to_string(), wire_default_node(value))
}

/// `{ tag: "<variant>" as const, …rest }`.
fn tagged(variant: &str, rest: Vec<TsObjectEntry>) -> TsExpr {
    let tag = TsExpr::AsConst(Box::new(TsExpr::Lit(TsLit::Str(variant.to_string()))));
    let mut entries = vec![TsObjectEntry::prop("tag".to_string(), tag)];
    entries.extend(rest);
    TsExpr::object_entries(entries)
}

/// `{ tag: "<variant>" as const, <key>: <value> }`, the value going into the variant's `key`.
fn payload(variant: &str, key: &str, value: &DefaultValue, slot: &Slot) -> TsExpr {
    let at = slot.variant(variant).field(key);
    tagged(
        variant,
        vec![TsObjectEntry::prop(
            key.to_string(),
            default_value_node(value, &at),
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
        DefaultValue::Branded {
            name: name.to_string(),
            value: Box::new(v),
            opaque: false,
        }
    }
    fn opaque(name: &str, v: DefaultValue) -> DefaultValue {
        DefaultValue::Branded {
            name: name.to_string(),
            value: Box::new(v),
            opaque: true,
        }
    }
    fn print(value: &DefaultValue) -> String {
        print_expr(&default_value_node(value, &Slot::Unbranded))
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
            // #3842: so is an opaque value.
            (opaque("Tok", text("k")), "\"k\""),
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
            assert_eq!(
                print_expr(&default_value_node(&value, &Slot::Typed(slot.clone()))),
                want
            );
        }
    }

    /// #3759: in test scaffolding each refined leaf is branded as a refined literal is there.
    #[test]
    fn a_scaffold_slot_brands_each_leaf_as_a_test_literal() {
        let value = DefaultValue::List(vec![some(refined("Rating", num("3")))]);
        assert_eq!(
            print_expr(&default_value_node(&value, &Slot::Scaffold)),
            "[{ tag: \"Some\" as const, value: (3 as any) }]"
        );
    }

    /// #3842: an opaque leaf is branded as its field is, by the same cast where the type is not
    /// nameable (an opaque type's TS type is a brand alias like a refined one), and in test
    /// scaffolding as an opaque test literal is there, through `Name.unsafe(…)`.
    #[test]
    fn an_opaque_leaf_is_branded_like_a_refined_one() {
        let slot = TsType::Indexed {
            object: Box::new(TsType::named("Holder")),
            index: Box::new(TsType::StringLiteral("tok".to_string())),
        };
        let tok = opaque("Tok", text("k"));
        assert_eq!(
            print_expr(&default_value_node(&tok, &Slot::Typed(slot))),
            "(\"k\" as Holder[\"tok\"])"
        );
        assert_eq!(
            print_expr(&default_value_node(
                &DefaultValue::List(vec![some(tok.clone())]),
                &Slot::Scaffold
            )),
            "[{ tag: \"Some\" as const, value: Tok.unsafe(\"k\") }]"
        );
        // A refined leaf beside it keeps its cast, which a refined type has no `unsafe` for.
        assert_eq!(
            print_expr(&default_value_node(
                &refined("Age", num("3")),
                &Slot::Scaffold
            )),
            "(3 as any)"
        );
        // And the wire form is the base value, as a refined one's is.
        assert_eq!(print_expr(&wire_default_node(&tok)), "\"k\"");
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

    fn wire(value: &DefaultValue) -> TsExpr {
        wire_default_node(value)
    }

    fn wire_text(value: &DefaultValue) -> String {
        print_expr(&wire(value))
    }

    /// 1A.40b: the wire form of every shape, as a tree and as the text it prints (the text the
    /// deleted `bynk-check` renderer produced; the goldens carry it).
    #[test]
    fn the_wire_form_of_every_shape() {
        let sum = |v: &str, f: Vec<(&str, DefaultValue)>| DefaultValue::Sum {
            variant: v.to_string(),
            fields: f.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        };
        let record = |f: Vec<(&str, DefaultValue)>| {
            DefaultValue::Record(f.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
        };
        let ok = |v| DefaultValue::Ok(Box::new(v));
        let err = |v| DefaultValue::Err(Box::new(v));
        for (value, text) in [
            (num("5"), "5"),
            (num("-5"), "-5"),
            (num("1e3"), "1e3"),
            (DefaultValue::Bool(true), "true"),
            (DefaultValue::Bool(false), "false"),
            (text("x"), "\"x\""),
            (text("a\"b\n"), "\"a\\\"b\\n\""),
            // A `Decimal` is its canonical text on the wire (#2622), not `{ coefficient, scale }`.
            (dec("-150", 2), "\"-1.50\""),
            (dec("000", 2), "\"0.00\""),
            // A refined value is its base value.
            (refined("Age", num("3")), "3"),
            (DefaultValue::None, "{ kind: \"None\" }"),
            (some(num("1")), "{ kind: \"Some\", value: 1 }"),
            (
                some(some(DefaultValue::None)),
                "{ kind: \"Some\", value: { kind: \"Some\", value: { kind: \"None\" } } }",
            ),
            (ok(num("1")), "{ kind: \"Ok\", value: 1 }"),
            (err(text("e")), "{ kind: \"Err\", error: \"e\" }"),
            (DefaultValue::List(vec![]), "[]"),
            (DefaultValue::List(vec![num("1"), num("2")]), "[1, 2]"),
            // An empty `Map` or `Set` travels as an empty array.
            (DefaultValue::EmptyMap, "[]"),
            (DefaultValue::EmptySet, "[]"),
            (
                record(vec![("a", num("1")), ("b", text("x"))]),
                "{ a: 1, b: \"x\" }",
            ),
            (sum("Nil", vec![]), "{ kind: \"Nil\" }"),
            (
                sum("Has", vec![("value", text("s"))]),
                "{ kind: \"Has\", value: \"s\" }",
            ),
        ] {
            assert_eq!(wire_text(&value), text, "{value:?}");
        }
    }

    /// The wire form is a node, so it compares as one: `kind` (not `tag`), no `as const`.
    #[test]
    fn the_wire_form_is_a_node_without_tag_or_as_const() {
        assert_eq!(
            wire(&DefaultValue::None),
            TsExpr::object_entries(vec![TsObjectEntry::prop(
                "kind".to_string(),
                TsExpr::Lit(TsLit::Str("None".to_string())),
            )])
        );
        assert_ne!(
            wire(&DefaultValue::None),
            default_value_node(&DefaultValue::None, &Slot::Unbranded)
        );
        assert_eq!(
            wire(&DefaultValue::EmptyMap),
            TsExpr::Array {
                items: vec![],
                multiline: false
            }
        );
    }

    /// A zero-field record prints `{}`; the text it replaced printed `{  }`. No fixture has one.
    #[test]
    fn an_empty_record_default_prints_as_braces_on_the_wire() {
        assert_eq!(wire_text(&DefaultValue::Record(vec![])), "{}");
    }
}
