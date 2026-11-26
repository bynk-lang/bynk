//! #3793: the source-only rules for `field: T replaces old`.
//!
//! A field that declares `replaces old` says it was renamed from `old`, so its stored value, a
//! locked event's recorded field and an incoming wire key under the old name are all read as this
//! field. The lock-dependent rules (the old name is recorded, the clause is still needed) live with
//! the reconcile in [`crate::schema_registry`]; the rules here need only the declaration.
//!
//! One code, `bynk.field.replaces_conflict`, covers the three ways a clause contradicts its own
//! declaration: it names the field itself, it names a field the declaration still has, or two
//! fields claim the same old name.

use std::collections::HashMap;

use bynk_syntax::ast::{AgentDecl, Ident, RecordBody};
use bynk_syntax::error::CompileError;

/// The code every rule here reports.
pub const REPLACES_CONFLICT: &str = "bynk.field.replaces_conflict";

/// A collection `store` field `replaces` cannot rename (yet): a `Cache` or `Log`, or a `Map` with an
/// `@indexed` index. A `Map` or `Set` is renamed (#4482), by moving its entries to the new name.
pub const STORE_REPLACES_COLLECTION: &str = "bynk.store.replaces_collection";

/// Whether `replaces` can rename this `store` field: a `Cell`, or a `Map` or `Set` that keeps no index
/// (`@indexed` or `@unique`).
fn renamable(f: &bynk_syntax::ast::StoreField) -> bool {
    match f.kind.head.name.as_str() {
        "Cell" | "Set" => true,
        // `@unique` keeps the same index postings `@indexed` does.
        "Map" => !f
            .annotations
            .iter()
            .any(|a| matches!(a.name.name.as_str(), "indexed" | "unique")),
        _ => false,
    }
}

/// The clauses of one record (a type or an event body).
pub(crate) fn check_record(body: &RecordBody) -> Vec<CompileError> {
    let fields: Vec<(&Ident, Option<&Ident>)> = body
        .fields
        .iter()
        .map(|f| (&f.name, f.replaces.as_ref()))
        .collect();
    check(&fields, "record")
}

/// The clauses of one agent's `store` fields.
pub(crate) fn check_store(agent: &AgentDecl) -> Vec<CompileError> {
    // A `Map`, `Set`, `Cache` or `Log` keeps every entry under a key of its own that carries the
    // field's name, so a rename moves storage keys rather than reading an old one. A `Map` and a
    // `Set` are moved (#4482); the rest are refused for every layout, as which layout a record is in
    // is not the author's to see.
    let mut errors: Vec<CompileError> = agent
        .store_fields
        .iter()
        .filter(|f| !renamable(f))
        .filter_map(|f| f.replaces.as_ref().map(|old| (f, old)))
        .map(|(f, old)| {
            CompileError::new(
                STORE_REPLACES_COLLECTION,
                old.span,
                format!(
                    "`store {}` is a `{}`, which `replaces` cannot rename",
                    f.name.name, f.kind.head.name
                ),
            )
            .with_note(
                "a `Cell`, a `Set` and a `Map` without `@indexed` or `@unique` are renamed by \
                 `replaces`; a `Cache`, a `Log` and an indexed `Map` keep every entry, expiry or posting under \
                 a storage key of its own that carries the field's name. Keep the collection's \
                 name, or move its entries with a `migrate from` step into a new field",
            )
        })
        .collect();
    let fields: Vec<(&Ident, Option<&Ident>)> = agent
        .store_fields
        .iter()
        .map(|f| (&f.name, f.replaces.as_ref()))
        .collect();
    errors.extend(check(&fields, "agent"));
    errors
}

fn check(fields: &[(&Ident, Option<&Ident>)], owner: &str) -> Vec<CompileError> {
    let mut errors = Vec::new();
    let mut claimed: HashMap<&str, &Ident> = HashMap::new();
    for (name, replaces) in fields {
        let Some(old) = replaces else { continue };
        if old.name == name.name {
            errors.push(
                CompileError::new(
                    REPLACES_CONFLICT,
                    old.span,
                    format!("field `{}` replaces itself", name.name),
                )
                .with_note("`replaces` names the field's previous name; a field that was not renamed needs no clause"),
            );
        } else if let Some((other, _)) = fields.iter().find(|(n, _)| n.name == old.name) {
            errors.push(
                CompileError::new(
                    REPLACES_CONFLICT,
                    old.span,
                    format!(
                        "field `{}` replaces `{}`, but `{}` is still declared in this {owner}",
                        name.name, old.name, old.name
                    ),
                )
                .with_label(other.span, "still declared here")
                .with_note("a rename moves the old field's data to the new one; remove the old field, or drop `replaces` if they are two fields"),
            );
        }
        if let Some(first) = claimed.insert(&old.name, old) {
            errors.push(
                CompileError::new(
                    REPLACES_CONFLICT,
                    old.span,
                    format!("the field name `{}` is replaced more than once", old.name),
                )
                .with_label(first.span, "first replaced here")
                .with_note("a field was renamed to one name, so only one field can read it under the old one"),
            );
        }
    }
    errors
}
