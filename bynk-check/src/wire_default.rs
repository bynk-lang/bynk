use std::sync::Arc;

use bynk_syntax::ast::{
    BaseType, Expr, ExprKind, RecordBody, SumBody, TypeBody, TypeDecl, TypeRef, UnaryOp,
};

/// Events slice 3a (#972): the **wire-form** JSON literal an event field's
/// default deserialises from when its key is absent from the incoming JSON.
/// Type-directed — given the field's expected `TypeRef` and the visible
/// `types` table, every syntactic ambiguity (a bare `Ident` that is really a
/// sum variant, a `FieldAccess` that is a qualified nullary variant) resolves
/// the same way the checker resolves it, just narrower (a bare name must
/// match a variant of *this* expected sum specifically) — so this needs no
/// per-expression `Ty` map (`expr_types`), which a subscriber regenerating a
/// publisher's own event codec (`emit_consumed_context_helpers`) has no way
/// to obtain anyway (no cross-unit `expr_types` store exists).
///
/// Produces the value in its **wire** shape (`kind` discriminant for a sum,
/// matching `emit_sum_codec`'s generated `Option`/`Result` instantiations
/// exactly — never the in-memory `tag` discriminant, and never a qualified
/// reference like `Region.Domestic` into a value namespace the emitting
/// module may not import), so it can be spliced in as the field's raw JSON
/// access and re-enter `emit_field_deserialise` (`bynk-emit`) completely
/// unchanged.
///
/// `Err(reason)` for anything not closed-form. The caller
/// (`bynk-emit/src/project/validate.rs`'s event-field-default check) turns
/// that into `bynk.event.bad_field_default` at check time, so a value this
/// function cannot build should never reach `emit_record` in practice — the
/// `.ok()` fallback there is a non-panicking safety net, not the intended
/// rejection path.
pub fn lower_field_default_wire(
    init: &Expr,
    expected: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Result<String, String> {
    if let ExprKind::Paren(inner) = &init.kind {
        return lower_field_default_wire(inner, expected, types);
    }
    match expected {
        TypeRef::Base(b, _) => lower_base_literal(*b, init),
        TypeRef::Named(id) => {
            let Some(decl) = types.get(&id.name) else {
                return Err(format!("cannot resolve type `{}`", id.name));
            };
            match &decl.body {
                TypeBody::Refined { base, .. } => lower_base_literal(*base, init),
                // ADR 0182: `.unsafe`'s value is an identity cast (`return
                // value as T;`, `emit.rs`'s opaque emission) — the literal
                // itself, verbatim, is the wire form. (Its refinement is
                // additionally checked *statically* for an event field
                // default specifically — `check_event_field_default` — since
                // this literal re-enters the same codec a real wire value
                // would, unlike an ordinary `.unsafe` bypass.)
                TypeBody::Opaque { base, .. } => match qualified_call(init) {
                    Some((type_name, "unsafe", [lit])) if type_name == id.name => {
                        lower_base_literal(*base, lit)
                    }
                    Some((_, "unsafe", _)) => {
                        Err("`.unsafe` takes exactly one argument".to_string())
                    }
                    _ => Err(format!(
                        "an opaque type's default must be `{}.unsafe(<literal>)`",
                        id.name
                    )),
                },
                TypeBody::Sum(s) => lower_sum_default(&id.name, s, init, types),
                TypeBody::Record(r) => lower_record_default(r, init, types),
            }
        }
        TypeRef::Option(inner, _) => match &init.kind {
            ExprKind::None => Ok("{ kind: \"None\" }".to_string()),
            ExprKind::Some(e) => {
                let v = lower_field_default_wire(e, inner, types)?;
                Ok(format!("{{ kind: \"Some\", value: {v} }}"))
            }
            _ => Err("an `Option` field default must be `Some(...)` or `None`".to_string()),
        },
        TypeRef::Result(ok, err, _) => match &init.kind {
            ExprKind::Ok(e) => {
                let v = lower_field_default_wire(e, ok, types)?;
                Ok(format!("{{ kind: \"Ok\", value: {v} }}"))
            }
            ExprKind::Err(e) => {
                let v = lower_field_default_wire(e, err, types)?;
                Ok(format!("{{ kind: \"Err\", error: {v} }}"))
            }
            _ => Err("a `Result` field default must be `Ok(...)` or `Err(...)`".to_string()),
        },
        TypeRef::List(elem, _) => match &init.kind {
            ExprKind::ListLit(items) => {
                let parts = items
                    .iter()
                    .map(|e| lower_field_default_wire(e, elem, types))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(format!("[{}]", parts.join(", ")))
            }
            _ => Err("a `List` field default must be a list literal".to_string()),
        },
        TypeRef::Map(..) => Err("a `Map` field has no closed-form default literal".to_string()),
        TypeRef::App { .. } => Err(
            "a generic type's field cannot carry a default (events are never generic)".to_string(),
        ),
        TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..) => Err("this field type is not wire-representable".to_string()),
    }
}

/// The wire form of a base-type literal — the raw literal a real wire value
/// of this base type would also be, so it re-enters
/// `emit_field_deserialise`'s (`bynk-emit`) ordinary `typeof`/
/// `Number.isInteger` checks unchanged. `Bytes` has no literal syntax, so it
/// is not admitted.
/// A qualified `TypeName.method(args)` call, in whichever `ExprKind` shape it
/// actually parses as. Confirmed empirically (`OrderId.unsafe("x")` parses to
/// `ExprKind::MethodCall { receiver: Ident("OrderId"), method: "unsafe", .. }`
/// — the parser never distinguishes a type-qualified call from an ordinary
/// instance method call; that's a resolver-time decision).
fn qualified_call(e: &Expr) -> Option<(&str, &str, &[Expr])> {
    match &e.kind {
        ExprKind::MethodCall {
            receiver,
            method,
            args,
            ..
        } => {
            let ExprKind::Ident(recv) = &receiver.kind else {
                return None;
            };
            Some((recv.name.as_str(), method.name.as_str(), args.as_slice()))
        }
        _ => None,
    }
}

fn lower_base_literal(base: BaseType, e: &Expr) -> Result<String, String> {
    // Strip one level of negation so `-5`/`-5.0` reach the literal arms below,
    // mirroring `const_literal`'s admission — `i64::checked_neg` guards
    // `i64::MIN`, which has no positive counterpart to negate away from.
    let (negate, inner) = match &e.kind {
        ExprKind::UnaryOp(UnaryOp::Neg, inner) => (true, &inner.kind),
        other => (false, other),
    };
    match (base, inner) {
        (BaseType::Int | BaseType::Instant, ExprKind::IntLit { value, .. }) => {
            let v = if negate {
                value
                    .checked_neg()
                    .ok_or_else(|| "integer literal has no negation".to_string())?
            } else {
                *value
            };
            Ok(v.to_string())
        }
        (BaseType::Float, ExprKind::IntLit { value, .. }) => {
            let v = if negate { -*value } else { *value };
            Ok(v.to_string())
        }
        (BaseType::Float, ExprKind::FloatLit { lexeme, .. }) => Ok(if negate {
            format!("-{lexeme}")
        } else {
            lexeme.clone()
        }),
        (BaseType::String, ExprKind::StrLit(s)) if !negate => {
            Ok(format!("\"{}\"", escape_ts_literal(s)))
        }
        (BaseType::Bool, ExprKind::BoolLit(b)) if !negate => Ok(b.to_string()),
        (BaseType::Duration, ExprKind::DurationLit { millis, .. }) if !negate => {
            Ok(millis.to_string())
        }
        (BaseType::Bytes, _) => Err("a `Bytes` field has no literal default form".to_string()),
        _ => Err(format!("expected a `{}` literal", base.name())),
    }
}

/// The wire form of a sum-variant default: `{ kind: "Variant" }` (nullary) or
/// `{ kind: "Variant", f1: ..., f2: ... }` (payload, positionally recursed
/// against each field's *declared* type) — never a qualified reference into
/// the sum's generated value namespace (`Sum.Variant`), which is what the
/// ordinary handler-body lowering (`lower_expr_into`) would produce and which
/// a foreign module regenerating this codec cannot import.
fn lower_sum_default(
    sum_name: &str,
    body: &SumBody,
    init: &Expr,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Result<String, String> {
    let (variant_name, args): (&str, &[Expr]) = match &init.kind {
        ExprKind::Ident(id) => (id.name.as_str(), &[]),
        ExprKind::FieldAccess { receiver, field } => {
            let ExprKind::Ident(recv) = &receiver.kind else {
                return Err(format!("expected a variant of `{sum_name}`"));
            };
            if recv.name != sum_name {
                return Err(format!(
                    "expected a variant of `{sum_name}`, not `{}`",
                    recv.name
                ));
            }
            (field.name.as_str(), &[])
        }
        ExprKind::Call { name, args, .. } => (name.name.as_str(), args.as_slice()),
        _ => match qualified_call(init) {
            Some((recv, method, args)) if recv == sum_name => (method, args),
            Some((recv, ..)) => {
                return Err(format!("expected a variant of `{sum_name}`, not `{recv}`"));
            }
            None => return Err(format!("expected a variant of `{sum_name}`")),
        },
    };
    let Some(variant) = body.variants.iter().find(|v| v.name.name == variant_name) else {
        return Err(format!("`{variant_name}` is not a variant of `{sum_name}`"));
    };
    if args.len() != variant.payload.len() {
        return Err(format!(
            "`{variant_name}` takes {} payload field(s), got {}",
            variant.payload.len(),
            args.len()
        ));
    }
    let mut parts = vec![format!("kind: \"{variant_name}\"")];
    for (field, arg) in variant.payload.iter().zip(args.iter()) {
        let v = lower_field_default_wire(arg, &field.type_ref, types)?;
        parts.push(format!("{}: {v}", field.name.name));
    }
    Ok(format!("{{ {} }}", parts.join(", ")))
}

/// The wire form of a record-literal default: a plain object literal, each
/// field recursed against its *declared* type. Records are structurally
/// wire-shaped (never tagged, never a named constructor), so this never
/// needs qualification either.
fn lower_record_default(
    body: &RecordBody,
    init: &Expr,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Result<String, String> {
    let ExprKind::RecordConstruction { fields, .. } = &init.kind else {
        return Err("expected a record literal".to_string());
    };
    let mut parts = Vec::new();
    for f in &body.fields {
        let Some(given) = fields.iter().find(|fi| fi.name.name == f.name.name) else {
            return Err(format!("record default is missing field `{}`", f.name.name));
        };
        let Some(value) = &given.value else {
            return Err(format!(
                "record default's field `{}` cannot use shorthand (no bindings are in scope)",
                f.name.name
            ));
        };
        let v = lower_field_default_wire(value, &f.type_ref, types)?;
        parts.push(format!("{}: {v}", f.name.name));
    }
    Ok(format!("{{ {} }}", parts.join(", ")))
}

/// Escapes a string for embedding in a TypeScript double-quoted string
/// literal. `pub` because `bynk-emit`'s `escape_ts_string` (~65 call sites
/// across the emitter's real emission code) delegates here rather than
/// keeping its own copy — the two must stay byte-identical since both
/// splice into generated TypeScript, so this is the one shared definition.
pub fn escape_ts_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}
