//! Durable event delivery §8.1 (#3898): `bynk.event.unbounded_payload`, a warning on an
//! `event` whose payload is plainly unbounded in size.
//!
//! An event will travel as one Cloudflare Queue message, which holds at most 128 KB (about
//! 127,000 bytes of it the event's own), so a large event will fault its handler. The size is
//! only known at run time, so this is a hint, not a check: it fires where large events come
//! from, a `Bytes` or a `List`, `Map` or `Set` of a non-scalar type, with no refinement that
//! bounds it. A payload of scalars and `String`s does not warn, since nearly every event would.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bynk_syntax::ast::{BaseType, EventDecl, Refinement, TypeBody, TypeDecl, TypeRef};
use bynk_syntax::error::CompileError;

use crate::context_checks::ts_type_ref_display;

/// The byte budget the warning names (track D3).
const EVENT_BUDGET_BYTES: u32 = 127_000;

/// Warn on each event of `events` whose payload reaches an unbounded `Bytes` or collection.
pub(crate) fn check_event_payload_sizes<'a>(
    events: impl Iterator<Item = &'a EventDecl>,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    let mut events: Vec<&EventDecl> = events.collect();
    events.sort_by_key(|e| e.span);
    for event in events {
        let found = event.body.fields.iter().find_map(|field| {
            let mut seen = HashSet::new();
            unbounded(&field.type_ref, field.refinement.as_ref(), types, &mut seen)
                .map(|what| (field, what))
        });
        let Some((field, what)) = found else {
            continue;
        };
        errors.push(
            CompileError::new(
                "bynk.event.unbounded_payload",
                field.name.span,
                format!(
                    "event `{}`'s field `{}` reaches {what}, which has no size bound",
                    event.name.name, field.name.name
                ),
            )
            .with_note(format!(
                "on Workers an event must fit one queue message, about {EVENT_BUDGET_BYTES} bytes; \
                 store a large value (R2, D1) and send a reference to it, or bound it with a refinement"
            )),
        );
    }
}

/// What unbounded part `t` reaches, if any. `refinement` is the refinement written on the
/// field or type that holds `t`; any refinement counts as a bound.
fn unbounded(
    t: &TypeRef,
    refinement: Option<&Refinement>,
    types: &HashMap<String, Arc<TypeDecl>>,
    seen: &mut HashSet<String>,
) -> Option<String> {
    match t {
        TypeRef::Base(BaseType::Bytes, _) if refinement.is_none() => Some("`Bytes`".into()),
        TypeRef::Base(..) => None,
        TypeRef::Option(inner, _) => unbounded(inner, refinement, types, seen),
        TypeRef::List(elem, _) | TypeRef::Set(elem, _) if !is_scalar(elem, types) => Some(format!(
            "`{}`, a collection of a non-scalar type",
            ts_type_ref_display(t)
        )),
        TypeRef::Map(_, value, _) if !is_scalar(value, types) => Some(format!(
            "`{}`, a collection of a non-scalar type",
            ts_type_ref_display(t)
        )),
        TypeRef::List(elem, _) | TypeRef::Set(elem, _) => unbounded(elem, None, types, seen),
        TypeRef::Map(_, value, _) => unbounded(value, None, types, seen),
        TypeRef::Named(id) => {
            if !seen.insert(id.name.clone()) {
                return None;
            }
            let decl = types.get(&id.name)?;
            match &decl.body {
                TypeBody::Refined {
                    base: BaseType::Bytes,
                    refinement: r,
                    ..
                }
                | TypeBody::Opaque {
                    base: BaseType::Bytes,
                    refinement: r,
                    ..
                } if r.is_none() && refinement.is_none() => {
                    Some(format!("`{}` (`Bytes`)", id.name))
                }
                TypeBody::Refined { .. } | TypeBody::Opaque { .. } => None,
                TypeBody::Record(body) => body
                    .fields
                    .iter()
                    .find_map(|f| unbounded(&f.type_ref, f.refinement.as_ref(), types, seen)),
                TypeBody::Sum(body) => body
                    .variants
                    .iter()
                    .flat_map(|v| &v.payload)
                    .find_map(|f| unbounded(&f.type_ref, None, types, seen)),
            }
        }
        _ => None,
    }
}

/// A base type other than `Bytes`, or a refined or opaque type over one.
fn is_scalar(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    match t {
        TypeRef::Base(b, _) => *b != BaseType::Bytes,
        TypeRef::Named(id) => types.get(&id.name).is_some_and(|d| {
            matches!(
                &d.body,
                TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } if *base != BaseType::Bytes
            )
        }),
        _ => false,
    }
}
