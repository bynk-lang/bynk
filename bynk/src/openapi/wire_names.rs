//! #3975: the wire names a declaration gives the schemas of a generic instantiation and a path
//! enum. A non-generic type's wire names are read off its `WireType`; an instantiation is built
//! from the declaration's substituted types, so its names are looked up on the declaration.

use std::collections::HashMap;
use std::sync::Arc;

use bynk_check::wire::WIRE_DISCRIMINANT;
use bynk_syntax::ast::{SumBody, TypeBody, TypeDecl};

type Types = HashMap<String, Arc<TypeDecl>>;

/// The segment text of each variant of a payload-less sum used as a path parameter: its wire
/// name.
pub(super) fn path_enum_names(sum: &SumBody) -> Vec<String> {
    sum.variants
        .iter()
        .map(|v| v.wire_name().unwrap_or_else(|| v.name.name.clone()))
        .collect()
}

/// The wire key of field `field` of the generic record `record`.
pub(super) fn record_field_key(types: &Types, record: &str, field: &str) -> String {
    match types.get(record).map(|d| &d.body) {
        Some(TypeBody::Record(r)) => r
            .fields
            .iter()
            .find(|f| f.name.name == field)
            .and_then(|f| f.wire_name()),
        _ => None,
    }
    .unwrap_or_else(|| field.to_string())
}

/// The wire names of a generic sum's declaration.
pub(super) struct SumNames<'a> {
    /// The key the variant travels under.
    pub discriminator: String,
    sum: Option<&'a SumBody>,
}

impl<'a> SumNames<'a> {
    pub(super) fn of(types: &'a Types, name: &str) -> Self {
        let decl = types.get(name);
        SumNames {
            discriminator: decl
                .and_then(|d| d.discriminator())
                .unwrap_or_else(|| WIRE_DISCRIMINANT.to_string()),
            sum: match decl.map(|d| &d.body) {
                Some(TypeBody::Sum(s)) => Some(s),
                _ => None,
            },
        }
    }

    fn variant(&self, variant: &str) -> Option<&'a bynk_syntax::ast::Variant> {
        self.sum?.variants.iter().find(|v| v.name.name == variant)
    }

    /// The value the discriminator takes for `variant`.
    pub(super) fn variant_name(&self, variant: &str) -> String {
        self.variant(variant)
            .and_then(|v| v.wire_name())
            .unwrap_or_else(|| variant.to_string())
    }

    /// The wire key of `variant`'s payload field `field`.
    pub(super) fn field_key(&self, variant: &str, field: &str) -> String {
        self.variant(variant)
            .and_then(|v| v.payload.iter().find(|p| p.name.name == field))
            .and_then(|p| p.wire_name())
            .unwrap_or_else(|| field.to_string())
    }

    /// `variant`'s payload as schema properties, keyed by wire name; every field is required.
    pub(super) fn payload<T>(
        &self,
        variant: &str,
        fields: &[(String, T)],
        mut schema: impl FnMut(&T) -> serde_json::Value,
    ) -> Vec<(String, serde_json::Value, bool)> {
        fields
            .iter()
            .map(|(f, t)| (self.field_key(variant, f), schema(t), true))
            .collect()
    }
}
