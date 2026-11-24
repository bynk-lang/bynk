//! The order of a `derives Ord` type, recorded in `bynk.schema.lock` (#3956).
//!
//! A `derives Ord` type orders by declaration order: a record by its fields in
//! turn, a sum by variant position and then payload. Moving a field or a variant
//! changes how every value of the type sorts, and nothing else reports it: the
//! wire contract hash ignores that order. The lock's `[orders]` section records
//! each of the project's own ordered types, and a build that finds a different
//! order fails with `bynk.types.ord_order_changed` until the change is accepted
//! (`bynk lock accept-order`), which rewrites the entry in the same commit.
//!
//! What is recorded is names only: the field names of a record, the variant
//! names of a sum with each variant's payload field names. A field's type
//! changing is a separate, type-checked change.
//!
//! What counts as a change ([DECISION C]): two names that were both present swap
//! relative order, or a field or variant is inserted anywhere but at the end.
//! Appending, removing and renaming in place are allowed and the lock follows.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bynk_project::ParsedFile;
use bynk_project::schema_registry::{OrderEntry, OrderMember, SchemaRegistry};
use bynk_syntax::ast::{CommonsItem, TypeBody, TypeDecl};
use bynk_syntax::error::{Applicability, CompileError};
use bynk_syntax::span::Span;

/// One of the project's own `derives Ord` types: its lock key
/// (`<unit>.<Type>`), the declaration and the order it declares.
pub struct OrderedType<'a> {
    pub key: String,
    pub decl: Arc<TypeDecl>,
    pub entry: OrderEntry,
    /// The source text of the file the declaration is in, for the edits that put
    /// its members back in the recorded order.
    pub source: &'a str,
}

/// The order `decl` declares, or `None` for a type that does not derive `Ord`
/// or whose order is not positional (a refined or opaque type orders by its base).
pub fn signature(decl: &TypeDecl) -> Option<OrderEntry> {
    if !decl.derives.iter().any(|d| d.name == "Ord") {
        return None;
    }
    match &decl.body {
        TypeBody::Record(r) => Some(OrderEntry {
            kind: "record".to_string(),
            members: r
                .fields
                .iter()
                .map(|f| OrderMember {
                    name: f.name.name.clone(),
                    fields: Vec::new(),
                })
                .collect(),
        }),
        TypeBody::Sum(s) => Some(OrderEntry {
            kind: "sum".to_string(),
            members: s
                .variants
                .iter()
                .map(|v| OrderMember {
                    name: v.name.name.clone(),
                    fields: v.payload.iter().map(|p| p.name.name.clone()).collect(),
                })
                .collect(),
        }),
        TypeBody::Refined { .. } | TypeBody::Opaque { .. } => None,
    }
}

/// Every `derives Ord` type the project itself declares, keyed `<unit>.<Type>`:
/// a dependency's types and the standard library's are not the project's to
/// accept.
pub fn live_orders(parsed: &[ParsedFile]) -> BTreeMap<String, OrderedType<'_>> {
    let mut out = BTreeMap::new();
    for pf in parsed {
        if pf.is_synthetic() || pf.package().is_some() {
            continue;
        }
        let unit = pf.unit().name().joined();
        for item in pf.items() {
            let CommonsItem::Type(t) = item else {
                continue;
            };
            let Some(entry) = signature(t) else {
                continue;
            };
            let key = format!("{unit}.{}", t.name.name);
            out.insert(
                key.clone(),
                OrderedType {
                    key,
                    decl: Arc::new(t.clone()),
                    entry,
                    source: pf.source(),
                },
            );
        }
    }
    out
}

/// Whether the order of the names in `old` survives in `new`: the names both
/// have keep their relative order, and a name `new` adds sits at the end (or
/// takes the place of one `old` had, a rename in place).
fn names_keep_order(old: &[String], new: &[String]) -> bool {
    let in_new: BTreeSet<&str> = new.iter().map(String::as_str).collect();
    let in_old: BTreeSet<&str> = old.iter().map(String::as_str).collect();
    let common_old: Vec<&str> = old
        .iter()
        .map(String::as_str)
        .filter(|n| in_new.contains(n))
        .collect();
    let common_new: Vec<&str> = new
        .iter()
        .map(String::as_str)
        .filter(|n| in_old.contains(n))
        .collect();
    if common_old != common_new {
        return false;
    }
    // The names each side holds between two neighbouring shared names (or before
    // the first, or after the last): a gap where the sides disagree.
    let gap = |names: &[String], other: &BTreeSet<&str>, k: usize| -> usize {
        let mut seen = 0;
        let mut count = 0;
        for n in names {
            if other.contains(n.as_str()) {
                seen += 1;
                continue;
            }
            if seen == k {
                count += 1;
            }
        }
        count
    };
    for k in 0..common_old.len() {
        let removed = gap(old, &in_new, k);
        let added = gap(new, &in_old, k);
        // A name inserted in the middle changes how existing values compare with
        // it; one replacing a removed name in the same place is a rename, and
        // fewer new names than removed ones is a rename plus a removal.
        if added > removed {
            return false;
        }
    }
    true
}

/// Whether moving from the recorded `old` order to the declared `new` one is
/// allowed without acceptance.
fn order_preserved(old: &OrderEntry, new: &OrderEntry) -> bool {
    let names =
        |e: &OrderEntry| -> Vec<String> { e.members.iter().map(|m| m.name.clone()).collect() };
    if old.kind != new.kind {
        // A record turned into a sum, or the reverse, is a type-checked change.
        return true;
    }
    if !names_keep_order(&names(old), &names(new)) {
        return false;
    }
    // A variant's payload fields compare in order too.
    old.members.iter().all(|o| {
        new.members
            .iter()
            .find(|n| n.name == o.name)
            .is_none_or(|n| names_keep_order(&o.fields, &n.fields))
    })
}

/// A recorded or declared order as the diagnostic and hover show it.
pub fn describe(entry: &OrderEntry) -> String {
    let member = |m: &OrderMember| {
        if m.fields.is_empty() {
            m.name.clone()
        } else {
            format!("{}({})", m.name, m.fields.join(", "))
        }
    };
    let joined = |sep: &str| {
        entry
            .members
            .iter()
            .map(member)
            .collect::<Vec<_>>()
            .join(sep)
    };
    if entry.kind == "sum" {
        joined(" < ")
    } else {
        format!(
            "{} (compared field by field, the first first)",
            joined(", ")
        )
    }
}

/// The source spans of the type's members (a record's fields or a sum's
/// variants), in the order they are written.
fn member_spans(decl: &TypeDecl) -> Vec<Span> {
    match &decl.body {
        TypeBody::Record(r) => r.fields.iter().map(|f| f.span).collect(),
        TypeBody::Sum(s) => s.variants.iter().map(|v| v.span).collect(),
        TypeBody::Refined { .. } | TypeBody::Opaque { .. } => Vec::new(),
    }
}

/// The edits that put the type's members back in the recorded order: each slot a
/// member is written in takes the text of the member that belongs there, and a
/// member the lock has not seen stays after the recorded ones. `None` when the
/// members are already in that order (only a payload moved) or a span does not
/// slice the source.
fn restore_edits(item: &OrderedType<'_>, recorded: &OrderEntry) -> Option<Vec<(Span, String)>> {
    let spans = member_spans(&item.decl);
    let current: Vec<&str> = item.entry.members.iter().map(|m| m.name.as_str()).collect();
    if spans.len() != current.len() {
        return None;
    }
    let mut target: Vec<usize> = recorded
        .members
        .iter()
        .filter_map(|m| current.iter().position(|n| *n == m.name))
        .collect();
    let unseen: Vec<usize> = (0..current.len()).filter(|i| !target.contains(i)).collect();
    target.extend(unseen);
    let mut edits = Vec::new();
    for (slot, &from) in target.iter().enumerate() {
        if slot == from {
            continue;
        }
        let src = spans[from];
        let text = item.source.get(src.start..src.end)?;
        edits.push((spans[slot], text.to_string()));
    }
    (!edits.is_empty()).then_some(edits)
}

/// The diagnostic for a type whose order differs from the lock.
fn order_changed(item: &OrderedType<'_>, recorded: &OrderEntry) -> CompileError {
    let name = &item.decl.name.name;
    let error = CompileError::new(
        "bynk.types.ord_order_changed",
        item.decl.span,
        format!("the order of `{name}` has changed"),
    )
    .with_note(format!("recorded order: {}", describe(recorded)))
    .with_note(format!("new order:      {}", describe(&item.entry)))
    .with_note(format!(
        "every sort, `<`, `min` and `max` on `{name}` now gives a different result"
    ))
    .with_note(format!(
        "help: if this is intended, run `bynk lock accept-order {}`; otherwise restore the order",
        item.key
    ));
    match restore_edits(item, recorded) {
        Some(edits) => error.with_suggestion(
            "Restore the recorded order",
            edits,
            Applicability::MaybeIncorrect,
        ),
        None => error,
    }
}

/// Reconcile the project's `derives Ord` types against the lock's `[orders]`:
/// record a new type, follow an allowed change, drop a type that is gone or no
/// longer derives `Ord`, and report any other change as
/// `bynk.types.ord_order_changed`, leaving that type's recorded entry as it was.
pub fn reconcile_orders(
    existing: &SchemaRegistry,
    updated: &mut SchemaRegistry,
    parsed: &[ParsedFile],
    errors: &mut Vec<CompileError>,
) {
    for (key, item) in live_orders(parsed) {
        match existing.get_order(&key) {
            Some(old) if !order_preserved(old, &item.entry) => {
                errors.push(order_changed(&item, old));
                updated.insert_order(key, old.clone());
            }
            _ => updated.insert_order(key, item.entry),
        }
    }
}

/// The lock with the order of each type `select` names re-recorded as declared
/// now (`bynk lock accept-order`), and the keys it accepted. `select` is a type
/// name, qualified (`shop.tasks.Priority`) or not when only one type has it.
pub fn accept_orders(
    existing: &SchemaRegistry,
    parsed: &[ParsedFile],
    select: Option<&str>,
) -> Result<(SchemaRegistry, Vec<String>), String> {
    let live = live_orders(parsed);
    let mut targets: Vec<&String> = Vec::new();
    match select {
        None => {
            for (key, item) in &live {
                if existing.get_order(key) != Some(&item.entry) {
                    targets.push(key);
                }
            }
        }
        Some(name) => {
            let hits: Vec<&String> = live
                .keys()
                .filter(|k| k.as_str() == name || k.rsplit('.').next() == Some(name))
                .collect();
            match hits.as_slice() {
                [] => {
                    return Err(format!(
                        "no `derives Ord` type `{name}` in this project (a dependency's types are not accepted here)"
                    ));
                }
                [one] => targets.push(one),
                many => {
                    let names: Vec<&str> = many.iter().map(|k| k.as_str()).collect();
                    return Err(format!(
                        "`{name}` names more than one type: {}; qualify it",
                        names.join(", ")
                    ));
                }
            }
        }
    }
    let mut out = existing.clone();
    let mut accepted = Vec::new();
    for key in targets {
        let entry = live[key].entry.clone();
        if existing.get_order(key) != Some(&entry) {
            accepted.push(key.clone());
        }
        out.insert_order(key.clone(), entry);
    }
    Ok((out, accepted))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_syntax::{lexer, parser};

    fn names(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn appending_removing_and_renaming_in_place_keep_the_order() {
        let old = names(&["a", "b", "c"]);
        assert!(
            names_keep_order(&old, &names(&["a", "b", "c", "d"])),
            "append"
        );
        assert!(names_keep_order(&old, &names(&["a", "c"])), "remove");
        assert!(
            names_keep_order(&old, &names(&["a", "x", "c"])),
            "rename in place"
        );
        assert!(
            names_keep_order(&old, &names(&["a", "b"])),
            "remove the last"
        );
        let four = names(&["a", "b", "c", "d"]);
        assert!(
            names_keep_order(&four, &names(&["a", "x", "d"])),
            "one removed and one renamed in the same gap"
        );
        assert!(
            !names_keep_order(&four, &names(&["a", "x", "y", "z", "d"])),
            "more new names than removed ones in a gap"
        );
    }

    #[test]
    fn a_swap_or_a_mid_insert_changes_the_order() {
        let old = names(&["a", "b", "c"]);
        assert!(!names_keep_order(&old, &names(&["b", "a", "c"])), "swap");
        assert!(
            !names_keep_order(&old, &names(&["a", "x", "b", "c"])),
            "insert in the middle"
        );
        assert!(
            !names_keep_order(&old, &names(&["x", "a", "b", "c"])),
            "insert at the start"
        );
        assert!(!names_keep_order(&old, &names(&["c", "b", "a"])), "reverse");
    }

    fn ordered(source: &str, name: &str) -> OrderedType<'static> {
        let source: &'static str = Box::leak(source.to_string().into_boxed_str());
        let tokens = lexer::tokenize(source).expect("lexes");
        let (unit, _) = parser::parse_unit_with_recovery(&tokens, source);
        let bynk_syntax::ast::SourceUnit::Commons(c) = unit.expect("parses") else {
            panic!("a commons");
        };
        let decl = c
            .items
            .iter()
            .find_map(|i| match i {
                CommonsItem::Type(t) if t.name.name == name => Some(t.clone()),
                _ => None,
            })
            .expect("the type");
        let entry = signature(&decl).expect("derives Ord");
        OrderedType {
            key: format!("m.{name}"),
            decl: Arc::new(decl),
            entry,
            source,
        }
    }

    #[test]
    fn a_type_that_does_not_derive_ord_has_no_order() {
        let item = ordered("commons m\n\ntype P = { a: Int } derives Ord\n", "P");
        assert_eq!(item.entry.kind, "record");
        let tokens = lexer::tokenize("commons m\n\ntype Q = { a: Int }\n").unwrap();
        let (unit, _) =
            parser::parse_unit_with_recovery(&tokens, "commons m\n\ntype Q = { a: Int }\n");
        let bynk_syntax::ast::SourceUnit::Commons(c) = unit.unwrap() else {
            panic!()
        };
        let CommonsItem::Type(q) = &c.items[0] else {
            panic!()
        };
        assert!(signature(q).is_none());
    }

    #[test]
    fn restoring_puts_each_member_back_in_its_recorded_slot() {
        let recorded = ordered(
            "commons m\n\ntype Priority = enum { Low, Medium, High } derives Ord\n",
            "Priority",
        )
        .entry;
        let now = ordered(
            "commons m\n\ntype Priority = enum { High, Low, Medium } derives Ord\n",
            "Priority",
        );
        let edits = restore_edits(&now, &recorded).expect("an edit");
        // Apply back to front so earlier offsets hold.
        let mut text = now.source.to_string();
        let mut sorted = edits.clone();
        sorted.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
        for (span, replacement) in sorted {
            text.replace_range(span.start..span.end, &replacement);
        }
        assert!(text.contains("enum { Low, Medium, High }"), "{text}");
    }

    #[test]
    fn a_new_member_stays_after_the_recorded_ones_when_restoring() {
        let recorded = ordered(
            "commons m\n\ntype P = { a: Int, b: Int } derives Ord\n",
            "P",
        )
        .entry;
        let now = ordered(
            "commons m\n\ntype P = { b: Int, a: Int, c: Int } derives Ord\n",
            "P",
        );
        let edits = restore_edits(&now, &recorded).expect("an edit");
        let mut text = now.source.to_string();
        let mut sorted = edits.clone();
        sorted.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
        for (span, replacement) in sorted {
            text.replace_range(span.start..span.end, &replacement);
        }
        assert!(text.contains("{ a: Int, b: Int, c: Int }"), "{text}");
    }
}
