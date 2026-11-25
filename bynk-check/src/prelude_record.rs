//! #4192: `MapEntry[K, V]` as a transparent prelude record.
//!
//! `map.entries()` has always produced `MapEntry` values, but the name was not
//! usable in source, so `Map.of` could only rebuild a map from another map's
//! entries. The record is `{ key: K, value: V }`, and it is written like any
//! generic record: `MapEntry { key: k, value: v }`, `List[MapEntry[K, V]]`.
//!
//! It has no declaration in any unit, so the lookups that resolve a type name
//! fall back to the declaration here, parsed once from its own source. The
//! two-parameter shape is reserved: a unit cannot declare its own generic
//! `MapEntry[A, B]`, because the emitter renders every `MapEntry[_, _]` as the
//! inline `{ key, value }` object. A `MapEntry` with another number of
//! parameters (or none) is an ordinary type of the unit's own.

use bynk_syntax::ast::{CommonsItem, TypeDecl};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use crate::builtin_names::types::MAP_ENTRY;

const SOURCE: &str = "commons prelude\n\ntype MapEntry[K, V] = { key: K, value: V }\n";

/// The prelude `MapEntry` declaration.
fn map_entry_decl() -> &'static Arc<TypeDecl> {
    static DECL: OnceLock<Arc<TypeDecl>> = OnceLock::new();
    DECL.get_or_init(|| {
        let tokens = bynk_syntax::lexer::tokenize(SOURCE).expect("the prelude source lexes");
        let commons =
            bynk_syntax::parser::parse(&tokens, SOURCE).expect("the prelude source parses");
        let decl = commons
            .items
            .iter()
            .find_map(|item| match item {
                CommonsItem::Type(t) => Some(t.clone()),
                _ => None,
            })
            .expect("the prelude source declares MapEntry");
        Arc::new(decl)
    })
}

/// Is `decl` a declaration of the reserved `MapEntry[A, B]`?
pub(crate) fn is_reserved(decl: &TypeDecl) -> bool {
    decl.name.name == MAP_ENTRY && decl.type_params.len() == 2
}

/// The error for a type name declared twice, with the first declaration labelled.
pub(crate) fn duplicate_error(
    name: &bynk_syntax::ast::Ident,
    prev: &TypeDecl,
) -> bynk_syntax::error::CompileError {
    bynk_syntax::error::CompileError::new(
        "bynk.resolve.duplicate_type",
        name.span,
        format!("type `{}` is already declared", name.name),
    )
    .with_label(prev.name.span, "previously declared here")
}

/// The error for a unit that declares the reserved `MapEntry[A, B]`, at the declared name.
pub(crate) fn reserved_error(name: &bynk_syntax::ast::Ident) -> bynk_syntax::error::CompileError {
    bynk_syntax::error::CompileError::new(
        "bynk.resolve.duplicate_type",
        name.span,
        format!("type `{}` is already declared", name.name),
    )
    .with_note(
        "`MapEntry[K, V]` is a prelude record (`{ key: K, value: V }`); use it, or name your own type differently",
    )
}

/// The declaration of the type `name`: the unit's own when it has one, else the
/// prelude's.
pub(crate) fn lookup(name: &str, types: &HashMap<String, Arc<TypeDecl>>) -> Option<Arc<TypeDecl>> {
    types
        .get(name)
        .cloned()
        .or_else(|| (name == MAP_ENTRY).then(|| map_entry_decl().clone()))
}
