//! The column mapping of an `Analytics` instance, recorded in `bynk.schema.lock` (#4105).
//!
//! `Signups.writeValue(point)` writes a record's fields to `blob1…`, `double1…` and `index1` by
//! position ([`crate::checker::analytics`]). A point already written keeps its positions, so
//! moving a field, changing its kind or removing it changes what every reader of the old points
//! sees, and nothing else reports it. The lock's `[analytics]` section records each instance's
//! mapping, and a build that finds a different one fails with `bynk.analytics.mapping_changed`
//! until the change is accepted (`bynk lock accept-analytics`), which rewrites the entry in the
//! same commit. The accept is the existing #3956 pattern (`bynk lock accept-order`), a sibling
//! subcommand of the same command, not a new family.
//!
//! What counts as a change: in a column family (blobs, doubles), a column that was there is
//! removed, moved, or changes kind (`text`, `enum`, `bool`, `number`, `decimal`), or the `@index`
//! field changes or is removed. Appending a field is allowed and the lock follows, and so is
//! renaming a field in place (the column keeps its place and its kind). The record's own name is
//! informational.
//!
//! The mapping is read from the checker's record of each `writeValue` call, so it is recorded
//! after the units are checked. An instance written with more than one record type has no single
//! mapping to lock; its recorded entry, if any, is left as it was.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bynk_project::schema_registry::{AnalyticsEntry, SchemaRegistry};
use bynk_syntax::ast::ExprId;
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::checker::Callee;
use crate::checker::analytics::{AnalyticsMapping, ColumnField, ColumnKind};

/// One instance as the units now write it: its lock key (`<unit>.<Instance>`), the mapping and
/// the record's declaration (where a refused change is reported).
pub struct LiveMapping {
    pub key: String,
    pub entry: AnalyticsEntry,
    pub span: Span,
}

/// The name of a column kind in the lock.
fn kind_name(kind: ColumnKind) -> &'static str {
    match kind {
        ColumnKind::Text => "text",
        ColumnKind::Variant => "enum",
        ColumnKind::Flag => "bool",
        ColumnKind::Number => "number",
        ColumnKind::Decimal => "decimal",
    }
}

/// The recorded form of a mapping.
pub fn entry_of(record: &str, mapping: &AnalyticsMapping) -> AnalyticsEntry {
    let column = |c: &ColumnField| format!("{}:{}", c.field, kind_name(c.kind));
    AnalyticsEntry {
        record: record.to_string(),
        index: mapping.index.clone(),
        blobs: mapping.blobs.iter().map(column).collect(),
        doubles: mapping.doubles.iter().map(column).collect(),
    }
}

/// The instances the units write with `writeValue`, and the keys of those written with more than
/// one record type (which have no single mapping to lock).
fn collect(
    unit_callees: &HashMap<String, HashMap<ExprId, Callee>>,
) -> (BTreeMap<String, LiveMapping>, BTreeSet<String>) {
    let mut found: BTreeMap<String, Vec<(String, AnalyticsMapping, Span)>> = BTreeMap::new();
    for (unit, callees) in unit_callees {
        for callee in callees.values() {
            if let Callee::AnalyticsWrite {
                cap,
                record,
                mapping,
                span,
            } = callee
            {
                found.entry(format!("{unit}.{cap}")).or_default().push((
                    record.clone(),
                    mapping.clone(),
                    *span,
                ));
            }
        }
    }
    let mut live = BTreeMap::new();
    let mut ambiguous = BTreeSet::new();
    for (key, mut sites) in found {
        let records: BTreeSet<&String> = sites.iter().map(|(r, _, _)| r).collect();
        if records.len() > 1 {
            ambiguous.insert(key);
            continue;
        }
        let (record, mapping, span) = sites.remove(0);
        live.insert(
            key.clone(),
            LiveMapping {
                key,
                entry: entry_of(&record, &mapping),
                span,
            },
        );
    }
    (live, ambiguous)
}

/// The instances the units now write, keyed `<unit>.<Instance>`.
pub fn live_mappings(
    unit_callees: &HashMap<String, HashMap<ExprId, Callee>>,
) -> BTreeMap<String, LiveMapping> {
    collect(unit_callees).0
}

/// A `name:kind` column's name.
fn column_name(column: &str) -> &str {
    column.split(':').next().unwrap_or(column)
}

/// A `name:kind` column's kind.
fn column_kind(column: &str) -> &str {
    column.rsplit(':').next().unwrap_or(column)
}

/// Why moving from the recorded `old` columns to the declared `new` ones changes what a column
/// holds, or `None` when only fields were appended or renamed in place.
fn columns_change(family: &str, old: &[String], new: &[String]) -> Option<String> {
    let removed = |o: &String, column: &str| {
        format!(
            "`{}` ({column}) is no longer written, so the columns after it move up",
            column_name(o)
        )
    };
    for (i, o) in old.iter().enumerate() {
        let column = format!("{family}{}", i + 1);
        let at = new.iter().position(|x| column_name(x) == column_name(o));
        let Some(n) = new.get(i) else {
            return Some(match at {
                Some(p) => format!(
                    "`{}` was {column} and is now {family}{}",
                    column_name(o),
                    p + 1
                ),
                None => removed(o, &column),
            });
        };
        if column_name(o) == column_name(n) {
            if column_kind(o) != column_kind(n) {
                return Some(format!(
                    "`{}` ({column}) was {} and is now {}",
                    column_name(o),
                    column_kind(o),
                    column_kind(n)
                ));
            }
            continue;
        }
        // The names differ at this column: the field moved, was removed, or was renamed in place.
        if let Some(p) = at {
            return Some(format!(
                "`{}` was {column} and is now {family}{}",
                column_name(o),
                p + 1
            ));
        }
        if old.iter().any(|x| column_name(x) == column_name(n)) {
            return Some(removed(o, &column));
        }
        if column_kind(o) != column_kind(n) {
            return Some(format!(
                "`{}` ({column}) was {} and is now {}",
                column_name(o),
                column_kind(o),
                column_kind(n)
            ));
        }
    }
    None
}

/// Why the declared mapping `new` differs from the recorded `old` in a way that matters, or
/// `None` when moving to it is allowed without acceptance.
fn change(old: &AnalyticsEntry, new: &AnalyticsEntry) -> Option<String> {
    if let Some(o) = &old.index
        && new.index.as_ref() != Some(o)
    {
        return Some(match &new.index {
            Some(n) => format!("the `@index` field `{o}` (index1) is now `{n}`"),
            None => format!("the `@index` field `{o}` (index1) is no longer written"),
        });
    }
    columns_change("blob", &old.blobs, &new.blobs)
        .or_else(|| columns_change("double", &old.doubles, &new.doubles))
}

/// A recorded or declared mapping as the diagnostic shows it.
pub fn describe(entry: &AnalyticsEntry) -> String {
    let mut parts = Vec::new();
    if let Some(i) = &entry.index {
        parts.push(format!("index1 = {i}"));
    }
    let family = |name: &str, columns: &[String]| {
        columns
            .iter()
            .enumerate()
            .map(|(i, c)| format!("{name}{} = {}", i + 1, column_name(c)))
            .collect::<Vec<_>>()
    };
    parts.extend(family("blob", &entry.blobs));
    parts.extend(family("double", &entry.doubles));
    if parts.is_empty() {
        "no columns".to_string()
    } else {
        parts.join(", ")
    }
}

/// The diagnostic for an instance whose mapping differs from the lock.
fn mapping_changed(item: &LiveMapping, recorded: &AnalyticsEntry, why: &str) -> CompileError {
    let name = item.key.rsplit('.').next().unwrap_or(&item.key);
    CompileError::new(
        "bynk.analytics.mapping_changed",
        item.span,
        format!("the columns `{name}` writes have changed: {why}"),
    )
    .with_note(format!("recorded mapping: {}", describe(recorded)))
    .with_note(format!("new mapping:      {}", describe(&item.entry)))
    .with_note(
        "points already written keep their old positions, so a query or a typed read of the new \
         mapping reads the wrong column of them",
    )
    .with_note(format!(
        "help: if this is intended, run `bynk lock accept-analytics {}`; otherwise restore the \
         record's fields (append new fields at the end)",
        item.key
    ))
}

/// Reconcile the instances the units write against the lock's `[analytics]`: record a new
/// instance, follow an allowed change, drop an instance no longer written, and report any other
/// change as `bynk.analytics.mapping_changed`, leaving that instance's recorded entry as it was.
pub fn reconcile_analytics(
    existing: &SchemaRegistry,
    updated: &mut SchemaRegistry,
    unit_callees: &HashMap<String, HashMap<ExprId, Callee>>,
    errors: &mut Vec<CompileError>,
) {
    let (live, ambiguous) = collect(unit_callees);
    for (key, item) in live {
        match existing.get_analytics(&key) {
            Some(old) => match change(old, &item.entry) {
                Some(why) => {
                    errors.push(mapping_changed(&item, old, &why));
                    updated.insert_analytics(key, old.clone());
                }
                None => updated.insert_analytics(key, item.entry),
            },
            None => updated.insert_analytics(key, item.entry),
        }
    }
    // Written with more than one record type: nothing new to lock, and nothing to lose.
    for key in ambiguous {
        if let Some(old) = existing.get_analytics(&key) {
            updated.insert_analytics(key, old.clone());
        }
    }
}

/// The lock with the mapping of each instance `select` names re-recorded as declared now
/// (`bynk lock accept-analytics`), and the keys it accepted. `select` is an instance name,
/// qualified (`shop.cart.Signups`) or not when only one instance has it.
pub fn accept_analytics(
    existing: &SchemaRegistry,
    unit_callees: &HashMap<String, HashMap<ExprId, Callee>>,
    select: Option<&str>,
) -> Result<(SchemaRegistry, Vec<String>), String> {
    let live = live_mappings(unit_callees);
    let mut targets: Vec<&String> = Vec::new();
    match select {
        None => {
            for (key, item) in &live {
                if existing.get_analytics(key) != Some(&item.entry) {
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
                        "no `Analytics` instance `{name}` written with `writeValue` in this project"
                    ));
                }
                [one] => targets.push(one),
                many => {
                    let names: Vec<&str> = many.iter().map(|k| k.as_str()).collect();
                    return Err(format!(
                        "`{name}` names more than one instance: {}; qualify it",
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
        if existing.get_analytics(key) != Some(&entry) {
            accepted.push(key.clone());
        }
        out.insert_analytics(key.clone(), entry);
    }
    Ok((out, accepted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(index: Option<&str>, blobs: &[&str], doubles: &[&str]) -> AnalyticsEntry {
        AnalyticsEntry {
            record: "Signup".to_string(),
            index: index.map(str::to_string),
            blobs: blobs.iter().map(|s| s.to_string()).collect(),
            doubles: doubles.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// Recorded `[a, b]`, now `[b, c]`: `a` was removed and `b` shifted, and the message names `a`.
    #[test]
    fn a_removal_plus_a_shift_reports_the_removal() {
        let why = change(
            &entry(None, &["a:text", "b:text"], &[]),
            &entry(None, &["b:text", "c:text"], &[]),
        )
        .expect("a change");
        assert!(why.contains("`a` (blob1) is no longer written"), "{why}");
        assert!(!why.contains('?'), "{why}");
    }

    /// Two fields of different kinds swapped is a move, not a retype.
    #[test]
    fn a_swap_between_kinds_reports_the_move() {
        let why = change(
            &entry(None, &["a:text", "b:enum"], &[]),
            &entry(None, &["b:enum", "a:text"], &[]),
        )
        .expect("a change");
        assert!(why.contains("`a` was blob1 and is now blob2"), "{why}");
    }

    #[test]
    fn appending_and_renaming_in_place_are_allowed() {
        let old = entry(
            Some("plan"),
            &["country:text", "channel:text"],
            &["seats:number"],
        );
        let appended = entry(
            Some("plan"),
            &["country:text", "channel:text", "ref:text"],
            &["seats:number", "amount:decimal"],
        );
        assert_eq!(change(&old, &appended), None, "append");
        let renamed = entry(
            Some("plan"),
            &["nation:text", "channel:text"],
            &["seats:number"],
        );
        assert_eq!(change(&old, &renamed), None, "rename in place");
        assert_eq!(
            change(
                &entry(None, &["a:text"], &[]),
                &entry(Some("p"), &["a:text"], &[])
            ),
            None,
            "an index added where there was none"
        );
    }

    #[test]
    fn a_reorder_a_retype_or_a_removal_is_a_change() {
        let old = entry(Some("plan"), &["a:text", "b:text"], &["n:number"]);
        assert!(
            change(
                &old,
                &entry(Some("plan"), &["b:text", "a:text"], &["n:number"])
            )
            .is_some(),
            "swap"
        );
        assert!(
            change(&old, &entry(Some("plan"), &["a:text"], &["n:number"])).is_some(),
            "removed last blob"
        );
        assert!(
            change(&old, &entry(Some("plan"), &["b:text"], &["n:number"])).is_some(),
            "removed first blob"
        );
        assert!(
            change(
                &old,
                &entry(Some("plan"), &["a:text", "b:bool"], &["n:number"])
            )
            .is_some(),
            "retyped"
        );
        assert!(
            change(&old, &entry(Some("plan"), &["a:text", "b:text"], &[])).is_some(),
            "doubles removed"
        );
        assert!(
            change(
                &old,
                &entry(Some("other"), &["a:text", "b:text"], &["n:number"])
            )
            .is_some(),
            "a different index"
        );
        assert!(
            change(&old, &entry(None, &["a:text", "b:text"], &["n:number"])).is_some(),
            "the index removed"
        );
    }

    #[test]
    fn a_field_moving_between_families_is_a_change() {
        let old = entry(None, &["a:text", "n:text"], &[]);
        let new = entry(None, &["a:text"], &["n:number"]);
        assert!(change(&old, &new).is_some());
    }
}
