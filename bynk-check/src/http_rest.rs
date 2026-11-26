//! #3752 (pass-through HTTP, slice 1; D2–D6): the `*name` rest segment, and the
//! emitted-name check every HTTP handler of a service now passes.
//!
//! A rest is a route's last segment, `*` then a name. It matches zero or more
//! remaining segments and binds them, still percent-encoded, as a `String`
//! (`design/tracks/pass-through-http.md` §5). The rules here are the ones
//! [`crate::context_checks`] can't state segment by segment: the rest's name and
//! type, the experimental gate, and the handler names.
//!
//! The per-segment walk in `validate_http_handler` reads a final rest as a path
//! parameter, and refuses `*` anywhere else (`bynk.http.reserved_route_syntax`).

use crate::stability::UnitStability;
use crate::symbols::UnitTable;
use bynk_syntax::CompileError;
use bynk_syntax::ast::{BaseType, Handler, HandlerKind, TypeRef};
use std::collections::HashMap;

/// The `[unstable] allow` entry a rest segment needs while it is experimental.
pub const REST_ITEM: &str = "bynk.http.rest";

/// The name a route's final `*name` rest binds, if it has one.
pub fn rest_name(path: &str) -> Option<&str> {
    let last = path.split('/').rfind(|s| !s.is_empty())?;
    crate::route_order::is_rest_segment(last).then(|| &last[1..])
}

/// Whether `seg`, a route's last segment, is a bare `*`, refused as
/// `bynk.http.rest_unnamed` rather than as reserved syntax.
pub fn is_unnamed_rest(seg: &str) -> bool {
    seg == "*"
}

/// The rest checks and the emitted-name check, for every HTTP handler of the
/// unit's services.
pub(crate) fn check_rest_routes(
    table: &UnitTable,
    stability: &UnitStability,
    errors: &mut Vec<CompileError>,
) {
    for service in table.services.values() {
        let mut names: HashMap<String, &Handler> = HashMap::new();
        for handler in &service.handlers {
            let HandlerKind::Http { method, path } = &handler.kind else {
                continue;
            };
            check_rest(handler, path, stability, errors);
            let name = crate::emitted_names::http_handler_name(method.as_str(), path);
            match names.get(&name) {
                Some(first) => errors.push(name_collision(handler, first, &name)),
                None => {
                    names.insert(name, handler);
                }
            }
        }
    }
}

fn check_rest(
    handler: &Handler,
    path: &str,
    stability: &UnitStability,
    errors: &mut Vec<CompileError>,
) {
    let last = path.split('/').rfind(|s| !s.is_empty()).unwrap_or("");
    if is_unnamed_rest(last) {
        errors.push(
            CompileError::new(
                "bynk.http.rest_unnamed",
                handler.span,
                format!("the rest segment in the route `{path}` has no name"),
            )
            .with_note("name it, as a `:name` parameter is named (`/files/*key`), and take a `key: String` parameter"),
        );
        return;
    }
    let Some(name) = rest_name(path) else {
        return;
    };
    if let Some(err) = stability.policy.check(REST_ITEM, handler.span) {
        errors.push(err);
    }
    let Some(param) = handler.params.iter().find(|p| p.name.name == name) else {
        // `bynk.http.unbound_path_param` reports it.
        return;
    };
    if !matches!(param.type_ref, TypeRef::Base(BaseType::String, _)) {
        errors.push(
            CompileError::new(
                "bynk.http.rest_param_type",
                param.type_ref.span(),
                format!("the rest parameter `{name}` must be typed `String`"),
            )
            .with_note("a rest binds the rest of the path as it was received, still percent-encoded; decode it with `percentDecode()`, or parse it, in the handler"),
        );
    }
}

fn name_collision(handler: &Handler, first: &Handler, name: &str) -> CompileError {
    CompileError::new(
        "bynk.http.handler_name_collision",
        handler.span,
        format!("this route's handler would be emitted as `{name}`, the same name as another route's in this service"),
    )
    .with_label(first.span, "this route has that name too")
    .with_note("characters outside `A-Z`, `a-z` and `0-9` are written `_` in the emitted name, so `/a-b` and `/a_b` collide; change one route's spelling, or move it to another service")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rest_is_named_and_last() {
        assert_eq!(rest_name("/files/*key"), Some("key"));
        assert_eq!(rest_name("/*path/"), Some("path"));
        assert_eq!(rest_name("/files/*"), None);
        assert_eq!(rest_name("/*a/b"), None);
        assert_eq!(rest_name("/files/:key"), None);
        assert!(is_unnamed_rest("*"));
    }
}
