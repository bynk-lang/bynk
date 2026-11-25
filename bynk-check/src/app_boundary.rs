//! The boundary rule for `Name[Arg, …]`, an application of a generic record,
//! split out of `context_checks::reject_fn_types`.
//!
//! #4192: `MapEntry[K, V]`, the prelude record `map.entries()` yields and
//! `Map.of` takes, is a value built and consumed in place, so, like a `Query`, it
//! is refused wherever a type is persisted or sent. It has no declaration to
//! build a wire codec from.

use bynk_syntax::ast::{Ident, TypeDecl, TypeRef};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;
use std::collections::HashMap;
use std::sync::Arc;

use crate::builtin_names::types::MAP_ENTRY;
use crate::context_checks::{
    recursive_generic_at_boundary, reject_fn_types, stream_carrying_at_boundary, stream_passes,
};

/// Check a generic-record application written in a boundary position.
pub(crate) fn reject_app(
    name: &Ident,
    args: &[TypeRef],
    span: Span,
    what: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    if name.name == MAP_ENTRY && !types.contains_key(&name.name) {
        errors.push(map_entry_at_boundary(span, what));
    }
    if !stream_passes(what)
        && let Some(rg) = bynk_syntax::ast::recursive_generic_path(
            &TypeRef::App {
                name: name.clone(),
                args: Vec::new(),
                span,
            },
            types,
        )
    {
        errors.push(recursive_generic_at_boundary(&rg, span, what));
    }
    for a in args {
        reject_fn_types(a, what, types, errors);
    }
    // K-streams (#2623): a generic record whose own fields carry a stream
    // (`Wrap[T] = { s: Stream[T] }`) is found through its declaration; a stream
    // argument was reported just above.
    if !stream_passes(what)
        && let Some(path) = bynk_syntax::ast::stream_path(&TypeRef::Named(name.clone()), types)
    {
        errors.push(stream_carrying_at_boundary(&name.name, &path, span, what));
    }
}

fn map_entry_at_boundary(span: Span, what: &str) -> CompileError {
    CompileError::new(
        "bynk.types.map_entry_at_boundary",
        span,
        format!(
            "a `MapEntry` type cannot appear in {what} — a map entry is built and consumed in place, never persisted or sent across a boundary"
        ),
    )
    .with_note("use a record with the fields you need (`type Pair = { key: K, value: V }`) at a boundary, and convert with `map`")
}
