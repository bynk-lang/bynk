//! Durable event delivery §8.1 (#3898): `bynk.event.unbounded_payload`, a warning on an
//! `event` whose payload is plainly unbounded in size.
//!
//! An event will travel as one Cloudflare Queue message, which holds at most 128 KB (about
//! 127,000 bytes of it the event's own), so a large event will fault its handler. The size is
//! only known at run time, so this is a hint, not a check: it fires where large events come
//! from, a `Bytes`, or a `List`, `Map` or `Set` of a non-scalar type that no refinement (such as
//! `MaxLength(n)`) bounds. A `Bytes` takes no length refinement, so it always warns. A payload of scalars and `String`s does not warn, since nearly every event would.

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
                 store a large value (R2, D1) and send a reference to it; a collection can instead be bounded \
                 with a refinement such as `MaxLength(n)`"
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
        // A refinement on the collection (`MaxLength(n)`) bounds it.
        TypeRef::List(..) | TypeRef::Set(..) | TypeRef::Map(..) if refinement.is_some() => None,
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

/// A value as small as a scalar: a base type other than `Bytes`, a refined or opaque type over
/// one, a sum whose variants carry no payload (an enum), or an `Option` of any of these.
fn is_scalar(t: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    match t {
        TypeRef::Base(b, _) => *b != BaseType::Bytes,
        TypeRef::Option(inner, _) => is_scalar(inner, types),
        TypeRef::Named(id) => types.get(&id.name).is_some_and(|d| match &d.body {
            TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. } => {
                *base != BaseType::Bytes
            }
            TypeBody::Sum(body) => body.variants.iter().all(|v| v.payload.is_empty()),
            TypeBody::Record(_) => false,
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    /// How many `bynk.event.unbounded_payload` warnings a context holding `body` draws.
    fn warnings(body: &str) -> usize {
        let source = format!(
            "context t.e\n\ntype Line = {{ sku: String }}\n\ntype Status = enum {{ Open, Closed }}\n\n{body}\n"
        );
        analyse(&[("t/e.bynk", &source)])
            .categories()
            .into_iter()
            .filter(|c| *c == "bynk.event.unbounded_payload")
            .count()
    }

    #[test]
    fn scalars_strings_enums_and_options_of_scalars_do_not_warn() {
        // §8.1: such a payload would warn on nearly every event.
        assert_eq!(
            warnings("event E = { a: String, b: Int, c: Status, d: Option[Int] }"),
            0
        );
        assert_eq!(
            warnings(
                "event E = { a: List[String], b: Map[String, Int], c: Set[Int], d: List[Status], e: List[Option[Int]] }"
            ),
            0
        );
    }

    #[test]
    fn a_refinement_on_a_collection_bounds_it() {
        assert_eq!(
            warnings("event E = { lines: List[Line] where MaxLength(10) }"),
            0
        );
        assert_eq!(warnings("event E = { lines: List[Line] }"), 1);
    }

    #[test]
    fn bytes_and_collections_of_records_warn_however_they_are_reached() {
        assert_eq!(warnings("event E = { data: Bytes }"), 1);
        assert_eq!(warnings("event E = { data: Option[Bytes] }"), 1);
        assert_eq!(warnings("type Blob = Bytes\n\nevent E = { blob: Blob }"), 1);
        assert_eq!(
            warnings("type A = { name: String, body: Bytes }\n\nevent E = { a: A }"),
            1
        );
        assert_eq!(
            warnings(
                "type C =\n  | Text(body: String)\n  | Binary(data: Bytes)\n\nevent E = { c: C }"
            ),
            1
        );
        assert_eq!(warnings("event E = { m: Map[String, Line] }"), 1);
    }

    #[test]
    fn a_recursive_type_is_followed_once_and_warns_once() {
        assert_eq!(
            warnings(
                "type Node = { label: String, children: List[Node] }\n\nevent E = { root: Node }"
            ),
            1
        );
        // A recursion through scalars only stays silent.
        assert_eq!(
            warnings(
                "type Chain = { label: String, next: Option[Chain] }\n\nevent E = { root: Chain }"
            ),
            0
        );
    }

    #[test]
    fn each_event_warns_at_most_once() {
        assert_eq!(
            warnings("event E = { a: Bytes, b: Bytes, c: List[Line] }"),
            1
        );
    }
}
