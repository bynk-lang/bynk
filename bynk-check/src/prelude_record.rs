//! #4192: `MapEntry[K, V]` as a transparent prelude record.
//!
//! `map.entries()` has always produced `MapEntry` values, but the name was not
//! usable in source, so `Map.of` could only rebuild a map from another map's
//! entries. The record is `{ key: K, value: V }`, and it is written like any
//! generic record: `MapEntry { key: k, value: v }`, `List[MapEntry[K, V]]`.
//!
//! It has no declaration in any unit, so the lookups that resolve a type name
//! fall back to the declaration here, parsed once from its own source. A type
//! of the same name that a unit declares or imports wins, as it did before.

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

/// The declaration of the type `name`: the unit's own when it has one, else the
/// prelude's.
pub(crate) fn lookup(name: &str, types: &HashMap<String, Arc<TypeDecl>>) -> Option<Arc<TypeDecl>> {
    types
        .get(name)
        .cloned()
        .or_else(|| (name == MAP_ENTRY).then(|| map_entry_decl().clone()))
}
