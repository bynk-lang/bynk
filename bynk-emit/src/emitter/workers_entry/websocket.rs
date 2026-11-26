//! The `from websocket` upgrade routes of the Worker entry (v0.104, slice 3b).

use super::respond::{set_fault_route, text_response};
use crate::ts_build::{
    await_expr, block, call, const_, ident, if_, member, method_call, null_lit, return_, str_lit,
    strict_eq,
};
use bynk_check::symbols::UnitTable;
use bynk_syntax::ast::Handler;
use bynk_ts::{TsExpr, TsStmt};

/// The `from websocket` upgrade routes: every `open` handler, in service order.
pub(super) fn collect_ws_open_routes<'a>(
    table: &'a UnitTable,
    service_names: &[&'a String],
) -> Vec<(&'a String, &'a Handler)> {
    let mut ws_open_routes: Vec<(&String, &Handler)> = Vec::new();
    for sname in service_names {
        let service = table.services.get(*sname).unwrap();
        for h in &service.handlers {
            if matches!(
                bynk_model::lower_handler_kind_ir(&h.kind),
                bynk_model::IrHandlerKind::Open
            ) {
                ws_open_routes.push((*sname, h));
            }
        }
    }
    ws_open_routes
}

/// The `Upgrade: websocket` branch of `fetch` (v0.104, slice 3b). An upgrade
/// request routes to the `from websocket` service's edge wrapper, which runs the
/// fail-closed auth seam and forwards to the hosting Durable Object. Route params
/// are read from the upgrade URL's query string by name (the v1 convention; a
/// missing required param is a `400`).
pub(super) fn ws_upgrade_dispatch(
    ws_open_routes: &[(&String, &Handler)],
    table: &UnitTable,
    context: &str,
) -> TsStmt {
    let mut ws_body: Vec<TsStmt> = Vec::new();
    for (sname, h) in ws_open_routes {
        // #1825 review: the auth seam and the DO forward run in the `try`
        // too, so name the dispatch for the fault log.
        ws_body.push(set_fault_route(format!("ws {sname}")));
        let mut args: Vec<TsExpr> = vec![ident("request")];
        for p in &h.params {
            let pn = &p.name.name;
            ws_body.push(const_(
                format!("__ws_{pn}"),
                method_call(
                    member(ident("url"), "searchParams"),
                    "get",
                    vec![str_lit(pn.clone())],
                ),
            ));
            ws_body.push(if_(
                strict_eq(ident(format!("__ws_{pn}")), null_lit()),
                return_(Some(text_response(
                    &format!("Missing parameter: {pn}"),
                    400,
                ))),
            ));
            args.push(ident(format!("__ws_{pn}")));
        }
        // #3914: an upgrade refused with `401` carries the actor's challenge, as an HTTP route's.
        let challenges = super::http::route_challenges(h, table, context)
            .into_iter()
            .map(str_lit)
            .collect();
        ws_body.push(return_(Some(call(
            ident("__applyChallenge"),
            vec![
                await_expr(call(
                    member(ident("surface"), format!("ws_{sname}_open")),
                    args,
                )),
                ident("request"),
                TsExpr::Array {
                    items: challenges,
                    multiline: false,
                },
            ],
        ))));
    }
    if_(
        strict_eq(
            method_call(
                member(ident("request"), "headers"),
                "get",
                vec![str_lit("Upgrade")],
            ),
            str_lit("websocket"),
        ),
        block(ws_body),
    )
}
