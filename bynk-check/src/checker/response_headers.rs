//! #3915: the response-header methods of an `HttpResult[T]`: `withHeader(name, value)`, the typed
//! helpers `retryAfter(d)`, `at(url)` and `download(filename)`, and the test accessor `headers()`.
//!
//! A literal name or value is judged here, at compile time, and a computed one is left to the
//! runtime, which faults the request (`500`) instead of sending a malformed header, as a computed
//! cookie does. A helper applies to the variants whose status it belongs to, so it must be written
//! on the variant itself (`TooManyRequests(m).retryAfter(30.seconds)`), after any other header
//! methods; the router cannot tell at compile time which variant a stored `HttpResult` holds.

use bynk_syntax::ast::{BaseType, Expr, ExprKind, Ident};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use super::calls::check_arg;
use super::{Ctx, Ty, TyId};
use crate::kernel_op::HttpResultOp;

/// The headers the router owns, which a handler may not set with `withHeader` (compared in lower
/// case). `access-control-*` is a prefix. The runtime holds the same list
/// (`RESERVED_RESPONSE_HEADERS` in `http.ts`), for a name computed at run time.
const RESERVED: &[&str] = &[
    "age",
    "allow",
    "cache-control",
    "cdn-cache-control",
    "cloudflare-cdn-cache-control",
    "connection",
    "content-disposition",
    "content-length",
    "content-range",
    "content-security-policy",
    "content-type",
    "etag",
    "expires",
    "keep-alive",
    "last-modified",
    "location",
    "pragma",
    "proxy-authenticate",
    "proxy-authorization",
    "referrer-policy",
    "retry-after",
    "set-cookie",
    "strict-transport-security",
    "surrogate-control",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "vary",
    "www-authenticate",
    "x-content-type-options",
];

/// What a helper may be written on, and the helpers' names, for the variant rule.
fn variants_of(op: HttpResultOp) -> &'static [&'static str] {
    match op {
        HttpResultOp::RetryAfter => &["TooManyRequests", "ServiceUnavailable"],
        HttpResultOp::At => &["Created", "RawCreated", "Accepted"],
        HttpResultOp::Download => &["Raw", "RawTagged", "RawStreamed", "PartialContent"],
        HttpResultOp::Heartbeat => &["Streaming"],
        HttpResultOp::Etag | HttpResultOp::LastModified => &["PartialContent", "NotModified"],
        _ => &[],
    }
}

/// Whether `name` is a header the router owns.
pub(crate) fn is_reserved_response_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("access-control-") || RESERVED.contains(&lower.as_str())
}

/// Whether `name` is an RFC 9110 `token`, the shape of a header name.
fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(
                    c,
                    '!' | '#'
                        | '$'
                        | '%'
                        | '&'
                        | '\''
                        | '*'
                        | '+'
                        | '-'
                        | '.'
                        | '^'
                        | '_'
                        | '`'
                        | '|'
                        | '~'
                )
        })
}

/// Whether `tag` is an RFC 9110 entity tag: `"` etagc* `"`, optionally `W/`-prefixed. The runtime's
/// `isEntityTag` is the same test for a tag computed at run time.
fn is_entity_tag(tag: &str) -> bool {
    let body = tag.strip_prefix("W/").unwrap_or(tag);
    body.len() >= 2
        && body.starts_with('"')
        && body.ends_with('"')
        && body[1..body.len() - 1]
            .chars()
            .all(|c| c == '\x21' || ('\x23'..='\x7e').contains(&c))
}

/// Whether `value` holds a character a header value must not carry (CR, LF or another control).
fn has_control(value: &str) -> bool {
    value.chars().any(|c| c.is_ascii_control())
}

/// The variant `receiver` is written as, looking through any other header methods: `Raw(…)` in
/// `Raw(…).withHeader(…).download(…)`.
fn head_variant(receiver: &Expr) -> Option<&str> {
    let mut e = receiver;
    loop {
        match &e.kind {
            ExprKind::MethodCall {
                receiver, method, ..
            } if HttpResultOp::from_name(&method.name).is_some() => e = receiver,
            ExprKind::Call { name, .. } => return Some(name.name.as_str()),
            ExprKind::Ident(id) => return Some(id.name.as_str()),
            _ => return None,
        }
    }
}

/// Whether `op` is one of the response-header methods this module types.
pub(crate) fn is_response_header_op(op: HttpResultOp) -> bool {
    matches!(
        op,
        HttpResultOp::WithHeader
            | HttpResultOp::RetryAfter
            | HttpResultOp::At
            | HttpResultOp::Download
            | HttpResultOp::Heartbeat
            | HttpResultOp::Etag
            | HttpResultOp::LastModified
            | HttpResultOp::Headers
    )
}

fn invalid(span: Span, message: String) -> CompileError {
    CompileError::new("bynk.http.invalid_response_header", span, message)
}

/// Type a response-header method on an `HttpResult[T]`.
pub(crate) fn check_response_header_method(
    op: HttpResultOp,
    method: &Ident,
    receiver: &Expr,
    recv_ty: TyId,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let string = tys.intern(Ty::Base(BaseType::String));
    let duration = tys.intern(Ty::Base(BaseType::Duration));
    let instant = tys.intern(Ty::Base(BaseType::Instant));
    let params: Vec<(TyId, &str)> = match op {
        HttpResultOp::WithHeader => vec![(string, "name"), (string, "value")],
        HttpResultOp::RetryAfter => vec![(duration, "delay")],
        HttpResultOp::At => vec![(string, "url")],
        HttpResultOp::Download => vec![(string, "filename")],
        HttpResultOp::Heartbeat => vec![(duration, "every")],
        HttpResultOp::Etag => vec![(string, "tag")],
        HttpResultOp::LastModified => vec![(instant, "at")],
        _ => vec![],
    };
    if args.len() != params.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`HttpResult.{}` takes {} argument{}, got {}",
                method.name,
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = super::type_of(a, None, ctx);
        }
        return None;
    }
    for (a, (ty, what)) in args.iter().zip(&params) {
        check_arg(
            a,
            *ty,
            &format!("the `HttpResult.{}` {what}", method.name),
            ctx,
        );
    }
    let wanted = variants_of(op);
    if !wanted.is_empty() && !head_variant(receiver).is_some_and(|v| wanted.contains(&v)) {
        let list = wanted
            .iter()
            .map(|v| format!("`{v}`"))
            .collect::<Vec<_>>()
            .join(", ");
        ctx.errors.push(
            CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "`{}` applies to a {list} result, written on the variant itself",
                    method.name
                ),
            )
            .with_note(format!(
                "write `{}(…).{}(…)`, optionally after other header methods; a stored `HttpResult` does not say which variant it holds",
                wanted[0], method.name
            )),
        );
    }
    lint_literals(op, args, ctx);
    Some(match op {
        HttpResultOp::Headers => {
            let list = tys.intern(Ty::List(string));
            tys.intern(Ty::Map(string, list))
        }
        _ => recv_ty,
    })
}

/// The compile-time judgement of a literal argument.
fn lint_literals(op: HttpResultOp, args: &[Expr], ctx: &mut Ctx) {
    match op {
        HttpResultOp::WithHeader => {
            if let Some(ExprKind::StrLit(name)) = args.first().map(|a| &a.kind) {
                let span = args[0].span;
                if !is_token(name) {
                    ctx.errors.push(invalid(
                        span,
                        format!("`{name}` is not a valid header name (an RFC 9110 token)"),
                    ));
                } else if is_reserved_response_header(name) {
                    ctx.errors.push(
                        CompileError::new(
                            "bynk.http.reserved_response_header",
                            span,
                            format!("`{name}` is a header the router owns, so `withHeader` cannot set it"),
                        )
                        .with_note(reserved_hint(name)),
                    );
                }
            }
            if let Some(ExprKind::StrLit(value)) = args.get(1).map(|a| &a.kind)
                && (has_control(value) || !value.is_ascii())
            {
                ctx.errors.push(invalid(
                    args[1].span,
                    "a header value is ASCII with no line break or other control character"
                        .to_string(),
                ));
            }
        }
        HttpResultOp::RetryAfter => {
            if let Some(ExprKind::DurationLit { millis, .. }) = args.first().map(|a| &a.kind)
                && *millis < 1_000
            {
                ctx.errors.push(
                    invalid(
                        args[0].span,
                        "`retryAfter` is whole seconds, so a delay under one second cannot be sent"
                            .to_string(),
                    )
                    .with_note("write at least `1.second`; a shorter wait is not expressible in `Retry-After`"),
                );
            }
        }
        HttpResultOp::At => {
            if let Some(ExprKind::StrLit(url)) = args.first().map(|a| &a.kind)
                && (url.is_empty() || has_control(url))
            {
                ctx.errors.push(invalid(
                    args[0].span,
                    "`at` needs a non-empty URL with no line break or control character"
                        .to_string(),
                ));
            }
        }
        HttpResultOp::Heartbeat => {
            if let Some(ExprKind::DurationLit { millis, .. }) = args.first().map(|a| &a.kind)
                && *millis < 1_000
            {
                ctx.errors.push(
                    invalid(
                        args[0].span,
                        "`heartbeat` needs an interval of at least one second".to_string(),
                    )
                    .with_note("a comment line every few hundred milliseconds is a busy loop, not a keep-alive; 15.seconds is typical"),
                );
            }
        }
        HttpResultOp::Etag => {
            if let Some(ExprKind::StrLit(tag)) = args.first().map(|a| &a.kind)
                && !is_entity_tag(tag)
            {
                ctx.errors.push(
                    invalid(
                        args[0].span,
                        format!("`{tag}` is not an entity tag"),
                    )
                    .with_note("an entity tag is a quoted string, optionally prefixed `W/` (RFC 9110 §8.8.3), such as \"v1\" or W/\"v1\""),
                );
            }
        }
        HttpResultOp::Download => {
            if let Some(ExprKind::StrLit(name)) = args.first().map(|a| &a.kind)
                && name.is_empty()
            {
                ctx.errors.push(invalid(
                    args[0].span,
                    "`download` needs a non-empty filename".to_string(),
                ));
            }
        }
        _ => {}
    }
}

/// Where the router's own header is set instead.
fn reserved_hint(name: &str) -> &'static str {
    match name.to_ascii_lowercase().as_str() {
        "set-cookie" => "set a cookie with `withCookie`",
        "location" => "use `.at(url)` on a `Created`, or a redirect variant",
        "retry-after" => "use `.retryAfter(delay)` on a `TooManyRequests` or `ServiceUnavailable`",
        "content-disposition" => "use `.download(filename)` on a `Raw` result",
        "expires"
        | "age"
        | "pragma"
        | "cdn-cache-control"
        | "cloudflare-cdn-cache-control"
        | "surrogate-control" => "caching headers come from `@cache` and the router",
        "etag" | "last-modified" => {
            "a `PartialContent` or `NotModified` takes its own with `.etag(tag)` and `.lastModified(at)`; the router sets it elsewhere"
        }
        "cache-control" | "vary" => "caching headers come from `@cache` and the router",
        "content-type" | "content-length" => "the body and its media type decide these",
        "x-content-type-options"
        | "strict-transport-security"
        | "content-security-policy"
        | "referrer-policy" => "the service's `security { }` section sets this",
        "www-authenticate" | "allow" => "the router sets this where it answers `401` or `405`",
        _ if name.to_ascii_lowercase().starts_with("access-control-") => {
            "the service's `cors { }` section sets the CORS headers"
        }
        _ => "this is a hop-by-hop header the platform manages",
    }
}

#[cfg(test)]
mod tests {
    use super::RESERVED;

    /// The compile-time list and the runtime's `RESERVED_RESPONSE_HEADERS` must name the same
    /// headers, so a literal and a computed name are judged alike.
    #[test]
    fn the_reserved_lists_match_the_runtime() {
        let src = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../bynk-emit/runtime/src/http.ts"
        ))
        .expect("the runtime source is in the workspace");
        let start = src
            .find("RESERVED_RESPONSE_HEADERS: ReadonlySet<string> = new Set([")
            .expect("the runtime declares the reserved list");
        let body = &src[start..];
        let body = &body[..body.find("]);").expect("the list ends")];
        let ts: Vec<&str> = body
            .lines()
            .filter_map(|l| l.trim().strip_prefix('"')?.strip_suffix("\","))
            .collect();
        assert_eq!(ts, RESERVED, "the runtime and checker lists differ");
    }
}
