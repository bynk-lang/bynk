//! The HTTP policy of the Worker entry: the CORS services, the security-header services and the
//! method table, each derived from the route table.

use super::http::HttpRoute;
use super::respond::{miss, new_response, rejection_renderer, str_array};
use crate::ts_build::{
    and_expr, await_expr, block, bool_lit, call, cond_expr, const_, ident, if_, member,
    method_call, null_lit, num_lit, or_expr, paren, return_, str_lit, strict_eq, strict_neq,
};
use bynk_check::symbols::UnitTable;
use bynk_syntax::ast::ServiceProtocol;
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
    pub(super) has_params: bool,
    /// Alphabetical, e.g. `["GET", "HEAD", "OPTIONS"]`.
    pub(super) methods: Vec<String>,
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
    paths
        .into_iter()
        .map(|path| {
            let methods = derive_allowed_methods(
                http_routes
                    .iter()
                    .filter(|r| r.path == path)
                    .map(|r| r.method),
            );
            let has_params = param_count(&path) > 0;
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
                has_params,
                methods,
                cors_const,
                security_const,
                rejected,
            }
        })
        .collect()
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
        let literal = TsExpr::object(vec![
            ("nosniff".to_string(), bool_lit(nosniff)),
            ("hstsMaxAgeSecs".to_string(), hsts_expr),
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
    let mut cond_terms = cs.paths.iter().map(|(path, has_params)| {
        if *has_params {
            strict_neq(
                call(
                    ident("__matchPath"),
                    vec![str_lit(path.clone()), ident("path")],
                ),
                null_lit(),
            )
        } else {
            strict_eq(ident("path"), str_lit(path.clone()))
        }
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

/// The method-aware router fall-through for one known path (v0.139, ADR 0162): a
/// request that matched no dispatch block and reaches a live path under an
/// unhandled method is a bare `OPTIONS` ⇒ `204 + Allow`, any other method ⇒
/// `405 + Allow` (the `Allow` derived from the route table). For a CORS-enabled
/// path the synthesised response is stamped with `applyCors` (D5) so a
/// cross-origin `405`/`OPTIONS` is not invisible to the browser.
pub(super) fn method_fall_through(pm: &PathMethods, member_mode: bool) -> TsStmt {
    let allow = pm.methods.join(", ");
    let match_cond = if pm.has_params {
        strict_neq(
            call(
                ident("__matchPath"),
                vec![str_lit(pm.path.clone()), ident("path")],
            ),
            null_lit(),
        )
    } else {
        strict_eq(ident("path"), str_lit(pm.path.clone()))
    };
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
                TsExpr::object(vec![("allow".to_string(), str_lit(allow))]),
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
