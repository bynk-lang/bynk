//! Literals and records (back-half paydown 1A.2, 1A.7, 2.2d, #4087): scalar and interpolated-string
//! literals, record construction and record spread. Moved out of `lower.rs` byte for byte.

use bynk_syntax::ast::{BaseType, Expr, FieldInit, Ident, InterpPart, RefinableLiteral, TypeBody};

use super::super::default_value::{Slot, default_value_node};
use super::super::lower_ctx::LowerCtx;
use super::super::ts_types::ts_ty_to_ts_type;
use super::super::{decimal_signed_coef, ts_ident};
use super::{Lowered, Pre};
use crate::ts_build::{ident, num_lit, str_lit};

/// A compile-time literal that v0.9.4 may admit as a refined type (int, float,
/// decimal or string), as a node (1A.17, Decision D). `None` for anything else.
///
/// A negated numeric literal is admissible at compile time (the checker folds the
/// sign in `const_literal`), so the lowering must route it through `unsafe` like
/// any other admitted literal; [`ExprKind::refinable_literal`] reads it as one.
pub(super) fn lower_const_literal_node(e: &Expr) -> Option<bynk_ts::TsExpr> {
    let sign = |negated: bool| if negated { "-" } else { "" };
    Some(match e.kind.refinable_literal()? {
        RefinableLiteral::Int { value, negated } => num_lit(format!("{}{value}", sign(negated))),
        // v0.21: the stored lexeme verbatim.
        RefinableLiteral::Float { lexeme, negated } => {
            num_lit(format!("{}{lexeme}", sign(negated)))
        }
        RefinableLiteral::Decimal {
            coef,
            scale,
            negated,
        } => decimal_lit(&format!("{}{coef}", sign(negated)), scale),
        RefinableLiteral::Duration { millis } => num_lit(millis.to_string()),
        RefinableLiteral::Str(s) => str_lit(s),
    })
}

/// A literal's data, destructured from its `ExprKind` by `lower_expr`'s own
/// arms, so [`Lit::lower`] matches an exhaustive type with no catch-all.
pub(super) enum Lit<'a> {
    /// The integer's digits.
    Int(String),
    /// The stored lexeme, verbatim.
    Float(&'a str),
    Decimal(&'a str, u32),
    /// The duration's resolved milliseconds (v0.86, ADR 0112): the value `Duration` erases to.
    Duration(String),
    Str(&'a str),
    Bool(bool),
    Unit,
    None,
}

/// Slice 1A.2 (#3694): the literal family lowers to typed nodes ([`Lit::lower`]). The string's
/// escaping is `TsLit::Str`'s (the one shared escaper); numbers print their
/// digits verbatim; `None` and the unit value are the runtime's `None` /
/// `undefined`.
impl Lit<'_> {
    pub(super) fn lower(self) -> Lowered {
        use bynk_ts::{TsExpr, TsLit};
        Lowered::node(match self {
            Lit::Int(digits) | Lit::Duration(digits) => num_lit(digits),
            // v0.21: the stored lexeme verbatim — `1e10` must not normalise.
            Lit::Float(lexeme) => num_lit(lexeme.to_string()),
            // #2622: a `Decimal` literal is an object literal (no runtime import).
            Lit::Decimal(coef, scale) => decimal_lit(coef, scale),
            Lit::Str(s) => TsExpr::Lit(TsLit::Str(s.to_string())),
            Lit::Bool(b) => TsExpr::Lit(TsLit::Bool(b)),
            Lit::None => TsExpr::Ident("None".to_string()),
            Lit::Unit => TsExpr::Ident("undefined".to_string()),
        })
    }
}

/// #2622: a `Decimal` literal as an object-literal node — the node twin of
/// [`decimal_ts_literal`](crate::emitter::ts_types::decimal_ts_literal)
/// (`{ coefficient: <signed>n, scale: <scale> }`, no runtime import).
pub(in crate::emitter) fn decimal_lit(coef: &str, scale: u32) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::object(vec![
        (
            "coefficient".to_string(),
            bynk_ts::TsExpr::bigint(decimal_signed_coef(coef)),
        ),
        ("scale".to_string(), num_lit(scale.to_string())),
    ])
}

/// Lower an interpolated string (v0.43, ADR 0075) to a TS template literal.
/// Chunks become escaped literal text; each hole becomes `${String(<expr>)}`.
/// `String(…)` is identity for a `String` hole and the display form for
/// `Int`/`Float`/`Bool` — and the checker guarantees only base scalars reach
/// here, so no `[object Object]` can be emitted.
pub(super) fn lower_interp_str(parts: &[InterpPart], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    // `chunks` are the already-escaped literal parts (`escape_ts_template`, which
    // `TemplateLit` prints verbatim): always one more than there are holes, with
    // an empty chunk between adjacent holes.
    let mut chunks: Vec<String> = vec![String::new()];
    let mut holes: Vec<bynk_ts::TsExpr> = Vec::new();
    let mut wrappers: Vec<&str> = Vec::new();
    for part in parts {
        match part {
            InterpPart::Chunk(text) => {
                if let Some(last) = chunks.last_mut() {
                    last.push_str(&escape_ts_template(text));
                }
            }
            InterpPart::Hole(hole) => {
                let lowered = pre.lower_node(hole, cx);
                // #3976: a `DecodeErrorKind` is a `{ tag }` value; it prints as its tag.
                let is_kind = cx.commons().expr_types.get(&hole.id).is_some_and(|te| {
                    matches!(&*cx.commons().tys().get(te.ty),
                        bynk_check::checker::Ty::Named { name, .. } if name == "DecodeErrorKind")
                });
                holes.push(if is_kind {
                    bynk_ts::TsExpr::Member {
                        object: Box::new(lowered),
                        property: "tag".to_string(),
                    }
                } else {
                    lowered
                });
                // #2622: a `Decimal` prints as its canonical text, not as the
                // host's `[object Object]`. #3977: an `Instant` and a `Duration` print as
                // their `toString()` does.
                let base = cx
                    .commons()
                    .expr_types
                    .get(&hole.id)
                    .and_then(|te| te.ty.base(cx.commons().tys()));
                if base == Some(BaseType::Decimal) {
                    cx.note_decimal("__bynkDecimalToString");
                }
                wrappers.push(match base {
                    Some(BaseType::Decimal) => "__bynkDecimalToString",
                    Some(BaseType::Instant) => "__bynkInstantToIso",
                    Some(BaseType::Duration) => "__bynkDurationToString",
                    _ => "String",
                });
                chunks.push(String::new());
            }
        }
    }
    let built = {
        let nodes = holes;
        let exprs = nodes
            .into_iter()
            .zip(&wrappers)
            .map(|(hole, w)| bynk_ts::TsExpr::Call {
                callee: Box::new(ident(*w)),
                args: vec![hole],
            })
            .collect();
        bynk_ts::TsExpr::template_lit(chunks.clone(), exprs)
    };
    pre.finish(built)
}

/// Escape a literal chunk for a TS template-literal context: backslash,
/// backtick, and `$` (to neutralise `${`), plus the control-char escapes
/// [`escape_ts_string`] applies. (v0.43.)
pub(crate) fn escape_ts_template(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '`' => out.push_str("\\`"),
            '$' => out.push_str("\\$"),
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

/// How one object-literal field is written: `key: <child>` (taking the next
/// child), or the bare shorthand `key`.
enum FieldSlot {
    Prop(String),
    Shorthand(String),
}

/// The fields of a record literal as `(slot, child)` pairs, lowering each
/// value through `pre`. A shorthand `{ x }` references the binding `x`; if the
/// binding was renamed (a reserved word) it expands to `x: __id_x` — the key is
/// wire format and must keep the source spelling.
fn field_inits(
    fields: &[FieldInit],
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> (Vec<FieldSlot>, Vec<bynk_ts::TsExpr>) {
    let mut slots = Vec::new();
    let mut children = Vec::new();
    for f in fields {
        match &f.value {
            Some(v) => {
                children.push(pre.lower_node(v, cx));
                slots.push(FieldSlot::Prop(f.name.name.clone()));
            }
            None => {
                let v = ts_ident(&f.name.name);
                if v == f.name.name {
                    slots.push(FieldSlot::Shorthand(f.name.name.clone()));
                } else {
                    children.push(ident(&v));
                    slots.push(FieldSlot::Prop(f.name.name.clone()));
                }
            }
        }
    }
    (slots, children)
}

/// `{ k: v, … }` (or, with `spread`, `{ ...base, k: v, … }`): an `Object` node.
/// `children` holds the spread base first (when present), then
/// one child per [`FieldSlot::Prop`], in order.
fn object_built(
    has_spread: bool,
    slots: Vec<FieldSlot>,
    children: Vec<bynk_ts::TsExpr>,
) -> bynk_ts::TsExpr {
    let nodes = children;
    let mut nodes = nodes.into_iter();
    let mut entries = Vec::new();
    if has_spread && let Some(base) = nodes.next() {
        entries.push(bynk_ts::TsObjectEntry::Spread(base));
    }
    for slot in &slots {
        entries.push(match slot {
            FieldSlot::Prop(key) => match nodes.next() {
                Some(v) => bynk_ts::TsObjectEntry::prop(key.clone(), v),
                None => unreachable!("one child per `Prop` slot"),
            },
            FieldSlot::Shorthand(key) => bynk_ts::TsObjectEntry::Shorthand(key.clone()),
        });
    }
    bynk_ts::TsExpr::object_entries(entries)
}

/// #3759: the slot a defaulted field's value fills, `record[field]`, so a refined value in it is
/// cast to its position's type (`default_value_node`). Indexed from the record's type as the
/// checker typed this construction: the refined type may not be nameable here (the record can
/// come from a unit this one imports without its field types), but the record is, since this
/// literal names it. Test scaffolding names no declared type, so it brands as it brands a refined
/// literal.
fn default_slot(e: &Expr, type_name: &Ident, field: &str, cx: &LowerCtx) -> Slot {
    if cx.in_test_scaffold() {
        return Slot::Scaffold;
    }
    let record = match cx.commons().expr_types.get(&e.id) {
        Some(te) => ts_ty_to_ts_type(te.ty, cx.ty_render()),
        None => bynk_ts::TsType::named(cx.consumed_type_name(&type_name.name)),
    };
    Slot::Typed(bynk_ts::TsType::Indexed {
        object: Box::new(record),
        index: Box::new(bynk_ts::TsType::StringLiteral(field.to_string())),
    })
}

pub(super) fn lower_record_construction(
    e: &Expr,
    type_name: &Ident,
    fields: &[FieldInit],
    cx: &mut LowerCtx,
) -> Lowered {
    let mut pre = Pre::new();
    let (mut slots, mut children) = field_inits(fields, &mut pre, cx);
    // #2607 (Decision C): each declared field the literal leaves out takes its
    // default, written as a self-contained value (`field_default_value`, built as a node by
    // `default_value_node`) so it needs nothing imported into this module. The checker has
    // already required every field without a default, and rejected any default that has no
    // such value. A default is a node (1A.31, #3732), so a record that takes one stays one.
    // #4626: read in the declaring unit's scope where this unit shadows a type a default names.
    let types = cx.commons().default_scope(&type_name.name);
    if let Some(decl) = types.get(&type_name.name)
        && let TypeBody::Record(r) = &decl.body
    {
        for declared in &r.fields {
            let Some(init) = &declared.init else {
                continue;
            };
            if fields.iter().any(|f| f.name.name == declared.name.name) {
                continue;
            }
            match bynk_check::wire_default::field_default_value(init, &declared.type_ref, types) {
                Ok(v) => {
                    slots.push(FieldSlot::Prop(declared.name.name.clone()));
                    // #3759, #3842: a refined or opaque value in it is cast to its position's type.
                    let slot = match v.holds_branded() {
                        true => default_slot(e, type_name, &declared.name.name, cx),
                        false => Slot::Unbranded,
                    };
                    let node: bynk_ts::TsExpr = default_value_node(&v, &slot);
                    children.push(node);
                }
                // The checker rejected any default that has no value: a missing
                // fact (1A.34), reported at the construction.
                Err(_) => {
                    slots.push(FieldSlot::Prop(declared.name.name.clone()));
                    let node: bynk_ts::TsExpr = cx.missing_fact(
                        type_name.span,
                        format_args!("default value for field `{}`", declared.name.name),
                    );
                    children.push(node);
                }
            }
        }
    }
    let built = object_built(false, slots, children);
    pre.finish(built)
}

pub(super) fn lower_record_spread(
    base: &Expr,
    overrides: &[FieldInit],
    cx: &mut LowerCtx,
) -> Lowered {
    let mut pre = Pre::new();
    let mut children = vec![pre.lower_node(base, cx)];
    let (slots, field_children) = field_inits(overrides, &mut pre, cx);
    children.extend(field_children);
    let built = object_built(true, slots, children);
    pre.finish(built)
}
