//! #3752: the routes `bynk openapi` and `bynk client` leave out, and why. Each
//! is named on stderr, never generated wrong.

use super::{Selected, select_services};
use bynk_driver::emsg;
use bynk_ide::ProjectDiagnostics;
use bynk_model::{IrHandlerKind, lower_handler_kind_ir};

/// #3752 (D6): why a route with a `*name` rest is left out. An OpenAPI path
/// parameter can't contain `/`, and a client that sent the rest as one segment
/// would encode its slashes.
pub(crate) const REST_LEFT_OUT: &str = "a `*` rest segment matches any number of segments, which an OpenAPI path parameter and a client's encoded segment can't express";

/// #3752 (D7): why an `ANY` pass-through route is left out: its methods,
/// statuses and bodies are the upstream's.
pub(crate) const PASS_THROUGH_LEFT_OUT: &str =
    "a pass-through route answers with whatever its upstream does, which no operation describes";

/// Why the route `method path` is left out of the document and the client, or
/// `None` when it is described.
pub(crate) fn left_out_because(method: &str, path: &str) -> Option<&'static str> {
    if method == "ANY" {
        Some(PASS_THROUGH_LEFT_OUT)
    } else {
        bynk_check::http_rest::rest_name(path).map(|_| REST_LEFT_OUT)
    }
}

/// The routes of `selected` the document leaves out, each as
/// `` `GET /x` of `service`: why ``.
fn routes_left_out(selected: &[Selected]) -> Vec<String> {
    let mut out = Vec::new();
    for sel in selected {
        for h in &sel.service.handlers {
            if let IrHandlerKind::Http { method, path } = lower_handler_kind_ir(&h.kind)
                && let Some(why) = left_out_because(method.as_str(), &path)
            {
                out.push(format!(
                    "`{} {path}` of `{}`: {why}",
                    method.as_str(),
                    sel.service.name.name
                ));
            }
        }
    }
    out
}

/// Whether the document skips `h`: it is not an HTTP route, or it is one the
/// document leaves out (and [`warn`] names).
pub(super) fn not_described(h: &bynk_syntax::ast::Handler) -> bool {
    match lower_handler_kind_ir(&h.kind) {
        IrHandlerKind::Http { method, path } => left_out_because(method.as_str(), &path).is_some(),
        _ => true,
    }
}

/// Names each route of the selected services the document leaves out on stderr.
pub(super) fn warn(analysis: &ProjectDiagnostics, service: Option<&str>) {
    let want: Vec<&str> = service.into_iter().collect();
    if let Ok(selected) = select_services(analysis, &want) {
        for s in routes_left_out(&selected) {
            emsg!("bynk: warning: skipped {s}");
        }
    }
}
