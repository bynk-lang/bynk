//! #3752 (pass-through HTTP, slice 3): the router's side of an `on ANY(…)` route
//! (`design/tracks/pass-through-http.md` §6, D5, D7, D9, D10). An `ANY` route
//! answers every method on the paths it owns, yields every other path to the
//! ordinary routes, and passes an upstream's answer through with nothing
//! synthesised: no `ETag`, `304`, `HEAD` body, `405`/`Allow` or CORS.

use super::http::HttpRoute;
use crate::ts_build::{bool_lit, call, ident, if_, return_, str_lit};
use bynk_model::IrHttpMethod;
use bynk_ts::{TsExpr, TsStmt};

/// Whether `route` is a pass-through route.
pub(super) fn is_any(route: &HttpRoute) -> bool {
    route.method == IrHttpMethod::Any
}

/// D5: sets each `ANY` route's `pass_through_yields`, the ordinary routes (of
/// every service and method) that own the paths the two share.
pub(super) fn set_yields(routes: &mut [HttpRoute]) {
    let keys: Vec<(&str, &str)> = routes
        .iter()
        .map(|r| (r.method.as_str(), r.path.as_str()))
        .collect();
    let yields: Vec<(usize, Vec<String>)> = bynk_check::route_order::pass_through_yields(&keys)
        .into_iter()
        .map(|(r, ys)| (r, ys.into_iter().map(|y| routes[y].path.clone()).collect()))
        .collect();
    for (r, ys) in yields {
        routes[r].pass_through_yields = ys;
    }
}

fn patterns(paths: &[String]) -> TsExpr {
    TsExpr::array(paths.iter().map(|p| str_lit(p.clone())).collect())
}

/// The method guard of an `ANY` route: every method, on a path no ordinary route
/// owns (`!__passThroughYields(path, [...])`).
pub(super) fn guard(route: &HttpRoute) -> TsExpr {
    if route.pass_through_yields.is_empty() {
        return bool_lit(true);
    }
    TsExpr::Unary {
        op: bynk_ts::TsUnaryOp::Not,
        expr: Box::new(call(
            ident("__passThroughYields"),
            vec![ident("path"), patterns(&route.pass_through_yields)],
        )),
    }
}

/// D10: wraps a CORS preflight so it never answers for a path an `ANY` route
/// owns. Unchanged when the Worker has no `ANY` route.
pub(super) fn guard_preflight(preflight: TsStmt, routes: &[HttpRoute]) -> TsStmt {
    let owners: Vec<TsExpr> = routes
        .iter()
        .filter(|r| is_any(r))
        .map(|r| TsExpr::array(vec![str_lit(r.path.clone()), patterns(&r.pass_through_yields)]))
        .collect();
    if owners.is_empty() {
        return preflight;
    }
    if_(
        TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(call(
                ident("__passThroughOwns"),
                vec![ident("path"), TsExpr::array(owners)],
            )),
        },
        crate::ts_build::block(vec![preflight]),
    )
}

/// D7, D15: the `501` an `ANY` route answers, before its handler runs, to a method
/// outside the seven it forwards or to an `Upgrade`. `refused` is the stamped
/// `501` response.
pub(super) fn refusal(refused: TsExpr) -> TsStmt {
    if_(
        call(ident("__passThroughRefuses"), vec![ident("request")]),
        return_(Some(refused)),
    )
}

/// D11: the `request: InboundRequest` argument, over the route's own headers view
/// (`headers`, its consumed credential hidden).
pub(super) fn inbound_request_arg(headers: TsExpr) -> TsExpr {
    call(ident("__inboundRequest"), vec![ident("request"), headers])
}

/// D9: the response a handler's `PassThrough` (`result`) becomes.
pub(super) fn response(opts: TsExpr) -> TsExpr {
    call(
        ident("__passThroughResponse"),
        vec![ident("result"), ident("request"), opts],
    )
}

/// D9: the service's security headers, filled rather than overwritten where an
/// upstream sent its own.
pub(super) fn secured(response: TsExpr, policy: &str) -> TsExpr {
    call(
        ident("__applyPassThroughSecurity"),
        vec![response, ident(policy.to_string())],
    )
}
