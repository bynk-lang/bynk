//! The HTTP policy of the Worker entry: the CORS services, the security-header services and the
//! method table, each derived from the route table.

use super::http::HttpRoute;
use super::respond::{miss, new_response, rejection_renderer, str_array};
use crate::ts_build::{
    and_expr, await_expr, block, bool_lit, call, cond_expr, const_, ident, if_, member,
    method_call, null_lit, num_lit, or_expr, paren, return_, str_lit, strict_eq, strict_neq,
};
use bynk_check::symbols::UnitTable;
use bynk_syntax::ast::{
    DEFAULT_REFERRER_POLICY, REFERRER_POLICIES, SecurityPolicy, ServiceProtocol,
};
use bynk_ts::{TsExpr, TsStmt};

/// Count the number of `:param` segments in a path pattern.
pub(super) fn param_count(path: &str) -> usize {
    path.split('/')
        .filter(|s| s.starts_with(':') && s.len() > 1)
        .count()
}

/// A CORS-enabled service and its synthesised policy (v0.131, ADR 0159). Carries
/// the emitted `CorsPolicy` object literal, the constant name it binds to, and
/// the service's distinct route paths (with a param flag) so the preflight branch
/// can match any of them.
pub(super) struct CorsService {
    pub(super) service: String,
    pub(super) const_name: String,
    pub(super) literal: TsExpr,
    pub(super) paths: Vec<(String, bool)>,
}

/// v0.139 (ADR 0162 D2): the one derivation of the methods a path answers,
/// shared by the CORS preflight's allow-methods, the `Allow` header on a
/// synthesised `405`/`OPTIONS`, and the `HEAD`-from-`GET` synthesis. It is the
/// union of the methods declared on the path, plus `HEAD` when `GET` is present,
/// plus `OPTIONS` always. The `BTreeSet` yields a stable alphabetical order
/// (`DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT`) — so `Allow: GET, HEAD,
/// OPTIONS` and `Allow: OPTIONS, POST` read as the proposal specifies.
pub(super) fn derive_allowed_methods(
    methods: impl Iterator<Item = bynk_model::IrHttpMethod>,
) -> Vec<String> {
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut has_get = false;
    for m in methods {
        let s = m.as_str().to_string();
        if s == "GET" {
            has_get = true;
        }
        set.insert(s);
    }
    if has_get {
        set.insert("HEAD".to_string());
    }
    set.insert("OPTIONS".to_string());
    set.into_iter().collect()
}

/// v0.139 (ADR 0162): a distinct route path and the method set it answers, the
/// table the method-aware router fall-through reads. `cors_const` names the
/// owning CORS-enabled service's policy constant (if any) so a synthesised
/// `405`/`OPTIONS` for a cross-origin path is stamped with `applyCors` (D5).
pub(super) struct PathMethods {
    pub(super) path: String,
    /// Alphabetical, e.g. `["GET", "HEAD", "OPTIONS"]`. #3909: the union over the
    /// routes that match every path this one does (`GET /users/me` beside
    /// `PUT /users/:id` answers a `POST` to `/users/me` with both).
    pub(super) methods: Vec<String>,
    /// #3909: routes that match only some of this path's requests, each with the
    /// `Allow` it adds when the request is one of them, decided at request time.
    pub(super) partial: Vec<(String, String)>,
    pub(super) cors_const: Option<String>,
    /// v0.141 (ADR 0164): the owning service's security policy constant, so a
    /// synthesised `405`/`OPTIONS` for the path is stamped with the same headers
    /// its real responses carry (DECISION E). `Some` for every `from http` path.
    pub(super) security_const: Option<String>,
    /// #2341 (decision C): the owning service's `on rejected` surface method,
    /// through which the synthesised `405` is rendered.
    pub(super) rejected: Option<String>,
}

/// Build the per-path method table across *every* `from http` service in the
/// context (unlike `build_cors_services`, which is scoped to CORS-enabled
/// services). Paths are emitted in sorted order for a deterministic router.
pub(super) fn build_path_method_table(
    http_routes: &[HttpRoute],
    cors_services: &[CorsService],
    security_services: &[SecurityService],
) -> Vec<PathMethods> {
    let mut paths: Vec<String> = Vec::new();
    for r in http_routes {
        if !paths.contains(&r.path) {
            paths.push(r.path.clone());
        }
    }
    paths.sort();
    // #3909: the most specific pattern first, so the first block whose pattern
    // matches the request is the one whose `Allow` is the union of its matches.
    let keys: Vec<(&str, &str)> = paths.iter().map(|p| ("", p.as_str())).collect();
    let order = bynk_check::route_order::order(&keys);
    let paths: Vec<String> = order.into_iter().map(|i| paths[i].clone()).collect();
    let allow_of = |only: &dyn Fn(&str) -> bool| {
        derive_allowed_methods(
            http_routes
                .iter()
                .filter(|r| only(&r.path))
                .map(|r| r.method),
        )
    };
    paths
        .iter()
        .map(|path| {
            let methods = allow_of(&|p| bynk_check::route_order::covers(p, path));
            let partial: Vec<(String, String)> = paths
                .iter()
                .filter(|q| {
                    *q != path
                        && bynk_check::route_order::overlap(q, path)
                        && !bynk_check::route_order::covers(q, path)
                })
                .map(|q| (q.clone(), allow_of(&|p| p == q).join(", ")))
                .collect();
            let path = path.clone();
            // A path is expected unique across a context's HTTP surface (the same
            // note ADR 0159 makes); if two services declare it, the first CORS
            // owner wins the stamping.
            let cors_const = cors_services
                .iter()
                .find(|cs| cs.paths.iter().any(|(p, _)| *p == path))
                .map(|cs| cs.const_name.clone());
            // v0.141: the owning service's security constant — the first service
            // that declares this path (paths are expected unique, so this is the
            // owner). Every `from http` service has one.
            let security_const = http_routes
                .iter()
                .find(|r| r.path == path)
                .and_then(|r| security_services.iter().find(|ss| ss.service == r.service))
                .map(|ss| ss.const_name.clone());
            let rejected = http_routes
                .iter()
                .find(|r| r.path == path)
                .and_then(|r| r.rejected.clone());
            PathMethods {
                path,
                methods,
                partial,
                cors_const,
                security_const,
                rejected,
            }
        })
        .collect()
}

/// #3925: the headers Bynk itself writes for a client to read, which every CORS
/// response exposes whether or not the author lists them: the `401` challenge
/// (#3914), `Retry-After` and `Location` (#3915) and `ETag` (`@cache`). Naming a
/// header the service never sends is harmless.
const AUTO_EXPOSED_HEADERS: [&str; 4] = ["WWW-Authenticate", "Retry-After", "Location", "ETag"];

/// #3925: the `Access-Control-Expose-Headers` list of a service: the wildcard
/// `["*"]` when the author wrote it (the checker refused it with credentials),
/// otherwise the automatic set followed by the author's names, a name already
/// present (compared without case) left out.
fn expose_headers(policy: &bynk_syntax::ast::CorsPolicy) -> Vec<String> {
    if policy.is_wildcard_expose() {
        return vec!["*".to_string()];
    }
    let mut out: Vec<String> = AUTO_EXPOSED_HEADERS.iter().map(|h| h.to_string()).collect();
    for name in policy.expose_headers() {
        if !out.iter().any(|h| h.eq_ignore_ascii_case(&name)) {
            out.push(name);
        }
    }
    out
}

/// Build the per-service CORS policies. A service with a `cors { }` section on a
/// `from http` protocol gets one `CorsService`; allow-methods is derived from its
/// routes (union + `OPTIONS`), and allow-headers defaults to `content-type` (plus
/// `Authorization` when any of its routes carries a Bearer seam) unless the author
/// overrode `headers:`.
pub(super) fn build_cors_services(
    service_names: &[&String],
    table: &UnitTable,
    http_routes: &[HttpRoute],
) -> Vec<CorsService> {
    let mut out = Vec::new();
    for sname in service_names {
        let service = table.services.get(*sname).unwrap();
        let Some(policy) = &service.cors else {
            continue;
        };
        if !matches!(service.protocol, ServiceProtocol::Http) {
            continue;
        }
        let routes: Vec<&HttpRoute> = http_routes
            .iter()
            .filter(|r| &r.service == *sname)
            .collect();
        if routes.is_empty() {
            continue;
        }

        // Allow-methods: derived from the routes via the one shared rule
        // (v0.139, ADR 0162 D2) — the union of the service's route methods, plus
        // HEAD when GET is present, plus OPTIONS always, alphabetical. So a
        // CORS-enabled service that answers GET advertises HEAD in its
        // `Access-Control-Allow-Methods` too (a cross-origin HEAD is a real
        // request), from the same table that drives the `Allow` header.
        let methods = derive_allowed_methods(routes.iter().map(|r| r.method));

        // Allow-headers: the author's override, else content-type (+ Authorization
        // when the service has a Bearer route — the header the browser must be
        // allowed to send for it).
        let allow_headers = policy.allow_headers().unwrap_or_else(|| {
            let mut hs = vec!["content-type".to_string()];
            if routes.iter().any(|r| r.bearer || r.oidc) {
                hs.push("authorization".to_string());
            }
            hs
        });

        let max_age_expr = match policy.max_age_secs() {
            Some(n) => num_lit(n.to_string()),
            None => null_lit(),
        };
        let literal = TsExpr::object(vec![
            ("origins".to_string(), str_array(&policy.origins())),
            ("allowMethods".to_string(), str_array(&methods)),
            ("allowHeaders".to_string(), str_array(&allow_headers)),
            (
                "exposeHeaders".to_string(),
                str_array(&expose_headers(policy)),
            ),
            ("credentials".to_string(), bool_lit(policy.credentials())),
            ("maxAgeSecs".to_string(), max_age_expr),
        ]);

        // Distinct route paths for the preflight match.
        let mut paths: Vec<(String, bool)> = Vec::new();
        for r in &routes {
            let entry = (r.path.clone(), param_count(&r.path) > 0);
            if !paths.contains(&entry) {
                paths.push(entry);
            }
        }

        out.push(CorsService {
            service: (*sname).clone(),
            const_name: format!("__cors_{sname}"),
            literal,
            paths,
        });
    }
    out
}

/// v0.141 (ADR 0164): a `from http` service and its synthesised security-headers
/// policy constant. Unlike `CorsService`, one exists for *every* `from http`
/// service with routes (not only those with a `security { }` block) — the safe
/// header (`nosniff`) is on by default, so a blockless service still stamps the
/// defaults.
pub(super) struct SecurityService {
    pub(super) service: String,
    pub(super) const_name: String,
    pub(super) literal: TsExpr,
}

/// #3923: whether `csp` already carries a `frame-ancestors` directive.
fn csp_has_frame_ancestors(csp: &str) -> bool {
    csp.split(';').any(|directive| {
        let d = directive.trim_start().to_ascii_lowercase();
        d.strip_prefix("frame-ancestors")
            .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
    })
}

/// #3923: the `Content-Security-Policy` and `Referrer-Policy` an HTML response gets.
///
/// With no `csp`, the policy is `frame-ancestors` alone: `'none'` by default, or the
/// declared `frameAncestors` (`"self"` as `'self'`). A declared `csp` is sent verbatim,
/// except that when it names no `frame-ancestors` of its own, the framing source list is
/// appended, so the framing default cannot be lost by writing a policy. The referrer
/// policy is the declared one, or `strict-origin-when-cross-origin`.
pub(super) fn html_security_values(policy: Option<&SecurityPolicy>) -> (String, String) {
    let sources = match policy.and_then(SecurityPolicy::frame_ancestors) {
        Some(list) if !list.is_empty() => list
            .iter()
            .map(|s| {
                if s == "self" {
                    "'self'".to_string()
                } else {
                    s.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
        _ => "'none'".to_string(),
    };
    let framing = format!("frame-ancestors {sources}");
    let csp = match policy.and_then(SecurityPolicy::csp) {
        Some(csp) if csp_has_frame_ancestors(csp) => csp.to_string(),
        Some(csp) => format!(
            "{}; {framing}",
            csp.trim_end().trim_end_matches(';').trim_end()
        ),
        None => framing,
    };
    let referrer = policy
        .and_then(SecurityPolicy::referrer_policy)
        .and_then(|camel| REFERRER_POLICIES.iter().find(|(c, _)| *c == camel))
        .map_or(DEFAULT_REFERRER_POLICY, |(_, header)| *header);
    (csp, referrer.to_string())
}

/// Build the per-service security policies. Every `from http` service with at
/// least one route gets a `__security_<service>` constant: the author's
/// `security { }` values, or the defaults (`nosniff: true`, no HSTS) when the
/// service declares no block. This is the one place the security lowering
/// diverges from `build_cors_services` (which skips a serviceless-of-a-block
/// service) — a default policy applies to every service (DECISION D).
pub(super) fn build_security_services(
    service_names: &[&String],
    table: &UnitTable,
    http_routes: &[HttpRoute],
) -> Vec<SecurityService> {
    let mut out = Vec::new();
    for sname in service_names {
        let service = table.services.get(*sname).unwrap();
        if !matches!(service.protocol, ServiceProtocol::Http) {
            continue;
        }
        if !http_routes.iter().any(|r| &r.service == *sname) {
            continue;
        }
        // The author's policy, or the safe defaults when there is no block.
        let (nosniff, hsts) = match &service.security {
            Some(p) => (p.nosniff(), p.hsts_max_age_secs()),
            None => (true, None),
        };
        let hsts_expr = match hsts {
            Some(n) => num_lit(n.to_string()),
            None => null_lit(),
        };
        let (html_csp, html_referrer) = html_security_values(service.security.as_ref());
        let literal = TsExpr::object(vec![
            ("nosniff".to_string(), bool_lit(nosniff)),
            ("hstsMaxAgeSecs".to_string(), hsts_expr),
            ("htmlCsp".to_string(), str_lit(html_csp)),
            ("htmlReferrerPolicy".to_string(), str_lit(html_referrer)),
        ]);
        out.push(SecurityService {
            service: (*sname).clone(),
            const_name: format!("__security_{sname}"),
            literal,
        });
    }
    out
}

/// The CORS preflight branch of one CORS-enabled service (v0.131, ADR 0159): an
/// `OPTIONS` against any route path of the service is answered here, before the
/// route dispatch and its auth seam, since a preflight is credential-less by spec
/// and must not be rejected by a `by` actor / Bearer check.
///
/// v0.139 (ADR 0162 D4): a *real* preflight is distinguished from a bare
/// discovery `OPTIONS` by the `Access-Control-Request-Method` header. A
/// preflight (has it) is answered here with the `Access-Control-*` grant; a bare
/// `OPTIONS` (lacks it) falls through to the generic `204 + Allow`, so the two
/// `OPTIONS` answers compose instead of colliding.
pub(super) fn cors_preflight(cs: &CorsService, security_services: &[SecurityService]) -> TsStmt {
    let mut cond_terms = cs.paths.iter().map(|(path, _)| {
        strict_neq(
            call(
                ident("__matchPath"),
                vec![str_lit(path.clone()), ident("path")],
            ),
            null_lit(),
        )
    });
    let first = cond_terms
        .next()
        .expect("a CORS service always has ≥1 route");
    let cond = cond_terms.fold(first, or_expr);
    let full_cond = and_expr(
        and_expr(
            strict_eq(ident("method"), str_lit("OPTIONS")),
            strict_neq(
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit("access-control-request-method")],
                ),
                null_lit(),
            ),
        ),
        paren(cond),
    );
    // v0.141 (ADR 0164 DECISION E): the synthesised preflight also carries the
    // service's security headers — a CORS-enabled service is a `from http`
    // service, so it always has a security policy constant.
    let preflight = call(
        ident("__corsPreflightResponse"),
        vec![
            ident(cs.const_name.clone()),
            method_call(
                member(ident("request"), "headers"),
                "get",
                vec![str_lit("origin")],
            ),
        ],
    );
    let stamped = match security_services.iter().find(|s| s.service == cs.service) {
        Some(ss) => call(
            ident("__applySecurityHeaders"),
            vec![preflight, ident(ss.const_name.clone())],
        ),
        None => preflight,
    };
    if_(full_cond, block(vec![return_(Some(stamped))]))
}

/// The `Allow` value of a path's fall-through: its methods, plus (#3909) those of
/// the routes that match only some of its requests, added at request time when the
/// request is one of them.
fn allow_expr(pm: &PathMethods, allow: String) -> TsExpr {
    if pm.partial.is_empty() {
        return str_lit(allow);
    }
    let extras = pm
        .partial
        .iter()
        .map(|(pattern, methods)| {
            TsExpr::array(vec![str_lit(pattern.clone()), str_lit(methods.clone())])
        })
        .collect();
    call(
        ident("__allowUnion"),
        vec![str_lit(allow), ident("path"), TsExpr::array(extras)],
    )
}

/// The method-aware router fall-through for one known path (v0.139, ADR 0162): a
/// request that matched no dispatch block and reaches a live path under an
/// unhandled method is a bare `OPTIONS` ⇒ `204 + Allow`, any other method ⇒
/// `405 + Allow` (the `Allow` derived from the route table). For a CORS-enabled
/// path the synthesised response is stamped with `applyCors` (D5) so a
/// cross-origin `405`/`OPTIONS` is not invisible to the browser.
pub(super) fn method_fall_through(pm: &PathMethods, member_mode: bool) -> TsStmt {
    let allow = pm.methods.join(", ");
    let match_cond = strict_neq(
        call(
            ident("__matchPath"),
            vec![str_lit(pm.path.clone()), ident("path")],
        ),
        null_lit(),
    );
    let mut fallthrough_body: Vec<TsStmt> = Vec::new();
    // OPTIONS → 204, everything else → 405; both carry the derived Allow.
    fallthrough_body.push(const_(
        "__status",
        cond_expr(
            strict_eq(ident("method"), str_lit("OPTIONS")),
            num_lit("204"),
            num_lit("405"),
        ),
    ));
    let res = new_response(vec![
        null_lit(),
        TsExpr::object(vec![
            ("status".to_string(), ident("__status")),
            (
                "headers".to_string(),
                TsExpr::object(vec![("allow".to_string(), allow_expr(pm, allow))]),
            ),
        ]),
    ]);
    // #2341 (decision C): a `405` is rendered through the owning service's
    // `on rejected` (the `204` to `OPTIONS` is not a rejection, and passes
    // through untouched).
    let res = match &pm.rejected {
        Some(m) => await_expr(call(
            ident("__renderRejection"),
            vec![res, rejection_renderer(m)],
        )),
        None => res,
    };
    fallthrough_body.push(const_("__res", res));
    // Stamp CORS (v0.131) then security headers (v0.141) — disjoint sets, so
    // the order is immaterial; every `from http` path carries the security
    // policy (DECISION E), CORS only when the path's service opts in.
    let cors_res = match &pm.cors_const {
        Some(c) => call(
            ident("__applyCors"),
            vec![
                ident("__res"),
                ident(c.clone()),
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit("origin")],
                ),
            ],
        ),
        None => ident("__res"),
    };
    let stamped = match &pm.security_const {
        Some(s) => call(
            ident("__applySecurityHeaders"),
            vec![cors_res, ident(s.clone())],
        ),
        None => cors_res,
    };
    fallthrough_body.push(return_(Some(miss(member_mode, stamped))));
    if_(match_cond, block(fallthrough_body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_model::IrHttpMethod;

    #[test]
    fn a_csp_with_its_own_frame_ancestors_is_left_alone() {
        assert!(csp_has_frame_ancestors("frame-ancestors 'self'"));
        assert!(csp_has_frame_ancestors(
            "default-src 'self'; Frame-Ancestors https://a.example"
        ));
        assert!(csp_has_frame_ancestors(
            "default-src 'self'; frame-ancestors"
        ));
        assert!(!csp_has_frame_ancestors(
            "default-src 'self'; img-src 'self'"
        ));
        // A directive that merely starts with the name is another directive.
        assert!(!csp_has_frame_ancestors("frame-ancestors-extra 'self'"));
        assert!(!csp_has_frame_ancestors(
            "default-src 'self' frame-ancestors"
        ));
    }

    #[test]
    fn a_service_with_no_security_block_frames_nothing_and_sends_the_default_referrer_policy() {
        let (csp, referrer) = html_security_values(None);
        assert_eq!(csp, "frame-ancestors 'none'");
        assert_eq!(referrer, "strict-origin-when-cross-origin");
    }

    #[test]
    fn param_count_counts_named_segments_only() {
        assert_eq!(param_count("/items"), 0);
        assert_eq!(param_count("/items/:id"), 1);
        assert_eq!(param_count("/a/:x/b/:y"), 2);
        // A bare `:` names nothing.
        assert_eq!(param_count("/a/:"), 0);
    }

    #[test]
    fn allowed_methods_add_head_for_get_and_always_options_in_order() {
        assert_eq!(
            derive_allowed_methods([IrHttpMethod::Get].into_iter()),
            ["GET", "HEAD", "OPTIONS"]
        );
        assert_eq!(
            derive_allowed_methods([IrHttpMethod::Post].into_iter()),
            ["OPTIONS", "POST"]
        );
        assert_eq!(
            derive_allowed_methods(
                [
                    IrHttpMethod::Put,
                    IrHttpMethod::Get,
                    IrHttpMethod::Delete,
                    IrHttpMethod::Get
                ]
                .into_iter()
            ),
            ["DELETE", "GET", "HEAD", "OPTIONS", "PUT"]
        );
        assert_eq!(derive_allowed_methods(std::iter::empty()), ["OPTIONS"]);
    }
}
