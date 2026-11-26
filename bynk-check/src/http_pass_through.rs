//! #3752 (pass-through HTTP, slice 3; D7–D11): the `on ANY(…)` pass-through
//! route, and the `bynk` types it is written with. Its rules are checked here,
//! beside the ordinary route's in [`crate::context_checks`].
//!
//! The types (`InboundRequest`, `Forwarded`, `PassThrough`, `UpstreamError`) and
//! the `Upstream` capability are the `bynk` unit's, imported with
//! `consumes bynk { … }` like `Secret`. Each is found by its declaration, never by
//! its name alone, so a type of the program's own called `PassThrough` is just
//! that.

use crate::stability::UnitStability;
use crate::symbols::UnitTable;
use bynk_syntax::CompileError;
use bynk_syntax::ast::{Handler, HandlerKind, HttpMethod, Param, TypeDecl, TypeRef};
use std::collections::HashMap;
use std::sync::Arc;

/// The `[unstable] allow` entry an `ANY` route needs while it is experimental.
pub const ANY_ITEM: &str = "bynk.http.ANY";

/// One of the `bynk` unit's pass-through types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassThroughType {
    InboundRequest,
    Forwarded,
    PassThrough,
    UpstreamError,
}

impl PassThroughType {
    fn of_name(name: &str) -> Option<Self> {
        Some(match name {
            "InboundRequest" => Self::InboundRequest,
            "Forwarded" => Self::Forwarded,
            "PassThrough" => Self::PassThrough,
            "UpstreamError" => Self::UpstreamError,
            _ => return None,
        })
    }
}

/// Which pass-through type `decl` is, when it is the `bynk` unit's own
/// declaration of one.
pub fn pass_through_type_of(decl: &TypeDecl) -> Option<PassThroughType> {
    let unit = crate::firstparty::unit(bynk_std::BYNK_UNIT)?;
    if decl.span.file != crate::project_model::firstparty_file_id(unit) {
        return None;
    }
    PassThroughType::of_name(&decl.name.name)
}

/// Which pass-through type `name` names in the project's type table, if any.
pub fn pass_through_type(
    name: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<PassThroughType> {
    types.get(name).and_then(|d| pass_through_type_of(d))
}

/// Whether `r` is `Effect[PassThrough]`, the `bynk` unit's `PassThrough`.
pub fn returns_pass_through(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    matches!(r, TypeRef::Effect(inner, _)
        if matches!(inner.as_ref(), TypeRef::Named(id)
            if pass_through_type(&id.name, types) == Some(PassThroughType::PassThrough)))
}

/// Whether `r` is the `bynk` unit's `InboundRequest`.
pub fn is_inbound_request(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    matches!(r, TypeRef::Named(id)
        if pass_through_type(&id.name, types) == Some(PassThroughType::InboundRequest))
}

/// Whether the route's return type is for the pass-through rules to judge: an
/// `ANY` route's, or an ordinary route's that returns `Effect[PassThrough]`.
/// `validate_http_handler` skips its own return check for these.
pub fn owns_return_check(
    method: HttpMethod,
    r: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> bool {
    method == HttpMethod::Any || returns_pass_through(r, types)
}

/// Whether `p` is for the pass-through rules to judge (an `ANY` route's
/// `request`, or an `InboundRequest` anywhere), so `validate_http_handler`
/// doesn't also call it an extra parameter.
pub fn owns_param(method: HttpMethod, p: &Param, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    (method == HttpMethod::Any && p.name.name == "request")
        || is_inbound_request(&p.type_ref, types)
}

/// The pass-through checks, for every HTTP handler of the unit's services.
pub(crate) fn check_pass_through_routes(
    table: &UnitTable,
    types: &HashMap<String, Arc<TypeDecl>>,
    stability: &UnitStability,
    errors: &mut Vec<CompileError>,
) {
    for service in table.services.values() {
        let has_any = service.handlers.iter().any(is_any_route);
        if has_any && let Some(cors) = &service.cors {
            errors.push(
                CompileError::new(
                    "bynk.http.cors_with_pass_through",
                    cors.span,
                    format!(
                        "the service `{}` declares a pass-through route, so it may not declare `cors {{ }}`",
                        service.name.name
                    ),
                )
                .with_note("a forwarded response carries the upstream's own CORS headers, and two policies on one response can't both be right; put the proxy in its own `from http` service"),
            );
        }
        for handler in &service.handlers {
            let HandlerKind::Http { method, .. } = &handler.kind else {
                continue;
            };
            if *method == HttpMethod::Any {
                if let Some(err) = stability.policy.check(ANY_ITEM, handler.span) {
                    errors.push(err);
                }
                check_any_route(handler, types, errors);
            } else {
                check_ordinary_route(handler, *method, types, errors);
            }
        }
    }
}

fn is_any_route(h: &Handler) -> bool {
    matches!(
        h.kind,
        HandlerKind::Http {
            method: HttpMethod::Any,
            ..
        }
    )
}

fn signature(span: bynk_syntax::span::Span, message: String, note: String) -> CompileError {
    CompileError::new("bynk.http.pass_through_signature", span, message).with_note(note)
}

/// An `ANY` route: it returns `Effect[PassThrough]`, takes only path parameters,
/// a rest, `request: InboundRequest` and `cf`, and isn't cached by `@cache`.
fn check_any_route(
    handler: &Handler,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    if !returns_pass_through(&handler.return_type, types) {
        errors.push(signature(
            handler.return_type.span(),
            format!(
                "an `on ANY` route must return `Effect[PassThrough]`, but returns `{}`",
                crate::context_checks::ts_type_ref_display(&handler.return_type)
            ),
            "`ANY` declares a pass-through route: it answers `Forward(response)` from `Upstream.forward`, or `Respond(result)` with an `HttpResult[()]`; `PassThrough` is `bynk`'s (`consumes bynk { PassThrough }`)".to_string(),
        ));
    }
    for p in &handler.params {
        let name = p.name.name.as_str();
        let refused = match name {
            "body" => Some(
                "the request body belongs to the upstream, which `Upstream.forward` streams it to",
            ),
            "headers" => Some("read the headers as `request.headers()`"),
            "query" => Some("read the raw query string as `request.query()`"),
            "cookies" => Some("read the `Cookie` header as `request.headers().get(\"cookie\")`"),
            "request" if !is_inbound_request(&p.type_ref, types) => Some(
                "`request` is the route's `InboundRequest` (`consumes bynk { InboundRequest }`)",
            ),
            _ => None,
        };
        if let Some(why) = refused {
            errors.push(signature(
                p.span,
                format!("an `on ANY` route can't take the parameter `{name}` here"),
                format!("a pass-through route takes its path parameters, a rest, `request: InboundRequest` and `cf`; {why}"),
            ));
        } else if is_inbound_request(&p.type_ref, types) && name != "request" {
            errors.push(not_pass_through(p, "it must be named `request`"));
        }
    }
    for ann in &handler.annotations {
        match ann.name.name.as_str() {
            "cache" => errors.push(
                CompileError::new(
                    "bynk.http.cache_on_pass_through",
                    ann.span,
                    "`@cache` can't be put on an `on ANY` route",
                )
                .with_note("the upstream's own `Cache-Control` is a forwarded answer's freshness"),
            ),
            "raw" | "form" | "multipart" => errors.push(signature(
                ann.span,
                format!("`@{}` can't be put on an `on ANY` route", ann.name.name),
                "it reads the request body, which belongs to the upstream: `Upstream.forward` streams it".to_string(),
            )),
            _ => {}
        }
    }
}

/// An ordinary route: it may not return `Effect[PassThrough]` or take an
/// `InboundRequest`.
fn check_ordinary_route(
    handler: &Handler,
    method: HttpMethod,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    if returns_pass_through(&handler.return_type, types) {
        errors.push(signature(
            handler.return_type.span(),
            format!(
                "only an `on ANY` route returns `Effect[PassThrough]`; `on {}` returns `Effect[HttpResult[T]]`",
                method.as_str()
            ),
            "a pass-through route must receive every method, `HEAD` and `OPTIONS` included, so it is declared with `ANY`".to_string(),
        ));
    }
    for p in &handler.params {
        if is_inbound_request(&p.type_ref, types) {
            errors.push(not_pass_through(p, "only an `on ANY` route receives one"));
        }
    }
}

fn not_pass_through(p: &Param, why: &str) -> CompileError {
    CompileError::new(
        "bynk.http.inbound_request_not_pass_through",
        p.type_ref.span(),
        format!("the parameter `{}` can't be an `InboundRequest`: {why}", p.name.name),
    )
    .with_note("an `InboundRequest` is what a pass-through route forwards; an ordinary route reads its request through `headers`, `query`, `cookies` and `body`")
}

/// #3752 (D13): an `Upstream`'s `@timeout`, a positive whole number of seconds
/// (the head's deadline; the body streams after it).
pub(crate) fn upstream_timeout(
    kind: &bynk_syntax::ast::ExprKind,
    option: &str,
    span: bynk_syntax::span::Span,
    path: &std::path::Path,
    errors: &mut crate::project_model::ErrorSink,
) -> Option<i64> {
    match kind {
        bynk_syntax::ast::ExprKind::DurationLit { millis, .. }
            if *millis > 0 && millis % 1000 == 0 =>
        {
            Some(*millis / 1000)
        }
        _ => {
            errors.push_for(
                Some(path),
                CompileError::new(
                    "bynk.capability.invalid_option",
                    span,
                    format!("`@{option}` takes a positive whole number of seconds"),
                )
                .with_note("write `@timeout(30.seconds)`, for example: it bounds the wait for the upstream's response head"),
            );
            None
        }
    }
}

/// #3752 (D13): whether `origin`, an `Upstream`'s `@origin`, is a serialised
/// origin (`scheme://host[:port]`, lower case, nothing after it), on `https`, or
/// on `http` for a loopback host only. Pushes the error when it isn't.
pub(crate) fn upstream_origin_ok(
    origin: &str,
    span: bynk_syntax::span::Span,
    path: &std::path::Path,
    errors: &mut crate::project_model::ErrorSink,
) -> bool {
    let Some(reason) = upstream_origin_problem(origin) else {
        return true;
    };
    errors.push_for(
        Some(path),
        CompileError::new(
            "bynk.capability.invalid_option",
            span,
            format!("`@origin(\"{origin}\")` {reason}"),
        )
        .with_note("an upstream is a serialised origin, `https://host[:port]`, or `http://` for `localhost`, `127.0.0.1` or `[::1]`; a pass-through route's path follows it"),
    );
    false
}

/// Why `origin` is not an upstream origin, or `None` when it is one.
pub fn upstream_origin_problem(origin: &str) -> Option<String> {
    if origin == "*" || origin.eq_ignore_ascii_case("null") {
        return Some("is not one origin".to_string());
    }
    if let Some(problem) = crate::cors_origin::check(origin) {
        return Some(problem.reason);
    }
    if origin.starts_with("https://") {
        return None;
    }
    let loopback = origin.strip_prefix("http://").is_some_and(|rest| {
        let host = match rest.strip_prefix("[::1]") {
            Some(port) => return port.is_empty() || port.starts_with(':'),
            None => rest.split(':').next().unwrap_or(rest),
        };
        host == "localhost" || host == "127.0.0.1"
    });
    if loopback {
        None
    } else {
        Some("is not `https`: only a loopback host (`localhost`, `127.0.0.1`, `[::1]`) may be reached over `http`".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::upstream_origin_problem as problem;

    // §8 "SSRF to internal addresses": `http` only for a loopback host.
    #[test]
    fn an_upstream_origin_is_https_or_loopback_http() {
        assert_eq!(problem("https://bynk-blog.pages.dev"), None);
        assert_eq!(problem("https://upstream.example:8443"), None);
        assert_eq!(problem("http://127.0.0.1:8787"), None);
        assert_eq!(problem("http://localhost:3000"), None);
        assert_eq!(problem("http://[::1]:9000"), None);
        assert!(problem("http://upstream.example").is_some());
        assert!(problem("http://10.0.0.1").is_some());
        assert!(problem("http://localhost.evil.example").is_some());
        assert!(problem("https://upstream.example/blog").is_some(), "a path");
        assert!(
            problem("https://upstream.example/").is_some(),
            "a trailing slash"
        );
        assert!(
            problem("https://user@upstream.example").is_some(),
            "user information"
        );
        assert!(problem("HTTPS://Upstream.example").is_some(), "upper case");
        assert!(problem("*").is_some());
        assert!(problem("null").is_some());
        assert!(problem("upstream.example").is_some(), "no scheme");
    }
}

/// The held type (`InboundRequest` or `Forwarded`) that the named type `name`
/// is, or holds at any depth in a record field or variant payload, as a
/// `Stream` is found in a type that carries one. `bynk`'s own `PassThrough`
/// holds a `Forwarded`, which is what a pass-through route returns.
pub fn held_in(name: &str, types: &HashMap<String, Arc<TypeDecl>>) -> Option<&'static str> {
    fn walk(
        name: &str,
        types: &HashMap<String, Arc<TypeDecl>>,
        seen: &mut std::collections::HashSet<String>,
    ) -> Option<&'static str> {
        let decl = types.get(name)?;
        match pass_through_type_of(decl) {
            Some(PassThroughType::InboundRequest) => return Some("InboundRequest"),
            Some(PassThroughType::Forwarded) => return Some("Forwarded"),
            _ => {}
        }
        if !seen.insert(name.to_string()) {
            return None;
        }
        let vars: std::collections::HashSet<&str> = decl
            .type_params
            .iter()
            .map(|p| p.name.name.as_str())
            .collect();
        let mut names = Vec::new();
        match &decl.body {
            bynk_syntax::ast::TypeBody::Record(r) => r
                .fields
                .iter()
                .for_each(|f| crate::project_model::type_ref_names(&f.type_ref, &vars, &mut names)),
            bynk_syntax::ast::TypeBody::Sum(sum) => sum.variants.iter().for_each(|v| {
                v.payload.iter().for_each(|p| {
                    crate::project_model::type_ref_names(&p.type_ref, &vars, &mut names)
                })
            }),
            _ => {}
        }
        names.iter().find_map(|n| walk(n, types, seen))
    }
    walk(name, types, &mut std::collections::HashSet::new())
}

/// `bynk.types.inbound_request_at_boundary` for `id`, which is or holds the held
/// type `held`, written in `what`.
pub(crate) fn held_at_boundary(
    id: &bynk_syntax::ast::Ident,
    held: &str,
    what: &str,
) -> CompileError {
    let article = if held == "InboundRequest" { "an" } else { "a" };
    let subject = if id.name == held {
        format!("{article} `{held}`")
    } else {
        format!("`{}`, which holds {article} `{held}`,", id.name)
    };
    let (is, instead) = if held == "Forwarded" {
        (
            "an upstream's answer, streaming through the route that forwarded it",
            "answer it with `Forward(response)`, or read what you need (`response.status()`, `response.header(\"…\")`) and store or send that",
        )
    } else {
        (
            "the request a pass-through route is answering, which only `Upstream.forward` sends on",
            "read what you need (`request.method()`, `request.headers().get(\"…\")`, `request.query()`) and store or send that",
        )
    };
    CompileError::new(
        "bynk.types.inbound_request_at_boundary",
        id.span,
        format!("{subject} cannot appear in {what} — {article} `{held}` is {is}, never persisted or sent"),
    )
    .with_note(instead)
}
