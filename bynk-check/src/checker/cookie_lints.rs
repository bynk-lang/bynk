//! #3446: what the checker knows about a cookie written to a response, beyond the safety rules of
//! `lint_cookie_literal`: that a typed write names the same cookie its record reads, that an untyped
//! literal agrees with the record that reads it when one can be proved, and that the one cookie only
//! `Actor.issue`/`Actor.expire` may write is not written by hand.
//!
//! Everything here is judged at compile time from a cookie whose name is known: a `Cookie { name:
//! "…" }` literal, `Cookie { ...R.f.cookie(v), … }` (a typed cookie with attributes) or a bare
//! `R.f.cookie(v)` / `R.f.expire()`. A computed name is left to the runtime backstop.

use bynk_syntax::ast::{BaseType, Expr, ExprKind, FieldInit, RecordField, TypeBody, TypeRef};
use bynk_syntax::error::{Applicability, CompileError};

use super::{Callee, Ctx};

/// A cookie expression with a compile-time name.
pub(super) struct CookieShape<'a> {
    /// The cookie's name.
    pub name: Option<String>,
    /// The record and field when the name comes from a typed write.
    pub typed: Option<(String, String)>,
    /// The attribute fields written explicitly (the literal's fields, or the spread's overrides).
    pub fields: &'a [FieldInit],
    /// Whether the cookie is an untyped `Cookie { name, value, … }` literal.
    pub untyped_literal: bool,
}

impl<'a> CookieShape<'a> {
    /// The shape of `arg`, or `None` when it is not a cookie expression whose name is known.
    pub(super) fn of(arg: &'a Expr, ctx: &Ctx) -> Option<CookieShape<'a>> {
        let typed_of = |e: &Expr| match ctx.callees.get(&e.id) {
            Some(Callee::CookieField { record, field, .. }) => {
                let wire = wire_name_of(record, field, ctx)?;
                Some((wire, (record.clone(), field.clone())))
            }
            _ => None,
        };
        match &arg.kind {
            ExprKind::RecordConstruction {
                type_name, fields, ..
            } if type_name.name == "Cookie" => {
                let name = fields
                    .iter()
                    .find(|f| f.name.name == "name")
                    .and_then(|f| f.value.as_ref())
                    .and_then(|e| match &e.kind {
                        ExprKind::StrLit(s) => Some(s.clone()),
                        _ => None,
                    });
                Some(CookieShape {
                    name,
                    typed: None,
                    fields,
                    untyped_literal: true,
                })
            }
            ExprKind::RecordSpread {
                type_name,
                base,
                overrides,
                ..
            } if type_name.as_ref().is_none_or(|t| t.name == "Cookie") => {
                let (name, typed) = typed_of(base)?;
                Some(CookieShape {
                    name: Some(name),
                    typed: Some(typed),
                    fields: overrides,
                    untyped_literal: false,
                })
            }
            ExprKind::MethodCall { .. } => {
                let (name, typed) = typed_of(arg)?;
                Some(CookieShape {
                    name: Some(name),
                    typed: Some(typed),
                    fields: &[],
                    untyped_literal: false,
                })
            }
            _ => None,
        }
    }
}

/// The wire name of field `field` of record `record`: its `@name`, else its own name.
fn wire_name_of(record: &str, field: &str, ctx: &Ctx) -> Option<String> {
    let TypeBody::Record(body) = &ctx.input.types.get(record)?.body else {
        return None;
    };
    let f = body.fields.iter().find(|f| f.name.name == field)?;
    Some(f.wire_name().unwrap_or_else(|| f.name.name.clone()))
}

/// How the `cookies` decoder reads one field's text (`fieldsObject`'s leaf rules), as far as a
/// literal can be judged against it at compile time.
enum Leaf {
    Int,
    Bool,
    /// The names a variant of the enum can be read as (unit variants).
    Enum(Vec<String>),
    /// A plain `String`: any text.
    PlainString,
    /// Refined, opaque, `Decimal`: the codec judges the text; not judged here.
    Other,
}

fn leaf_of(f: &RecordField, ctx: &Ctx) -> Leaf {
    let ty = match &f.type_ref {
        TypeRef::Option(inner, _) => inner.as_ref(),
        other => other,
    };
    let of_base = |b: &BaseType, plain: bool| match b {
        BaseType::Int => Leaf::Int,
        BaseType::Bool => Leaf::Bool,
        BaseType::String if plain => Leaf::PlainString,
        _ => Leaf::Other,
    };
    match ty {
        TypeRef::Base(b, _) => of_base(b, true),
        TypeRef::Named(id) => match ctx.input.types.get(&id.name).map(|t| &t.body) {
            Some(TypeBody::Refined {
                base, refinement, ..
            }) => of_base(base, refinement.is_none()),
            Some(TypeBody::Opaque { base, .. }) => of_base(base, false),
            Some(TypeBody::Sum(sum)) => Leaf::Enum(
                sum.variants
                    .iter()
                    .filter(|v| v.payload.is_empty())
                    .map(|v| v.name.name.clone())
                    .collect(),
            ),
            _ => Leaf::Other,
        },
        _ => Leaf::Other,
    }
}

/// Whether `text` is a value the decoder reads for a field of this leaf kind.
fn decodes(leaf: &Leaf, text: &str) -> bool {
    match leaf {
        Leaf::Int => {
            let digits = text.strip_prefix('-').unwrap_or(text);
            !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit())
                && text
                    .parse::<i64>()
                    .is_ok_and(|n| n.unsigned_abs() < (1 << 53))
        }
        Leaf::Bool => text == "true" || text == "false",
        Leaf::Enum(variants) => variants.iter().any(|v| v == text),
        Leaf::PlainString | Leaf::Other => true,
    }
}

/// The typed call that writes `text` for a field of this leaf kind, when that is provably the same
/// cookie: the argument source, or `None` when it cannot be said (a refined, opaque or `Decimal`
/// field, a text the decoder would not read).
fn typed_argument(leaf: &Leaf, text: &str) -> Option<String> {
    match leaf {
        Leaf::Int | Leaf::Bool | Leaf::Enum(_) if decodes(leaf, text) => Some(text.to_string()),
        Leaf::PlainString => Some(format!(
            "\"{}\"",
            text.replace('\\', "\\\\").replace('"', "\\\"")
        )),
        _ => None,
    }
}

/// The cookie record and field an untyped name is read by in this unit, if there is one.
fn reading_field<'a>(name: &str, ctx: &Ctx<'a>) -> Option<(String, &'a RecordField)> {
    let mut records: Vec<&String> = ctx.input.cross_context.cookie_records.iter().collect();
    records.sort();
    records.into_iter().find_map(|r| {
        let TypeBody::Record(body) = &ctx.input.types.get(r)?.body else {
            return None;
        };
        body.fields
            .iter()
            .find(|f| f.wire_name().unwrap_or_else(|| f.name.name.clone()) == name)
            .map(|f| (r.clone(), f))
    })
}

/// The compile-time checks on a cookie's relation to the records and actors of its unit.
pub(super) fn check(shape: &CookieShape, arg: &Expr, ctx: &mut Ctx) {
    let Some(name) = shape.name.as_deref() else {
        return;
    };
    // The `SignedCookie` cookie is minted and cleared by its actor alone, with its signing key, its
    // expiry and its CSRF seam: a hand-built or record-built cookie of that name would bypass them.
    if ctx.input.cross_context.signed_cookie_names.contains(name) {
        let (code, what) = if shape.typed.is_some() {
            (
                "bynk.actor.signed_cookie_write_directly",
                "a cookie record's field",
            )
        } else {
            (
                "bynk.actor.signed_cookie_write_literal",
                "a `Cookie` literal",
            )
        };
        ctx.errors.push(
            CompileError::new(
                code,
                arg.span,
                format!("`{name}` is the session cookie of a `SignedCookie` actor, which {what} may not write"),
            )
            .with_note(
                "only the actor's `issue(identity)` and `expire()` write it, so the credential is signed with the actor's key, expires, and sits under its CSRF seam",
            ),
        );
        return;
    }
    if !shape.untyped_literal {
        return;
    }
    let Some((record, field)) = reading_field(name, ctx) else {
        return;
    };
    let leaf = leaf_of(field, ctx);
    let value = shape
        .fields
        .iter()
        .find(|f| f.name.name == "value")
        .and_then(|f| f.value.as_ref());
    let literal = value.and_then(|v| match &v.kind {
        ExprKind::StrLit(s) => Some((s.as_str(), v.span)),
        _ => None,
    });
    if let Some((text, span)) = literal
        && !decodes(&leaf, text)
    {
        ctx.errors.push(
            CompileError::new(
                "bynk.http.cookie_value_undecodable",
                span,
                format!(
                    "`{text}` is not a value `{record}.{}` reads: the next request's `cookies` decoder would answer `400`",
                    field.name.name
                ),
            )
            .with_note(match &leaf {
                Leaf::Enum(variants) => format!(
                    "an enum cookie is its variant's exact name: {}",
                    variants.join(", ")
                ),
                Leaf::Int => "an `Int` cookie is its decimal digits".to_string(),
                _ => "a `Bool` cookie is `true` or `false`".to_string(),
            })
            .with_note(format!(
                "write it with `{record}.{}.cookie(…)`, which checks the value against the field",
                field.name.name
            )),
        );
        return;
    }
    let call = format!("{record}.{}", field.name.name);
    let mut warning = CompileError::new(
        "bynk.http.cookie_untyped_has_field",
        arg.span,
        format!("this cookie is read by `{call}`; write it with `{call}.cookie(…)`"),
    )
    .with_note(
        "the typed write is named by the field's wire name and valued as its decoder reads it, so a rename or a changed type cannot leave the two disagreeing",
    );
    // The rewrite is offered only when the literal is just a name and a value, and the value
    // provably decodes to the same text.
    let bare = shape
        .fields
        .iter()
        .all(|f| f.name.name == "name" || f.name.name == "value");
    if bare
        && let Some((text, _)) = literal
        && let Some(argument) = typed_argument(&leaf, text)
    {
        warning = warning.with_suggestion(
            format!("write it as `{call}.cookie({argument})`"),
            vec![(arg.span, format!("{call}.cookie({argument})"))],
            Applicability::MachineApplicable,
        );
    } else {
        warning = warning.with_note(format!(
            "to keep its attributes, spread it: `Cookie {{ ...{call}.cookie(…), maxAge: … }}`"
        ));
    }
    ctx.errors.push(warning);
}
