//! #3974: the plain-JSON shapes of `bynk openapi`: what a `json { options: plain }` route writes for an
//! enum, a `String`-keyed `Map` and an `Instant`.

use std::collections::HashMap;
use std::sync::Arc;

use bynk_check::wire::WireSum;
use bynk_syntax::ast::{BaseType, TypeBody, TypeDecl, TypeRef};
use serde_json::{Map, Value, json};

/// An `Instant`, written as an RFC 3339 string.
pub(super) fn instant_schema() -> Value {
    json!({ "type": "string", "format": "date-time", "x-bynk-base": "Instant" })
}

/// A sum with no payload variant, written as its variant's name: a string `enum`. `None` for any
/// other sum.
pub(super) fn enum_schema(sum: &WireSum) -> Option<Map<String, Value>> {
    if sum.variants.is_empty() || sum.variants.iter().any(|v| !v.payload.is_empty()) {
        return None;
    }
    let names: Vec<&str> = sum.variants.iter().map(|v| v.name.as_str()).collect();
    Some(Map::from_iter([
        ("type".to_string(), json!("string")),
        ("enum".to_string(), json!(names)),
    ]))
}

/// Whether a map key is a `String`, or a refined or opaque `String`.
pub(super) fn is_string_key(types: &HashMap<String, Arc<TypeDecl>>, key: &TypeRef) -> bool {
    let named_string = |name: &str| {
        matches!(
            types.get(name).map(|d| &d.body),
            Some(
                TypeBody::Refined {
                    base: BaseType::String,
                    ..
                } | TypeBody::Opaque {
                    base: BaseType::String,
                    ..
                }
            )
        )
    };
    matches!(key, TypeRef::Base(BaseType::String, _))
        || matches!(key, TypeRef::Named(id) if named_string(&id.name))
}
