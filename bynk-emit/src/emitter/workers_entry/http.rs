//! The external HTTP routes of the Worker entry: the route table and the per-route dispatch.

use super::field_specs::{
    FieldSpec, MultipartSpec, cookies_field_specs, effective_max_body, field_specs_exprs,
    form_field_specs, headers_field_specs, multipart_spec, query_field_specs,
};
use super::http_policy::param_count;
use super::respond::{
    json_error_kind, json_response, null_response, rejection_renderer, set_fault_route,
    try_catch_stmt,
};
use super::wire::{deserialise_call, http_result_inner, wire_ident};
use super::{consumed_credential_headers, rejected_surface, signed_cookie_name};
use crate::emitter::{RuntimeUse, http_handler_method_name_ir};
use crate::ts_build::{
    and_expr, as_expr, await_expr, binary, block, bool_lit, call, cond_expr, const_, ident, if_,
    index, let_typed, member, method_call, new, null_lit, num_lit, or_expr, paren, return_,
    str_lit, strict_eq, strict_neq,
};
use bynk_check::symbols::UnitTable;
use bynk_syntax::ast::{BaseType, Handler, TypeRef};
use bynk_ts::{TsExpr, TsStmt, TsType, TsTypeMember};

#[derive(Debug, Clone)]
pub(super) struct HttpRoute {
    pub(super) service: String,
    pub(super) method: bynk_model::IrHttpMethod,
    pub(super) path: String,
    pub(super) handler: Handler,
    /// v0.47: the handler's `by` clause names a Bearer actor, so its surface
    /// wrapper runs the verification seam and takes the request as its first
    /// argument.
    pub(super) bearer: bool,
    /// v0.151: the handler's `by` clause names an `Oidc` actor — its surface
    /// wrapper runs the JWKS verification seam and takes the request first, like
    /// Bearer.
    pub(super) oidc: bool,
    /// #3386: the handler's `by` clause names a `SignedCookie` actor, whose session
    /// cookie (this name) the wrapper verifies and the handler's `cookies` view omits.
    /// Its wrapper takes the request first, like Bearer.
    pub(super) signed_cookie: Option<String>,
    /// #3994: the credential headers the route's seam consumes (`authorization`, a signature
    /// header); the handler's `RequestHeaders` omits them.
    pub(super) hidden_headers: Vec<String>,
    /// v0.51: the handler's `by` clause names a Signature actor — the entry
    /// dispatch reads the raw body, verifies the HMAC, and parses the body from
    /// those same bytes.
    pub(super) signature: Option<bynk_check::actors::SignatureSeam>,
    /// #3914: the `WWW-Authenticate` challenges any `401` of this route carries, one per
    /// actor of its `by` clause, in order.
    pub(super) challenges: Vec<String>,
    /// v0.52: the handler's `by` clause names a multi-actor sum — its wrapper
    /// owns the boundary, so the entry passes `request` (+ path params) and does
    /// not read or parse the body itself.
    pub(super) sum: bool,
    /// v0.140 (ADR 0163): the GET handler's `@cache` freshness policy, if declared.
    /// `Some` only for a `GET` carrying a well-formed `@cache`; drives the
    /// `applyCache` `Cache-Control` stamp. The conditional `ETag`/`304` half is
    /// automatic for every eligible GET and needs no policy.
    pub(super) cache: Option<bynk_model::CacheIr>,
    /// L7 (#2414, Decision C): the context declares an `EdgeCache` instance,
    /// so its Worker is served through Workers Cache: a `@cache` route stamps
    /// its freshness on a `2xx` alone (`applyEdgeCache`), and a `GET` without
    /// `@cache` says `no-store`.
    pub(super) edge_cache: bool,
    /// Review of #2417: an opted-in Worker that negotiates its locale from
    /// `Accept-Language` names that header in a `@cache` route's `Vary`, so
    /// the edge keeps one response per language.
    pub(super) vary_locale: bool,
    /// v0.142 (ADR 0165): the route's effective request-body ceiling in bytes —
    /// the handler's `@limit(maxBody:)` if present, else the service's
    /// `limits { maxBody }`, else `None` (no cap). `Some` emits a `Content-Length`
    /// fast-reject to a synthesised `413` before the body is read; `None` leaves
    /// the route byte-for-byte unchanged (opt-in, DECISION E).
    pub(super) max_body: Option<i64>,
    /// The media type of a `@raw(contentType:)` handler: its `body: Bytes` is the
    /// request's octets as read, undecoded, and the router answers `415` to any
    /// other `Content-Type` (`"*/*"` accepts all). A raw route always has a cap
    /// (`bynk.http.raw_body_unbounded`), so it always takes the counted read.
    pub(super) raw: Option<Vec<String>>,
    /// #2858: the route writes `Option` as plain JSON (`v` / `null`) in its
    /// response body — its `@json(options:)`, else the service's `json { }`.
    pub(super) json_plain: bool,
    /// #2341 (decision C): the compose-surface method of the service's
    /// `on rejected` (`rejected_<service>`), when it declares one. The route's
    /// synthesised rejections are rendered through it.
    pub(super) rejected: Option<String>,
    /// #2854: the fields of the handler's typed `query` record, as the runtime's
    /// `queryObject` reads them off the query string: `(name, wire kind, list)`.
    /// `None` when the route has no typed `query` (a raw `RequestQuery` is not
    /// decoded).
    pub(super) query_fields: Option<Vec<FieldSpec>>,
    /// #3276: the fields of a `@form` handler's `body` record, as the runtime's
    /// `formObject` reads them off an `application/x-www-form-urlencoded` body:
    /// `(name, wire kind, list, option)`, as `query_fields`. `Some` marks the
    /// route's `body` as a form: the router checks the `Content-Type` (`415`) and
    /// decodes the body through the same record codec a JSON body takes.
    pub(super) form_fields: Option<Vec<FieldSpec>>,
    /// #3277: the fields of the handler's `cookies` record, as the runtime's
    /// `cookieObject` reads them off the `Cookie` header: `(name, wire kind, list,
    /// option)`. `None` when the route takes no `cookies` parameter.
    pub(super) cookies_fields: Option<Vec<FieldSpec>>,
    /// #3911: the fields of the handler's typed `headers` record, as the runtime's
    /// `headersObject` reads them off the request's headers. `None` when the route takes
    /// the raw `RequestHeaders` view or no `headers` parameter.
    pub(super) headers_fields: Option<Vec<FieldSpec>>,
    /// #3276: a `@multipart` route's body fields (as `form_fields`, with the `File`
    /// fields kind `"file"`) and its per-file and part-count caps.
    pub(super) multipart: Option<MultipartSpec>,
}

/// #3994: the cookie argument of `__requestHeaders` when only headers are hidden: no cookie.
fn no_cookie_arg() -> TsExpr {
    ident("undefined")
}

/// #2341: the `RequestHeaders` a handler is passed. #3386: on a `SignedCookie` route the session
/// cookie is hidden from its `Cookie` header, as it is from the `cookies` views.
fn request_headers_arg(route: &HttpRoute) -> TsExpr {
    let mut args = vec![ident("request")];
    let cookie = route
        .signed_cookie
        .as_deref()
        .map(|h| str_lit(crate::emitter::escape_ts_string(h)));
    // #3994: a seam's credential header is hidden too (the third argument).
    if cookie.is_some() || !route.hidden_headers.is_empty() {
        args.push(cookie.unwrap_or_else(no_cookie_arg));
    }
    if !route.hidden_headers.is_empty() {
        args.push(TsExpr::Array {
            items: route
                .hidden_headers
                .iter()
                .map(|n| str_lit(crate::emitter::escape_ts_string(n)))
                .collect(),
            multiline: false,
        });
    }
    call(ident("__requestHeaders"), args)
}

/// #3513 C: `cf` is what Cloudflare derived about the connection, read off `request.cf`
/// into the `RequestCf` record; a missing or oddly shaped property is `None`.
fn bind_cf_param(
    route: &HttpRoute,
    p: &bynk_syntax::ast::Param,
    guarded: &mut Vec<TsStmt>,
    call_args: &mut Vec<TsExpr>,
    (cors_const, security_const, rejected): (Option<&str>, Option<&str>, Option<&str>),
    ru: &RuntimeUse,
) -> bool {
    if route.path.split('/').any(|seg| seg == ":cf")
        || !matches!(&p.type_ref, TypeRef::Named(id) if id.name == "RequestCf")
    {
        return false;
    }
    let dser = deserialise_call(
        &p.type_ref,
        &call(ident("__cfObject"), vec![ident("request")]),
        "cf",
        ru,
    );
    guarded.push(const_("__r_cf", dser));
    // `__cfObject` keeps only fields the record accepts, so this does not reject; the arm
    // is what narrows the `Result` for the compiler.
    guarded.push(if_(
        strict_eq(member(ident("__r_cf"), "tag"), str_lit("Err")),
        return_(Some(stamp_rejection(
            json_response(member(ident("__r_cf"), "error"), 400),
            cors_const,
            security_const,
            rejected,
        ))),
    ));
    guarded.push(const_("cf", member(ident("__r_cf"), "value")));
    call_args.push(ident("cf"));
    true
}

/// The request-reading parameters that bind a record: `cf`, and `cookies`.
fn bind_request_record_param(
    route: &HttpRoute,
    p: &bynk_syntax::ast::Param,
    guarded: &mut Vec<TsStmt>,
    call_args: &mut Vec<TsExpr>,
    wrappers: (Option<&str>, Option<&str>, Option<&str>),
    ru: &RuntimeUse,
) -> bool {
    if p.name.name == "cf" {
        return bind_cf_param(route, p, guarded, call_args, wrappers, ru);
    }
    if p.name.name == "headers" {
        return bind_headers_param(route, p, guarded, call_args, wrappers, ru);
    }
    bind_cookies_param(route, p, guarded, call_args, wrappers, ru)
}

/// #3911: bind a handler's typed `headers` record: the request's headers are decoded by
/// `headersObject` (names compare without case, a `List` field splits its comma-separated
/// value) and the record's own codec checks refinements and fills defaults, answering `400`
/// at `headers.<field>` as a `body` does. A header the route's seam consumed reads as absent.
/// Returns whether the parameter was a typed one (the raw `RequestHeaders` view binds below,
/// and a `:headers` path segment is a path parameter instead).
fn bind_headers_param(
    route: &HttpRoute,
    p: &bynk_syntax::ast::Param,
    guarded: &mut Vec<TsStmt>,
    call_args: &mut Vec<TsExpr>,
    (cors_const, security_const, rejected): (Option<&str>, Option<&str>, Option<&str>),
    ru: &RuntimeUse,
) -> bool {
    let Some(fields) = &route.headers_fields else {
        return false;
    };
    let mut args = vec![ident("request"), TsExpr::array(field_specs_exprs(fields))];
    if !route.hidden_headers.is_empty() {
        args.push(TsExpr::array(
            route
                .hidden_headers
                .iter()
                .map(|n| str_lit(crate::emitter::escape_ts_string(n)))
                .collect(),
        ));
    }
    guarded.push(const_("__h", call(ident("__headersObject"), args)));
    guarded.push(if_(
        TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(member(ident("__h"), "ok")),
        },
        return_(Some(stamp_rejection(
            json_response(member(ident("__h"), "error"), 400),
            cors_const,
            security_const,
            rejected,
        ))),
    ));
    let dser = deserialise_call(&p.type_ref, &member(ident("__h"), "value"), "headers", ru);
    guarded.push(const_("__r_headers", dser));
    guarded.push(if_(
        strict_eq(member(ident("__r_headers"), "tag"), str_lit("Err")),
        return_(Some(stamp_rejection(
            json_response(member(ident("__r_headers"), "error"), 400),
            cors_const,
            security_const,
            rejected,
        ))),
    ));
    guarded.push(const_("headers", member(ident("__r_headers"), "value")));
    call_args.push(ident("headers"));
    true
}

/// #3277: bind a handler's `cookies` parameter. `cookies: RequestCookies` (decision A) is the
/// raw view of the request's cookies; a `cookies` record is decoded off the `Cookie` header and
/// validated before the handler runs, answering `400` as a `body` does, with the record's own
/// codec checking refinements and filling defaults. #3386: a route guarded by a `SignedCookie`
/// actor consumes that cookie at the seam, so it is hidden from either form. Returns whether the
/// parameter was a `cookies` one (a `:cookies` path segment is a path parameter instead).
fn bind_cookies_param(
    route: &HttpRoute,
    p: &bynk_syntax::ast::Param,
    guarded: &mut Vec<TsStmt>,
    call_args: &mut Vec<TsExpr>,
    wrappers: (Option<&str>, Option<&str>, Option<&str>),
    ru: &RuntimeUse,
) -> bool {
    if p.name.name != "cookies" || route.path.split('/').any(|seg| seg == ":cookies") {
        return false;
    }
    let hidden = route.signed_cookie.as_deref();
    if matches!(p.type_ref, TypeRef::RequestCookies(_)) {
        let mut args = vec![ident("request")];
        args.extend(hidden.map(|h| str_lit(crate::emitter::escape_ts_string(h))));
        call_args.push(call(ident("__requestCookies"), args));
        return true;
    }
    let Some(fields) = &route.cookies_fields else {
        return false;
    };
    push_cookies_param(
        fields,
        &p.type_ref,
        hidden,
        guarded,
        call_args,
        wrappers,
        ru,
    );
    true
}

/// #3277: the statements that decode a route's `cookies` record off the `Cookie`
/// header and bind it for the handler call: `400` on a rejection, as a `body`
/// answers, with the record's own codec checking refinements and filling defaults.
fn push_cookies_param(
    fields: &[FieldSpec],
    type_ref: &TypeRef,
    hide: Option<&str>,
    guarded: &mut Vec<TsStmt>,
    call_args: &mut Vec<TsExpr>,
    (cors_const, security_const, rejected): (Option<&str>, Option<&str>, Option<&str>),
    ru: &RuntimeUse,
) {
    let spec: Vec<TsExpr> = field_specs_exprs(fields);
    let mut cookie_args = vec![ident("request"), TsExpr::array(spec)];
    // #3386: a `SignedCookie` actor's credential is not the handler's to read.
    if let Some(hidden) = hide {
        cookie_args.push(str_lit(crate::emitter::escape_ts_string(hidden)));
    }
    guarded.push(const_("__c", call(ident("__cookieObject"), cookie_args)));
    guarded.push(if_(
        TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(member(ident("__c"), "ok")),
        },
        return_(Some(stamp_rejection(
            json_response(member(ident("__c"), "error"), 400),
            cors_const,
            security_const,
            rejected,
        ))),
    ));
    let dser = deserialise_call(type_ref, &member(ident("__c"), "value"), "cookies", ru);
    guarded.push(const_("__r_cookies", dser));
    guarded.push(if_(
        strict_eq(member(ident("__r_cookies"), "tag"), str_lit("Err")),
        return_(Some(stamp_rejection(
            json_response(member(ident("__r_cookies"), "error"), 400),
            cors_const,
            security_const,
            rejected,
        ))),
    ));
    guarded.push(const_("cookies", member(ident("__r_cookies"), "value")));
    call_args.push(ident("cookies"));
}

/// Stamp a rejection response with the service's policy the same way a
/// handled response is stamped — `applySecurityHeaders(applyCors(…))` — so a
/// boundary `400`/`401` carries `nosniff` (and CORS, when the service opts in)
/// exactly as the `200`, the `405`, and the `413` do. `inner` is the finished
/// `new Response(…)` TS expression; the wrapping mirrors the happy path
/// (`emit_http_route_dispatch`) and the `413` ceiling above it. ADR 0164 D6:
/// *every response a `from http` route emits carries the policy* — before this,
/// the bare-`new Response` rejection sites skipped the wrapper (#659), leaving
/// `nosniff` off precisely the responses that reflect attacker input.
fn stamp_rejection(
    inner: TsExpr,
    cors_const: Option<&str>,
    security_const: Option<&str>,
    // #2341 (decision C): the service's `on rejected` surface method, if it
    // declares one — the default response is rendered through it first, and
    // the headers are stamped on what it returns.
    rejected: Option<&str>,
) -> TsExpr {
    let inner = match rejected {
        Some(m) => await_expr(call(
            ident("__renderRejection"),
            vec![inner, rejection_renderer(m)],
        )),
        None => inner,
    };
    let corsed = match cors_const {
        Some(c) => call(
            ident("__applyCors"),
            vec![
                inner,
                ident(c.to_string()),
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit("origin")],
                ),
            ],
        ),
        None => inner,
    };
    match security_const {
        Some(s) => call(
            ident("__applySecurityHeaders"),
            vec![corsed, ident(s.to_string())],
        ),
        None => corsed,
    }
}

/// The first statements of a matched route's block: the fault-log route name
/// (#1825), then, for a route with path parameters, #3908's `400 MalformedPath`:
/// a path whose percent-encoding does not decode is the client's error and no
/// fault, answered before the actor, body or handler.
fn route_prelude(
    route: &HttpRoute,
    (cors_const, security_const, rejected): (Option<&str>, Option<&str>, Option<&str>),
) -> Vec<TsStmt> {
    // #1825: names the route, by its pattern, for the fault log.
    let mut out = vec![set_fault_route(format!(
        "{} {}",
        route.method.as_str(),
        route.path
    ))];
    if param_count(&route.path) == 0 {
        return out;
    }
    let malformed = stamp_rejection(
        json_response(
            json_error_kind(
                "MalformedPath",
                vec![("details".to_string(), member(ident("__m"), "malformed"))],
            ),
            400,
        ),
        cors_const,
        security_const,
        rejected,
    );
    out.push(if_(
        member(ident("__m"), "malformed"),
        block(vec![return_(Some(malformed))]),
    ));
    out
}

/// The order the router tries its routes in (#3909): by fewest parameter segments,
/// then method and path, so the output is stable; then, where two routes of a
/// method match a common path, the one with the literal at the first segment where
/// they differ goes first (`bynk_check::route_order`), so the answer never depends
/// on how a route is spelt.
fn sort_routes(mut routes: Vec<HttpRoute>) -> Vec<HttpRoute> {
    routes.sort_by(|a, b| {
        param_count(&a.path)
            .cmp(&param_count(&b.path))
            .then_with(|| a.method.as_str().cmp(b.method.as_str()))
            .then_with(|| a.path.cmp(&b.path))
    });
    let keys: Vec<(&str, &str)> = routes
        .iter()
        .map(|r| (r.method.as_str(), r.path.as_str()))
        .collect();
    let order = bynk_check::route_order::order(&keys);
    let mut slots: Vec<Option<HttpRoute>> = routes.into_iter().map(Some).collect();
    order
        .into_iter()
        .map(|i| slots[i].take().expect("each route is placed once"))
        .collect()
}

/// The `415` a `@raw` route answers to a `Content-Type` outside the declared
/// media type or list of them (#3912), or a JSON `body` route answers to a
/// `Content-Type` that is present and neither `application/json` nor a `+json`
/// type (a missing one is accepted). A router decision like the `413`, ahead of
/// the auth seam and the body read, so it costs nothing and reveals nothing.
/// `@form` and `@multipart` routes have their own checks.
fn raw_json_media_guard(
    route: &HttpRoute,
    has_body: bool,
    (cors_const, security_const, rejected): (Option<&str>, Option<&str>, Option<&str>),
) -> Option<TsStmt> {
    if !has_body {
        return None;
    }
    let (matcher, args, expected) = match route.raw.as_deref() {
        Some([only]) if only == "*/*" => return None,
        Some(types) => {
            let list = TsExpr::array(types.iter().map(|t| str_lit(t.clone())).collect());
            (
                "__mediaTypeMatches",
                vec![ident("request"), list],
                types.join(", "),
            )
        }
        None if route.form_fields.is_none() && route.multipart.is_none() => (
            "__jsonMediaTypeMatches",
            vec![ident("request")],
            "application/json".to_string(),
        ),
        None => return None,
    };
    let details = if route.raw.is_some() {
        format!("expected Content-Type {expected}")
    } else {
        "expected Content-Type application/json".to_string()
    };
    let unsupported = stamp_rejection(
        json_response(
            json_error_kind(
                "UnsupportedMediaType",
                vec![("details".to_string(), str_lit(details))],
            ),
            415,
        ),
        cors_const,
        security_const,
        rejected,
    );
    Some(if_(
        TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(call(ident(matcher), args)),
        },
        block(vec![return_(Some(unsupported))]),
    ))
}

/// Generate the per-route dispatch block for one `on http` handler. The
/// router has already been entered via `try`; this block extracts path
/// parameters, deserialises the body (when present), invokes the handler,
/// and serialises the HttpResult through `httpResultToResponse`. Returns
/// the whole bare `{ ... }` block statement — `emit_worker_entry`'s own
/// caller pushes it directly into the `try` block's own statement list.
pub(super) fn emit_http_route_dispatch(
    route: &HttpRoute,
    cors_const: Option<&str>,
    security_const: Option<&str>,
    ru: &RuntimeUse,
    consumed_types: &std::collections::HashSet<String>,
) -> TsStmt {
    let h = &route.handler;
    let rejected = route.rejected.as_deref();
    let method_key = http_handler_method_name_ir(route.method, &route.path);
    let has_path_params = param_count(&route.path) > 0;
    // v0.139 (ADR 0162 D3): a `GET` route also answers `HEAD` — the guard widens
    // so the `GET` handler runs, then the built response's body is stripped
    // below. Other methods match exactly.
    let method_guard = if route.method == bynk_model::IrHttpMethod::Get {
        or_expr(
            strict_eq(ident("method"), str_lit("GET")),
            strict_eq(ident("method"), str_lit("HEAD")),
        )
    } else {
        strict_eq(ident("method"), str_lit(route.method.as_str()))
    };

    let mut inner: Vec<TsStmt> = Vec::new();
    if has_path_params {
        inner.push(const_(
            "__m",
            call(
                ident("__matchPath"),
                vec![str_lit(route.path.clone()), ident("path")],
            ),
        ));
    }
    let route_cond = if has_path_params {
        and_expr(method_guard, ident("__m"))
    } else {
        // #3909: a literal route is matched the same way, so an empty segment is
        // insignificant and a literal is compared percent-decoded.
        and_expr(
            method_guard,
            strict_neq(
                call(
                    ident("__matchPath"),
                    vec![str_lit(route.path.clone()), ident("path")],
                ),
                null_lit(),
            ),
        )
    };

    // #1825: names the route, by its pattern, for the fault log.
    let mut guarded = route_prelude(route, (cors_const, security_const, rejected));

    // v0.142 (ADR 0165): the request-body ceiling. A route with an effective cap
    // rejects an oversized body with a synthesised `413` derived from the declared
    // `Content-Length`, *before* the body is read — ahead of the `by`/Bearer/sum
    // wrapper and the Signature seam's raw read, so an oversized (possibly
    // unauthenticated) body is never buffered (DECISION D/E). The `413` is
    // `applyCors`/`applySecurityHeaders`-stamped so a cross-origin caller can read
    // it, the same visibility rule the `405` follows. Emitted only for a
    // body-taking route with a cap; a capless route is byte-for-byte unchanged
    // (opt-in). This placement also covers a multi-actor sum route, whose wrapper
    // reads the body itself.
    let has_body = h.params.iter().any(|p| p.name.name == "body");
    // The synthesised `413`, shared by the `Content-Length` fast-reject and the
    // counted read below.
    let too_large = |cap: i64| {
        stamp_rejection(
            json_response(
                json_error_kind(
                    "PayloadTooLarge",
                    vec![(
                        "details".to_string(),
                        str_lit(format!("request body exceeds {cap} bytes")),
                    )],
                ),
                413,
            ),
            cors_const,
            security_const,
            rejected,
        )
    };
    guarded.extend(raw_json_media_guard(
        route,
        has_body,
        (cors_const, security_const, rejected),
    ));
    // #3276: a `@form` route answers `415` to any `Content-Type` but a UTF-8
    // `application/x-www-form-urlencoded`, at the same point as a `@raw` route.
    if has_body && route.form_fields.is_some() {
        let unsupported = stamp_rejection(
            json_response(
                json_error_kind(
                    "UnsupportedMediaType",
                    vec![(
                        "details".to_string(),
                        str_lit("expected Content-Type application/x-www-form-urlencoded (UTF-8)"),
                    )],
                ),
                415,
            ),
            cors_const,
            security_const,
            rejected,
        );
        guarded.push(if_(
            TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Not,
                expr: Box::new(call(
                    ident("__formMediaTypeMatches"),
                    vec![ident("request")],
                )),
            },
            block(vec![return_(Some(unsupported))]),
        ));
    }
    // #3276: a `@multipart` route answers `415` to any `Content-Type` but
    // `multipart/form-data`; a body with no boundary is a `400 MalformedForm`, below.
    if has_body && route.multipart.is_some() {
        let unsupported = stamp_rejection(
            json_response(
                json_error_kind(
                    "UnsupportedMediaType",
                    vec![(
                        "details".to_string(),
                        str_lit("expected Content-Type multipart/form-data"),
                    )],
                ),
                415,
            ),
            cors_const,
            security_const,
            rejected,
        );
        guarded.push(if_(
            TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Not,
                expr: Box::new(call(
                    ident("__multipartMediaTypeMatches"),
                    vec![ident("request")],
                )),
            },
            block(vec![return_(Some(unsupported))]),
        ));
    }
    if let Some(cap) = route.max_body.filter(|_| has_body) {
        guarded.push(const_(
            "__contentLength",
            method_call(
                member(ident("request"), "headers"),
                "get",
                vec![str_lit("content-length")],
            ),
        ));
        guarded.push(if_(
            and_expr(
                strict_neq(ident("__contentLength"), null_lit()),
                binary(
                    bynk_ts::TsBinaryOp::GreaterThan,
                    call(ident("globalThis.Number"), vec![ident("__contentLength")]),
                    num_lit(cap.to_string()),
                ),
            ),
            block(vec![return_(Some(too_large(cap)))]),
        ));
    }

    // Extract path parameters from the matched params map.
    let mut call_args: Vec<TsExpr> = Vec::new();
    for p in &h.params {
        let pname = &p.name.name;
        if pname == "body" {
            // Body deserialisation happens below; the body takes its place in
            // the argument list here, in declaration order. A multi-actor sum
            // route's wrapper reads the body itself, so it isn't passed.
            if !route.sum {
                call_args.push(ident("body"));
            }
            continue;
        }
        // #2341 (decision A): `headers` is the request's headers, read-only.
        if pname == "headers" && matches!(p.type_ref, TypeRef::RequestHeaders(_)) {
            call_args.push(request_headers_arg(route));
            continue;
        }
        // #2854 (decision B): `query: RequestQuery` is the raw query string.
        let query_is_path = route.path.split('/').any(|seg| seg == ":query");
        if pname == "query" && !query_is_path && matches!(p.type_ref, TypeRef::RequestQuery(_)) {
            call_args.push(call(ident("__requestQuery"), vec![ident("request")]));
            continue;
        }
        // #2854 (decision A): a typed `query` is decoded off the query string
        // and validated before the handler runs, answering `400` as a `body`
        // does. The runtime reads the fields into a JSON object, which the
        // record's own codec then checks (refinements, defaults, `Option`s).
        if pname == "query"
            && !query_is_path
            && let Some(fields) = &route.query_fields
        {
            let spec: Vec<TsExpr> = field_specs_exprs(fields);
            guarded.push(const_(
                "__q",
                call(
                    ident("__queryObject"),
                    vec![ident("request"), TsExpr::array(spec)],
                ),
            ));
            guarded.push(if_(
                TsExpr::Unary {
                    op: bynk_ts::TsUnaryOp::Not,
                    expr: Box::new(member(ident("__q"), "ok")),
                },
                return_(Some(stamp_rejection(
                    json_response(member(ident("__q"), "error"), 400),
                    cors_const,
                    security_const,
                    rejected,
                ))),
            ));
            let dser = deserialise_call(&p.type_ref, &member(ident("__q"), "value"), "query", ru);
            guarded.push(const_("__r_query", dser));
            guarded.push(if_(
                strict_eq(member(ident("__r_query"), "tag"), str_lit("Err")),
                return_(Some(stamp_rejection(
                    json_response(member(ident("__r_query"), "error"), 400),
                    cors_const,
                    security_const,
                    rejected,
                ))),
            ));
            guarded.push(const_("query", member(ident("__r_query"), "value")));
            call_args.push(ident("query"));
            continue;
        }
        if bind_request_record_param(
            route,
            p,
            &mut guarded,
            &mut call_args,
            (cors_const, security_const, rejected),
            ru,
        ) {
            continue;
        }
        // It's a path parameter — extract from __m.params and construct. The
        // `__m.params` key is the wire segment name (verbatim); the constructed
        // binding must dodge JS reserved words for the surface call.
        let jname = wire_ident(pname);
        guarded.push(const_(
            format!("__raw_{pname}"),
            index(member(ident("__m"), "params"), str_lit(pname.clone())),
        ));
        guarded.extend(emit_path_param_construction(
            pname,
            &p.type_ref,
            cors_const,
            security_const,
            rejected,
            consumed_types,
        ));
        call_args.push(ident(jname));
    }

    // Body parameter (POST/PUT/PATCH). A multi-actor sum route's wrapper reads
    // and parses the body itself (it must verify over the raw bytes first), so
    // the entry skips the body here and passes only `request`.
    if !route.sum
        && let Some(body_param) = h.params.iter().find(|p| p.name.name == "body")
    {
        // v0.188 (#659): the body-read rejections are stamped with the service
        // policy just like the handled `200`. bynk_ts::TsExpr here, at their use site, to
        // match the local style (the `413`, `body_reject`, and path-param
        // rejections are all built lazily too) — a bodyless `GET` never allocates
        // them. `malformed_400` is reached from both the signature and plain body
        // paths; `unauthorized_401` only from the signature seam, so it is built
        // there; the input-reflecting body-deser `400` stamps inline below.
        let malformed_400 = || {
            stamp_rejection(
                json_response(
                    json_error_kind(
                        "MalformedJson",
                        vec![("details".to_string(), str_lit("Invalid request body"))],
                    ),
                    400,
                ),
                cors_const,
                security_const,
                rejected,
            )
        };
        // A capped body is read through the counting reader, so a chunked body, or
        // one whose `Content-Length` understates it, is cut off at the cap with the
        // same `413` as the fast-reject above. A raw route is always capped.
        let streamed = streamed_body(route); // pulled by the handler, so not read here
        let counted = route.max_body.filter(|_| !streamed);
        let raw = route.raw.is_some();
        let form = route.form_fields.as_ref();
        let multipart = route.multipart.as_ref();
        let decoded = || {
            method_call(
                new(ident("globalThis.TextDecoder"), vec![]),
                "decode",
                vec![ident("__bytes")],
            )
        };
        if let Some(cap) = counted {
            guarded.push(let_typed(
                "__bytes",
                TsType::named("globalThis.Uint8Array | undefined"),
            ));
            guarded.push(try_catch_stmt(
                vec![TsStmt::assign(
                    ident("__bytes"),
                    await_expr(call(
                        ident("__readBodyCapped"),
                        vec![ident("request"), num_lit(cap.to_string())],
                    )),
                    None,
                )],
                None,
                vec![return_(Some(malformed_400()))],
            ));
            guarded.push(if_(
                strict_eq(ident("__bytes"), ident("undefined")),
                block(vec![return_(Some(too_large(cap)))]),
            ));
        }
        if !raw && form.is_none() && multipart.is_none() {
            guarded.push(let_typed("__body_json", TsType::named("__JsonValue")));
        }
        if let Some(seam) = &route.signature {
            // v0.51: read the raw body once, verify the HMAC fail-closed (401),
            // then parse the body param from the *same* bytes (not a re-read /
            // re-serialisation — the signature is over these exact bytes).
            let unauthorized_401 = || {
                stamp_rejection(
                    challenged(route, null_response(401)),
                    cors_const,
                    security_const,
                    rejected,
                )
            };
            // What the HMAC covers: a raw route's octets as received; otherwise
            // the body's text.
            let signed = if raw || multipart.is_some() {
                ident("__bytes")
            } else if counted.is_some() {
                guarded.push(const_("__raw", decoded()));
                ident("__raw")
            } else {
                guarded.push(let_typed("__raw", TsType::named("string")));
                guarded.push(try_catch_stmt(
                    vec![TsStmt::assign(
                        ident("__raw"),
                        await_expr(method_call(ident("request"), "text", vec![])),
                        None,
                    )],
                    None,
                    vec![return_(Some(malformed_400()))],
                ));
                ident("__raw")
            };
            guarded.push(const_("__secret", {
                let explicit = index(
                    as_expr(
                        as_expr(ident("env"), TsType::named("unknown")),
                        TsType::named_with_args(
                            "globalThis.Record",
                            vec![TsType::named("string"), TsType::named("unknown")],
                        ),
                    ),
                    str_lit(seam.secret.clone()),
                );
                let global_probe = TsExpr::OptionalIndex {
                    object: Box::new(TsExpr::OptionalMember {
                        object: Box::new(member(
                            as_expr(
                                ident("globalThis"),
                                TsType::Object(vec![TsTypeMember::optional_prop(
                                    "process",
                                    TsType::Object(vec![TsTypeMember::optional_prop(
                                        "env",
                                        TsType::named_with_args(
                                            "globalThis.Record",
                                            vec![TsType::named("string"), TsType::named("unknown")],
                                        ),
                                    )]),
                                )]),
                            ),
                            "process",
                        )),
                        property: "env".to_string(),
                    }),
                    index: Box::new(str_lit(seam.secret.clone())),
                };
                binary(
                    bynk_ts::TsBinaryOp::NullishCoalescing,
                    explicit,
                    global_probe,
                )
            }));
            guarded.push(if_(
                strict_neq(
                    TsExpr::Unary {
                        op: bynk_ts::TsUnaryOp::Typeof,
                        expr: Box::new(ident("__secret")),
                    },
                    str_lit("string"),
                ),
                return_(Some(unauthorized_401())),
            ));
            // Timestamp (when bound): must be present; passed to the verifier.
            let ts_expr: TsExpr = match &seam.timestamp_header {
                Some(th) => {
                    guarded.push(const_(
                        "__ts",
                        method_call(
                            member(ident("request"), "headers"),
                            "get",
                            vec![str_lit(th.clone())],
                        ),
                    ));
                    guarded.push(if_(
                        strict_eq(ident("__ts"), null_lit()),
                        return_(Some(unauthorized_401())),
                    ));
                    ident("__ts")
                }
                None => null_lit(),
            };
            let tol_expr = match seam.tolerance_secs {
                Some(n) => num_lit(n.to_string()),
                None => null_lit(),
            };
            guarded.push(const_(
                "__ok",
                await_expr(call(
                    ident("__verifySignatureHmacSha256"),
                    vec![
                        signed.clone(),
                        ident("__secret"),
                        method_call(
                            member(ident("request"), "headers"),
                            "get",
                            vec![str_lit(seam.header.clone())],
                        ),
                        ts_expr,
                        tol_expr,
                    ],
                )),
            ));
            guarded.push(if_(
                TsExpr::Unary {
                    op: bynk_ts::TsUnaryOp::Not,
                    expr: Box::new(ident("__ok")),
                },
                return_(Some(unauthorized_401())),
            ));
            if !raw && form.is_none() && multipart.is_none() {
                guarded.push(try_catch_stmt(
                    vec![TsStmt::assign(
                        ident("__body_json"),
                        as_expr(
                            method_call(ident("globalThis.JSON"), "parse", vec![ident("__raw")]),
                            TsType::named("__JsonValue"),
                        ),
                        None,
                    )],
                    None,
                    vec![return_(Some(malformed_400()))],
                ));
            }
        } else if multipart.is_some() {
            // The counted bytes in `__bytes` are parsed below.
        } else if form.is_some() {
            // The body's text, read here; the form reader below decodes it.
            if counted.is_some() {
                guarded.push(const_("__raw", decoded()));
            } else {
                guarded.push(let_typed("__raw", TsType::named("string")));
                guarded.push(try_catch_stmt(
                    vec![TsStmt::assign(
                        ident("__raw"),
                        await_expr(method_call(ident("request"), "text", vec![])),
                        None,
                    )],
                    None,
                    vec![return_(Some(malformed_400()))],
                ));
            }
        } else if !raw {
            let parsed = if counted.is_some() {
                method_call(ident("globalThis.JSON"), "parse", vec![decoded()])
            } else {
                // This site's own real historical text keeps the
                // parens (`(await request.json()) as JsonValue`),
                // unlike the `/_bynk/call/` dispatch's bare `args`
                // above — see that site's own comment.
                paren(await_expr(method_call(ident("request"), "json", vec![])))
            };
            guarded.push(try_catch_stmt(
                vec![TsStmt::assign(
                    ident("__body_json"),
                    as_expr(parsed, TsType::named("__JsonValue")),
                    None,
                )],
                None,
                vec![return_(Some(malformed_400()))],
            ));
        }
        if streamed {
            guarded.push(streamed_body_binding(route));
        } else if raw {
            // The octets, untouched: no JSON, no base64.
            guarded.push(const_("body", ident("__bytes")));
        } else {
            // #3276: a form body is read into the JSON object the record's codec
            // takes, naming `body.<field>` in its rejections.
            let (source, root) = if let Some(mp) = multipart {
                let spec: Vec<TsExpr> = field_specs_exprs(&mp.fields);
                let limits = TsExpr::object(vec![
                    (
                        "maxFile".to_string(),
                        match mp.max_file {
                            Some(n) => num_lit(n.to_string()),
                            None => null_lit(),
                        },
                    ),
                    ("maxParts".to_string(), num_lit(mp.max_parts.to_string())),
                ]);
                guarded.push(const_(
                    "__f",
                    await_expr(call(
                        ident("__multipartObject"),
                        vec![
                            ident("__bytes"),
                            binary(
                                bynk_ts::TsBinaryOp::NullishCoalescing,
                                method_call(
                                    member(ident("request"), "headers"),
                                    "get",
                                    vec![str_lit("content-type")],
                                ),
                                str_lit(""),
                            ),
                            TsExpr::array(spec),
                            limits,
                        ],
                    )),
                ));
                // A `413` (too many parts, a file over its cap) or a `400` (not a
                // multipart body, or a part that is not its field's shape).
                let reject = |status: i64| {
                    stamp_rejection(
                        json_response(member(ident("__f"), "error"), status),
                        cors_const,
                        security_const,
                        rejected,
                    )
                };
                guarded.push(if_(
                    TsExpr::Unary {
                        op: bynk_ts::TsUnaryOp::Not,
                        expr: Box::new(member(ident("__f"), "ok")),
                    },
                    block(vec![return_(Some(cond_expr(
                        strict_eq(member(ident("__f"), "status"), num_lit("413")),
                        reject(413),
                        reject(400),
                    )))]),
                ));
                (member(ident("__f"), "value"), "body")
            } else if let Some(fields) = form {
                let spec: Vec<TsExpr> = field_specs_exprs(fields);
                guarded.push(const_(
                    "__f",
                    call(
                        ident("__formObject"),
                        vec![ident("__raw"), TsExpr::array(spec)],
                    ),
                ));
                guarded.push(if_(
                    TsExpr::Unary {
                        op: bynk_ts::TsUnaryOp::Not,
                        expr: Box::new(member(ident("__f"), "ok")),
                    },
                    return_(Some(stamp_rejection(
                        json_response(member(ident("__f"), "error"), 400),
                        cors_const,
                        security_const,
                        rejected,
                    ))),
                ));
                (member(ident("__f"), "value"), "body")
            } else {
                (ident("__body_json"), "$")
            };
            let dser = deserialise_call(&body_param.type_ref, &source, root, ru);
            guarded.push(const_("__r_body", dser));
            // The body-deser `400` reflects the offending input into its JSON body, so
            // stamping it (#659) matters most here — `nosniff` on a reflected value.
            let body_reject = stamp_rejection(
                json_response(member(ident("__r_body"), "error"), 400),
                cors_const,
                security_const,
                rejected,
            );
            guarded.push(if_(
                strict_eq(member(ident("__r_body"), "tag"), str_lit("Err")),
                return_(Some(body_reject)),
            ));
            if multipart.is_some_and(|mp| mp.has_files) {
                // The `File` values, laid over the decoded record's placeholders.
                guarded.push(const_(
                    "body",
                    call(
                        ident("globalThis.Object.assign"),
                        vec![
                            TsExpr::object(vec![]),
                            member(ident("__r_body"), "value"),
                            member(ident("__f"), "files"),
                        ],
                    ),
                ));
            } else {
                guarded.push(const_("body", member(ident("__r_body"), "value")));
            }
        }
    }

    // Invoke the handler and serialise the HttpResult. The handler is
    // wrapped on the surface so its deps are wired by `compose`. v0.47: a
    // Bearer wrapper takes the request first (it runs the verification seam).
    let surface_args: Vec<TsExpr> =
        if route.bearer || route.sum || route.oidc || route.signed_cookie.is_some() {
            // The Bearer, Oidc, and sum wrappers take the request first (they run the
            // verification seam); a sum wrapper also reads/parses the body itself,
            // so `call_args` here carries only the path params.
            let mut a = vec![ident("request")];
            a.extend(call_args.iter().cloned());
            a
        } else {
            call_args
        };
    push_handler_invocation(route, method_key, surface_args, too_large, &mut guarded);
    let inner_ty = http_result_inner(&h.return_type);
    let ser_fn = crate::emitter::serialisation::serialise_ref_via(&inner_ty, "handlers.", ru);
    // #2858: a plain-JSON route's body serialiser runs with the `Option` encoders
    // switched to `v` / `null`; every other route passes `ser_fn` untouched.
    let ser_fn = if route.json_plain {
        call(ident("__withPlainJson"), vec![ser_fn])
    } else {
        ser_fn
    };
    // v0.140 (ADR 0163): a `GET` response carries a weak `ETag` (an `Ok` body gets
    // the validator via `weakEtag`), an optional `@cache` `Cache-Control`
    // (`applyCache`), and is answered `304` when the request revalidates
    // (`notModifiedIfMatch`) — composed innermost-first so CORS still stamps the
    // `304`. Non-`GET`/unsafe methods are byte-for-byte unchanged: no validator, no
    // conditional, no freshness (DECISION B).
    let response_expr = if route.method == bynk_model::IrHttpMethod::Get {
        let base = call(
            ident("__httpResultToResponse"),
            vec![
                ident("result"),
                ser_fn,
                TsExpr::object(vec![("weakEtag".to_string(), bool_lit(true))]),
            ],
        );
        // `immutable` is passed only when `true`, so every other `@cache` route
        // emits exactly the three-argument call it did before.
        let cached = match &route.cache {
            Some(p) => {
                let mut args = vec![base, num_lit(p.max_age_secs.to_string()), str_lit(p.scope)];
                if p.immutable {
                    args.push(bool_lit(true));
                }
                let apply = if route.edge_cache {
                    "__applyEdgeCache"
                } else {
                    "__applyCache"
                };
                if route.vary_locale {
                    args[0] = call(ident("__varyAcceptLanguage"), vec![args[0].clone()]);
                }
                call(ident(apply), args)
            }
            None if route.edge_cache => call(ident("__noStore"), vec![base]),
            None => base,
        };
        call(
            ident("__notModifiedIfMatch"),
            vec![cached, ident("request")],
        )
    } else {
        call(
            ident("__httpResultToResponse"),
            vec![ident("result"), ser_fn],
        )
    };
    // #3914: a `401` carries the route's `WWW-Authenticate` challenge (inside CORS, so the
    // header is exposed to a browser client).
    let response_expr = challenged(route, response_expr);
    // v0.131: a CORS-enabled service stamps the `Access-Control-*` headers onto
    // every real response, uniformly across variant families — including the
    // synthesised `304`, so a cross-origin revalidation stays readable.
    let cors_expr = match cors_const {
        Some(c) => call(
            ident("__applyCors"),
            vec![
                response_expr,
                ident(c.to_string()),
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit("origin")],
                ),
            ],
        ),
        None => response_expr,
    };
    // v0.141 (ADR 0164): stamp the security headers (`nosniff` by default, HSTS
    // opt-in) onto every response — outside `applyCors`, but the two set disjoint
    // headers so order is immaterial. Every `from http` service has a policy, so
    // this is `Some` on every route; it also flows onto the `304` (inside
    // `response_expr`) and the `HEAD` answer (`headResponse` copies the headers).
    let build_expr = match security_const {
        Some(s) => call(
            ident("__applySecurityHeaders"),
            vec![cors_expr, ident(s.to_string())],
        ),
        None => cors_expr,
    };
    // v0.139 (ADR 0162 D3): on a `GET` route, a `HEAD` returns the same status
    // and headers with an empty body — the handler ran, so the headers are the
    // real ones a `GET` would produce; `headResponse` discards the body without
    // reading it (a `Streaming` body is never drained).
    if route.method == bynk_model::IrHttpMethod::Get {
        guarded.push(const_("__response", build_expr));
        guarded.push(return_(Some(cond_expr(
            strict_eq(ident("method"), str_lit("HEAD")),
            call(ident("__headResponse"), vec![ident("__response")]),
            ident("__response"),
        ))));
    } else {
        guarded.push(return_(Some(build_expr)));
    }

    inner.push(if_(route_cond, block(guarded)));
    block(inner)
}

/// #3914: the challenges of a route's `401`s: `Bearer` for a `Bearer` or `Oidc` actor
/// (RFC 6750), `Bynk-Signature` for a `Signature` actor, `Bynk-Cookie` for a `SignedCookie`
/// actor, one per member of a sum in the sum's order, and `Bynk` for anything else (a handler
/// of a `Visitor` route returning `Unauthorized` itself). `realm` is the context's name.
pub(super) fn route_challenges(h: &Handler, table: &UnitTable, context: &str) -> Vec<String> {
    use bynk_check::actors::SumMemberSeam;
    let realm = crate::emitter::escape_ts_string(context);
    let bearer = || format!("Bearer realm=\"{realm}\"");
    let signature = |s: &bynk_check::actors::SignatureSeam| {
        let header = crate::emitter::escape_ts_string(&s.header);
        format!("Bynk-Signature realm=\"{realm}\", header=\"{header}\"")
    };
    if let Some(members) = bynk_check::actors::sum_members_for(h, &table.actors) {
        return members
            .iter()
            .map(|m| match &m.seam {
                SumMemberSeam::None => format!("Bynk realm=\"{realm}\""),
                SumMemberSeam::Bearer { .. } => bearer(),
                SumMemberSeam::Signature(s) => signature(s),
            })
            .collect();
    }
    if bynk_check::actors::bearer_seam_for(h, &table.actors).is_some()
        || bynk_check::actors::oidc_seam_for(h, &table.actors).is_some()
    {
        return vec![bearer()];
    }
    if let Some(s) = bynk_check::actors::signature_seam_for(h, &table.actors) {
        return vec![signature(&s)];
    }
    if signed_cookie_name(h, &table.actors).is_some() {
        return vec![format!("Bynk-Cookie realm=\"{realm}\"")];
    }
    vec![format!("Bynk realm=\"{realm}\"")]
}

/// #3914: wrap a route's response so a `401` carries the route's challenge.
fn challenged(route: &HttpRoute, response: TsExpr) -> TsExpr {
    let items = route
        .challenges
        .iter()
        .map(|c| str_lit(c.clone()))
        .collect();
    call(
        ident("__applyChallenge"),
        vec![
            response,
            ident("request"),
            TsExpr::Array {
                items,
                multiline: false,
            },
        ],
    )
}

/// #3380: `const body = requestBodyStream(request, cap)`: the octets as they
/// arrive, counted against the cap as the handler pulls them. The checker
/// requires a cap on every `@raw` route.
fn streamed_body_binding(route: &HttpRoute) -> TsStmt {
    let cap = route
        .max_body
        .expect("a streamed `@raw` route has a cap (bynk.http.raw_body_unbounded)");
    const_(
        "body",
        call(
            ident("__requestBodyStream"),
            vec![ident("request"), num_lit(cap.to_string())],
        ),
    )
}

/// The statements that invoke a route's handler and bind its `result`. A streamed
/// body that crosses its cap while the handler reads it faults that read with
/// `BodyTooLarge` (#3380); no response has been built yet, so the router answers the
/// route's `413`, stamped as the `Content-Length` fast-reject is, and not a `500`.
fn push_handler_invocation(
    route: &HttpRoute,
    method_key: String,
    surface_args: Vec<TsExpr>,
    too_large: impl Fn(i64) -> TsExpr,
    guarded: &mut Vec<TsStmt>,
) {
    let invoke = call(member(ident("surface"), method_key), surface_args);
    let Some(cap) = route.max_body.filter(|_| streamed_body(route)) else {
        guarded.push(const_("result", await_expr(invoke)));
        return;
    };
    guarded.push(const_(
        "__handled",
        await_expr(call(ident("__untilBodyTooLarge"), vec![invoke])),
    ));
    guarded.push(if_(
        member(ident("__handled"), "tooLarge"),
        block(vec![return_(Some(too_large(cap)))]),
    ));
    guarded.push(const_("result", member(ident("__handled"), "value")));
}

/// #3380: whether `route`'s `body` is a streamed one: a `@raw` route whose `body`
/// is a `Stream[Bytes]`, handed to the handler as the request body arrives rather
/// than read whole first.
fn streamed_body(route: &HttpRoute) -> bool {
    route.raw.is_some()
        && route
            .handler
            .params
            .iter()
            .any(|p| p.name.name == "body" && matches!(p.type_ref, TypeRef::Stream(..)))
}

/// Emit code that converts the raw path-parameter string to the parameter's
/// declared type. For plain `String`, this is a direct assignment; for
/// refined or opaque `String`-based types we lower through `T.of(...)`
/// which performs refinement validation and returns 400 on failure.
///
/// #2252: a type from a consumed context is imported type-only, so it has no
/// `of` here. It is validated through this context's own codec,
/// `handlers.__deserialise_T(raw, "path.<p>")`, whose refinement check is the
/// owner's and whose error is the same `RefinementViolation` this answers.
fn emit_path_param_construction(
    pname: &str,
    t: &TypeRef,
    cors_const: Option<&str>,
    security_const: Option<&str>,
    rejected: Option<&str>,
    consumed_types: &std::collections::HashSet<String>,
) -> Vec<TsStmt> {
    // `pname` names the wire segment (the `__raw_`/`__r_` temps and diagnostic
    // path stay verbatim); `jname` is the reserved-word-safe binding the surface
    // call references.
    let jname = wire_ident(pname);
    let raw = format!("__raw_{pname}");
    match t {
        TypeRef::Base(BaseType::String, _) => {
            vec![const_(jname, ident(raw))]
        }
        TypeRef::Named(id) => {
            let result = format!("__r_{pname}");
            let wire_path = str_lit(format!("path.{pname}"));
            let consumed = consumed_types.contains(&id.name);
            let construct = if consumed {
                method_call(
                    ident("handlers"),
                    format!("__deserialise_{}", id.name),
                    vec![ident(raw), wire_path.clone()],
                )
            } else {
                method_call(
                    member(ident("handlers"), id.name.clone()),
                    "of",
                    vec![ident(raw)],
                )
            };
            let mut stmts = vec![const_(result.clone(), construct)];
            // The path-param refinement `400` reflects the offending segment into
            // its JSON body, so it is stamped with the service policy (#659).
            // A codec's error is already that body (`kind`, `path`, `violation`).
            let body = if consumed {
                member(ident(result.clone()), "error")
            } else {
                json_error_kind(
                    "RefinementViolation",
                    vec![
                        ("path".to_string(), wire_path),
                        (
                            "violation".to_string(),
                            member(ident(result.clone()), "error"),
                        ),
                    ],
                )
            };
            let inner_resp = json_response(body, 400);
            let reject = stamp_rejection(inner_resp, cors_const, security_const, rejected);
            stmts.push(if_(
                strict_eq(member(ident(result.clone()), "tag"), str_lit("Err")),
                return_(Some(reject)),
            ));
            stmts.push(const_(jname, member(ident(result), "value")));
            stmts
        }
        TypeRef::Base(..)
        | TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::List(..)
        | TypeRef::Map(..)
        | TypeRef::Set(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..)
        | TypeRef::App { .. } => {
            // P7.2 (review of #1300): this used to emit `__raw_{pname}` as a
            // silent fallback ("other shapes are rejected by the static
            // check"), which was survivable only while the receiving wrapper
            // param was itself `any`/`unknown` — a raw `string` satisfies
            // anything. Now that `emit_http_wrapper`/`emit_http_oidc_wrapper`/
            // `emit_http_sum_wrapper` type that same param at its real
            // declared type (`qualified_type_ref`, `workers.rs`), the fallback
            // would hand a `string` to a `number`/whatever-typed parameter and
            // ship a project `tsc --strict` rejects — silently, since this
            // function has no way to surface that. `bynk.http.path_param_not_stringy`
            // (`bynk-check/src/context_checks.rs`) is a total guarantee: a path
            // parameter's `TypeRef` is checked to be exactly `String` or a
            // `String`-based `Named` before emission ever runs, so this arm is
            // unreachable for any program that passed the checker. Fail loudly
            // if that guarantee is ever wrong, rather than emit TypeScript that
            // cannot compile.
            panic!(
                "bynk internal error: path parameter `{pname}` reached emission with a \
                 non-string-constructible type ({t:?}) — `bynk.http.path_param_not_stringy` \
                 should have rejected this at check time"
            );
        }
    }
}

/// Collect every `from http` route across the services, sorted so
/// literal-segment routes precede parameter-segment routes that share a prefix.
pub(super) fn collect_http_routes(
    table: &UnitTable,
    service_names: &[&String],
    visible_types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
    edge_cache: bool,
    needs_locale_request: bool,
    context: &str,
) -> Vec<HttpRoute> {
    let mut http_routes: Vec<HttpRoute> = Vec::new();
    for sname in service_names {
        let service = table.services.get(*sname).unwrap();
        for h in &service.handlers {
            // Slice 1 of `#1542` (the IR cutover, `design/archive/retired-tracks.md` §5): reads
            // `IrHandlerKind::Http`'s own `method`/`path` directly — the IR
            // payload P6.24a already computes here, no longer re-derived from
            // `h.kind` a second time. `HttpRoute::method` is `IrHttpMethod`
            // (un-deferred from Q7's original "stays AST until phase 7's
            // printer" — phase 7 shipped, see the front-loading ADR).
            if let bynk_model::IrHandlerKind::Http { method, path } =
                bynk_model::lower_handler_kind_ir(&h.kind)
            {
                let query_fields = query_field_specs(h, &path, visible_types);
                let cookies_fields = cookies_field_specs(h, &path, visible_types);
                let headers_fields = headers_field_specs(h, &path, visible_types);
                http_routes.push(HttpRoute {
                    service: (*sname).clone(),
                    method,
                    path,
                    handler: h.clone(),
                    // v0.47: a Bearer handler's surface wrapper runs the
                    // verification seam and needs the request passed in.
                    bearer: bynk_check::actors::bearer_seam_for(h, &table.actors).is_some(),
                    // v0.151: an `Oidc` wrapper runs its verification seam, taking the request first.
                    oidc: bynk_check::actors::oidc_seam_for(h, &table.actors).is_some(),
                    signed_cookie: signed_cookie_name(h, &table.actors),
                    hidden_headers: consumed_credential_headers(h, &table.actors),
                    signature: bynk_check::actors::signature_seam_for(h, &table.actors),
                    challenges: route_challenges(h, table, context),
                    // v0.52: a multi-actor sum handler's wrapper owns the whole
                    // boundary (raw read, first-wins resolution, body parse), so
                    // the entry just passes `request` and skips the body parse.
                    sum: bynk_check::actors::sum_members_for(h, &table.actors).is_some(),
                    // v0.140 (ADR 0163): the handler's `@cache` freshness policy, if
                    // any — lowered to a `Cache-Control` on this GET's responses.
                    cache: bynk_model::lower_route_cache_ir(h),
                    edge_cache,
                    vary_locale: edge_cache && needs_locale_request,
                    // v0.142 (ADR 0165): the route's effective request-body ceiling
                    // in bytes — the handler's `@limit` if any, else the service's
                    // `limits { }`, else none. `Some` drives the synthesised `413`
                    // `Content-Length` guard before the body is read.
                    max_body: effective_max_body(service.limits.as_ref(), h),
                    raw: Some(h.raw_content_types())
                        .filter(|v| !v.is_empty())
                        .map(|v| v.into_iter().map(str::to_string).collect()),
                    json_plain: bynk_syntax::ast::effective_json_options(service.json.as_ref(), h)
                        == bynk_syntax::ast::JsonOptions::Plain,
                    rejected: rejected_surface(service, sname),
                    query_fields,
                    form_fields: form_field_specs(h, visible_types),
                    cookies_fields,
                    headers_fields,
                    multipart: multipart_spec(service.limits.as_ref(), h, visible_types),
                });
            }
        }
    }
    sort_routes(http_routes)
}
