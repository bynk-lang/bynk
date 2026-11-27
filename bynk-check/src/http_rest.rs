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
    // Every service of a context is composed into one surface object, so the
    // emitted names are the context's, not a service's. Services in name order,
    // so the route reported is always the same one.
    let mut services: Vec<_> = table.services.values().collect();
    services.sort_by(|a, b| a.name.name.cmp(&b.name.name));
    let mut names: HashMap<String, (&Handler, String)> = HashMap::new();
    for service in services {
        for handler in &service.handlers {
            let HandlerKind::Http { method, path } = &handler.kind else {
                continue;
            };
            check_rest(handler, path, stability, errors);
            let name = crate::emitted_names::http_handler_name(method.as_str(), path);
            let shape = route_shape(path);
            match names.get(&name) {
                // One route spelt twice (`/a`, `/a/`) is `bynk.http.duplicate_route`'s
                // or `bynk.http.unreachable_route`'s.
                Some((_, first_shape)) if *first_shape == shape => {}
                Some((first, _)) => errors.push(name_collision(handler, first, &name)),
                None => {
                    names.insert(name, (handler, shape));
                }
            }
        }
    }
}

/// A route's segments with the empty ones dropped, which is what the router
/// compares: `/a`, `/a/` and `//a` are one route.
fn route_shape(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// #3752 (D6): why `value`, a literal a test case gives for the rest of the route
/// `pattern`, is not a rest the router could bind, or `None` when it is one. The
/// system tier sends it as written after the prefix, so it must be what the
/// router hands a handler: `""` or starting with `/` (never `""` under a root
/// rest, which always binds at least `/`), with no `?` or `#` (they end the path)
/// and no `.` or `..` segment (the router refuses one).
pub fn rest_argument_problem(pattern: &str, value: &str) -> Option<&'static str> {
    let root = route_shape(pattern)
        .split('/')
        .next()
        .is_some_and(crate::route_order::is_rest_segment);
    if value.is_empty() {
        return root.then_some("a root rest always holds at least `/`");
    }
    if !value.starts_with('/') {
        return Some("a rest that isn't empty starts with `/`");
    }
    if value.contains(['?', '#']) {
        return Some("`?` and `#` end a path, so they are never part of a rest");
    }
    let dot = |s: &str| {
        let s = s.to_ascii_lowercase().replace("%2e", ".");
        s == "." || s == ".."
    };
    if value.split('/').any(dot) {
        return Some("the router refuses a `.` or `..` segment");
    }
    None
}

/// #3752: `bynk.test.rest_argument` for each string-literal argument a test
/// case gives a route's rest that [`rest_argument_problem`] refuses.
pub(crate) fn check_rest_arguments(
    pattern: &str,
    params: &[bynk_syntax::ast::Param],
    args: &[bynk_syntax::ast::Expr],
    errors: &mut Vec<CompileError>,
) {
    let Some(name) = rest_name(pattern) else {
        return;
    };
    for (param, arg) in params.iter().zip(args) {
        if param.name.name != name {
            continue;
        }
        if let bynk_syntax::ast::ExprKind::StrLit(value) = &arg.kind
            && let Some(why) = rest_argument_problem(pattern, value)
        {
            errors.push(
                CompileError::new(
                    "bynk.test.rest_argument",
                    arg.span,
                    format!("`\"{value}\"` is not a rest the route `{pattern}` could bind: {why}"),
                )
                .with_note("a rest is the rest of the path as the router receives it, so give it as a request would carry it: `\"\"`, or `\"/a/b.png\"`; at the `system` tier it is sent as written"),
            );
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
        format!("this route's handler would be emitted as `{name}`, the same name as another route's in this context"),
    )
    .with_label(first.span, "this route has that name too")
    .with_note("`/` and every character outside `A-Z`, `a-z` and `0-9` are written `_` in the emitted name, so `/a-b`, `/a_b` and `/a/b` collide; spell one route differently")
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

    // #3752 (review of #4559): a case's rest is what the router could bind.
    #[test]
    fn a_rest_argument_is_one_the_router_could_bind() {
        let p = |pat, v| rest_argument_problem(pat, v);
        assert_eq!(p("/raw/*path", ""), None);
        assert_eq!(p("/raw/*path", "/a//b/"), None);
        assert_eq!(p("/raw/*path", "/caf%C3%A9"), None);
        assert!(p("/raw/*path", "a/b").is_some());
        assert!(p("/*path", "").is_some());
        assert_eq!(p("/*path", "/"), None);
        assert!(p("/raw/*path", "/a?b").is_some());
        assert!(p("/raw/*path", "/a#b").is_some());
        assert!(p("/raw/*path", "/a/../b").is_some());
        assert!(p("/raw/*path", "/%2E%2e").is_some());
        assert_eq!(p("/raw/*path", "/a..b/.env"), None);
    }
}
