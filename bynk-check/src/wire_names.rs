//! #3975: the wire names a type declares.
//!
//! `@name("…")` before a record field, a sum variant or a variant's payload field gives it the
//! key (or, for a variant, the discriminator value) it has on the wire; `@discriminator("…")`
//! before a sum's `type` names the key that carries the variant. They are part of the type's one
//! encoding, so they apply at every boundary. The Bynk-side names never change.
//!
//! This module holds the argument reading shared with the record-field checks in
//! `context_checks`, and the checks for sums.

use std::collections::HashMap;

use bynk_syntax::ast::{
    Annotation, AnnotationArg, BaseType, ConstDecl, ExprKind, RecordBody, SumBody, TypeBody,
    TypeDecl, TypeRef,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::context_checks::ts_type_ref_display;

/// The key a sum's variants travel under when no `@discriminator` is declared.
pub const DEFAULT_DISCRIMINATOR: &str = "kind";

/// A wire name read off an annotation: its text, where a diagnostic about it points (the
/// reference to a constant, when the author wrote one), and that argument.
pub(crate) struct NameArg<'a> {
    pub literal: String,
    pub at: Span,
    pub origin: Option<&'a AnnotationArg>,
}

/// The single string-literal argument of `ann` (`@name("…")`, `@discriminator("…")`), reporting
/// `bad_code` when it is anything else. A bare constant reference was resolved to its literal
/// upstream (`annotation_consts`).
pub(crate) fn string_arg<'a>(
    ann: &'a Annotation,
    consts: &HashMap<String, ConstDecl>,
    bad_code: &'static str,
    errors: &mut Vec<CompileError>,
) -> Option<NameArg<'a>> {
    let what = &ann.name.name;
    let arg = match ann.args.as_slice() {
        [a] if a.label.is_none() => Some(a),
        _ => None,
    };
    let literal = arg.and_then(|a| match &a.value.kind {
        ExprKind::StrLit(s) => Some(s.clone()),
        _ => None,
    });
    let Some(literal) = literal else {
        let mut err = CompileError::new(
            bad_code,
            ann.span,
            format!(
                "`@{what}` takes exactly one string literal, or a `String` constant bound to one"
            ),
        )
        .with_note(format!(
            "write `@{what}(\"__Host-sid\")`, or `const Key: String = \"__Host-sid\"` and `@{what}(Key)`"
        ));
        // #3445: a bare name that did not resolve (pass `annotation_consts`).
        if let Some(a) = arg
            && let ExprKind::Ident(id) = &a.value.kind
        {
            err = err.with_note(match consts.get(&id.name) {
                Some(c) if matches!(c.ty, TypeRef::Base(BaseType::String, _)) => format!(
                    "`{}` is declared `String`, but its value does not end at a string literal: every constant it passes through must be declared exactly `String`, and the last bound to a string literal",
                    id.name,
                ),
                Some(c) => format!(
                    "`{}` is declared `{}`; `@{what}` needs a constant declared exactly `String` and bound to a string literal (or to another such constant)",
                    id.name,
                    ts_type_ref_display(&c.ty),
                ),
                None => format!("`{}` is not a constant in scope", id.name),
            });
        }
        errors.push(err);
        return None;
    };
    // #3445: where a diagnostic about the value points: at the reference, where the author can
    // act, when there is one.
    let (at, origin) = match arg {
        Some(a) if a.from_const.is_some() => (a.value.span, Some(a)),
        _ => (ann.span, None),
    };
    Some(NameArg {
        literal,
        at,
        origin,
    })
}

/// A wire name that is not valid where it stands (`why` says what is wrong).
pub(crate) fn invalid_name(
    code: &'static str,
    annotation: &str,
    literal: &str,
    why: &str,
    at: Span,
    origin: Option<&AnnotationArg>,
) -> CompileError {
    let shown = match origin.and_then(|a| a.from_const.as_ref()) {
        Some(c) => format!(
            "`@{annotation}({})` is not a valid name here: {why} (`{literal}`)",
            c.name.name
        ),
        None => format!("`@{annotation}(\"{literal}\")` is not a valid name here: {why}"),
    };
    let err = CompileError::new(code, at, shown);
    match origin {
        Some(a) => crate::annotation_consts::with_const_origin(err, a),
        None => err,
    }
}

/// One name `@name` read on `annotations`: reports an unknown annotation (`unknown_code`), a
/// second `@name`, and an empty name. `None` when the position carries no usable `@name`.
fn name_of(
    annotations: &[Annotation],
    owner: &str,
    unknown_code: &'static str,
    unknown_note: &str,
    consts: &HashMap<String, ConstDecl>,
    errors: &mut Vec<CompileError>,
) -> Option<(String, Span)> {
    let mut found: Option<(String, Span)> = None;
    for ann in annotations {
        if ann.name.name != "name" {
            errors.push(
                CompileError::new(
                    unknown_code,
                    ann.span,
                    format!("unknown annotation `@{}` on {owner}", ann.name.name),
                )
                .with_note(unknown_note.to_string()),
            );
            continue;
        }
        let Some(NameArg {
            literal,
            at,
            origin,
        }) = string_arg(ann, consts, "bynk.types.field_name_bad_arg", errors)
        else {
            continue;
        };
        if found.is_some() {
            errors.push(CompileError::new(
                "bynk.types.field_name_duplicate",
                ann.span,
                format!("{owner} has more than one `@name`"),
            ));
            continue;
        }
        if literal.is_empty() {
            errors.push(invalid_name(
                "bynk.types.field_name_invalid",
                "name",
                &literal,
                "a wire name must not be empty",
                at,
                origin,
            ));
        }
        found = Some((literal, at));
    }
    found
}

/// The wire-name checks for one unit's own types: every `@name` on a record field, a variant or a
/// payload field, and `@discriminator` on a type, with the collisions between them.
pub(crate) fn validate_wire_names<'a>(
    types: impl IntoIterator<Item = &'a TypeDecl>,
    consts: &HashMap<String, ConstDecl>,
    errors: &mut Vec<CompileError>,
) {
    for decl in types {
        let name = &decl.name.name;
        let discriminator = discriminator_of(decl, consts, errors);
        match &decl.body {
            TypeBody::Record(rec) => validate_record(name, rec, consts, errors),
            TypeBody::Sum(sum) => {
                validate_variants(name, sum, discriminator.as_deref(), consts, errors)
            }
            _ => {}
        }
    }
}

/// A record's `@name`s: the annotation is known and well formed, a field has one, no name is
/// empty, and no two fields have the same wire name (a wire name is the `@name`, or the field's
/// own name). The request surfaces add their own rules in `context_checks`.
pub(crate) fn validate_record(
    name: &str,
    rec: &RecordBody,
    consts: &HashMap<String, ConstDecl>,
    errors: &mut Vec<CompileError>,
) {
    let mut wire: Vec<(String, Span)> = Vec::new();
    for f in &rec.fields {
        let mut external: Option<(String, Span)> = None;
        for ann in &f.annotations {
            // #4105: `@index` marks a data point's sampling key; it takes no argument.
            if ann.name.name == "index" {
                if !ann.args.is_empty() {
                    errors.push(CompileError::new(
                        "bynk.types.field_annotation_unknown",
                        ann.span,
                        "`@index` takes no argument",
                    ));
                }
                continue;
            }
            if ann.name.name != "name" {
                errors.push(
                    CompileError::new(
                        "bynk.types.field_annotation_unknown",
                        ann.span,
                        format!("unknown field annotation `@{}`", ann.name.name),
                    )
                    .with_note("the field annotations are `@name(\"…\")` and `@index`"),
                );
                continue;
            }
            let Some(NameArg {
                literal,
                at,
                origin,
            }) = string_arg(ann, consts, "bynk.types.field_name_bad_arg", errors)
            else {
                continue;
            };
            if external.is_some() {
                errors.push(CompileError::new(
                    "bynk.types.field_name_duplicate",
                    ann.span,
                    format!("field `{}` has more than one `@name`", f.name.name),
                ));
                continue;
            }
            if literal.is_empty() {
                errors.push(invalid_name(
                    "bynk.types.field_name_invalid",
                    "name",
                    &literal,
                    "a wire name must not be empty",
                    at,
                    origin,
                ));
            }
            external = Some((literal, at));
        }
        let (key, span) = external.unwrap_or_else(|| (f.name.name.clone(), f.name.span));
        wire.push((key, span));
    }
    for (i, (a, span)) in wire.iter().enumerate() {
        if wire[..i].iter().any(|(b, _)| a == b) {
            errors.push(CompileError::new(
                "bynk.types.field_name_duplicate",
                *span,
                format!("two fields of `{name}` read the same wire name `{a}`"),
            ));
        }
    }
}

/// The `@discriminator` of `decl`, reporting the annotation on a type that is not a sum, an
/// unknown annotation, a second `@discriminator` and an empty name.
fn discriminator_of(
    decl: &TypeDecl,
    consts: &HashMap<String, ConstDecl>,
    errors: &mut Vec<CompileError>,
) -> Option<String> {
    let is_sum = matches!(decl.body, TypeBody::Sum(_));
    let mut found: Option<String> = None;
    for ann in &decl.annotations {
        if ann.name.name != "discriminator" {
            errors.push(
                CompileError::new(
                    "bynk.types.annotation_unknown",
                    ann.span,
                    format!(
                        "unknown annotation `@{}` on type `{}`",
                        ann.name.name, decl.name.name
                    ),
                )
                .with_note("a type's annotation is `@discriminator(\"…\")`, on a sum"),
            );
            continue;
        }
        if !is_sum {
            errors.push(
                CompileError::new(
                    "bynk.types.discriminator_not_sum",
                    ann.span,
                    format!(
                        "`@discriminator` is on `{}`, which is not a sum type",
                        decl.name.name
                    ),
                )
                .with_note("only a sum's variants are told apart by a key; a record has none"),
            );
            continue;
        }
        let Some(NameArg {
            literal,
            at,
            origin,
        }) = string_arg(ann, consts, "bynk.types.discriminator_bad_arg", errors)
        else {
            continue;
        };
        if found.is_some() {
            errors.push(CompileError::new(
                "bynk.types.discriminator_bad_arg",
                ann.span,
                format!("`{}` has more than one `@discriminator`", decl.name.name),
            ));
            continue;
        }
        if literal.is_empty() {
            errors.push(invalid_name(
                "bynk.types.discriminator_bad_arg",
                "discriminator",
                &literal,
                "a discriminator must not be empty",
                at,
                origin,
            ));
            continue;
        }
        found = Some(literal);
    }
    found
}

fn validate_variants(
    type_name: &str,
    sum: &SumBody,
    discriminator: Option<&str>,
    consts: &HashMap<String, ConstDecl>,
    errors: &mut Vec<CompileError>,
) {
    let key = discriminator.unwrap_or(DEFAULT_DISCRIMINATOR);
    let mut seen: Vec<(String, Span)> = Vec::new();
    for v in &sum.variants {
        let owner = format!("variant `{}`", v.name.name);
        let wire = name_of(
            &v.annotations,
            &owner,
            "bynk.types.annotation_unknown",
            "a variant's annotation is `@name(\"…\")`",
            consts,
            errors,
        );
        let (wire_name, at) = wire.unwrap_or_else(|| (v.name.name.clone(), v.name.span));
        if let Some((_, _)) = seen.iter().find(|(n, _)| *n == wire_name) {
            errors.push(CompileError::new(
                "bynk.types.field_name_duplicate",
                at,
                format!("two variants of `{type_name}` have the wire name `{wire_name}`"),
            ));
        }
        seen.push((wire_name, at));
        let mut payload_names: Vec<String> = Vec::new();
        for p in &v.payload {
            let owner = format!("payload field `{}`", p.name.name);
            let wire = name_of(
                &p.annotations,
                &owner,
                "bynk.types.field_annotation_unknown",
                "a payload field's annotation is `@name(\"…\")`",
                consts,
                errors,
            );
            let (field_wire, at) = wire.unwrap_or_else(|| (p.name.name.clone(), p.name.span));
            if payload_names.contains(&field_wire) {
                errors.push(CompileError::new(
                    "bynk.types.field_name_duplicate",
                    at,
                    format!(
                        "two payload fields of `{}` have the wire name `{field_wire}`",
                        v.name.name
                    ),
                ));
            }
            if field_wire == key {
                errors.push(
                    CompileError::new(
                        "bynk.types.discriminator_collides",
                        at,
                        format!(
                            "payload field `{}` of `{}` has the wire name `{field_wire}`, which is `{type_name}`'s discriminator",
                            p.name.name, v.name.name
                        ),
                    )
                    .with_note(
                        "the discriminator and a payload field are keys of one object; rename the field with `@name`, or choose another `@discriminator`",
                    ),
                );
            }
            payload_names.push(field_wire);
        }
    }
}
