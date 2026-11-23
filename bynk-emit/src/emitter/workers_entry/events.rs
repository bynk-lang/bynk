//! The events delivery route of the Worker entry (spine #936, ADR 0284).

use super::call::verify_gate;
use super::respond::{
    case_, default_case, json_response, miss, null_response, own_path_guard, set_fault_route,
    text_response,
};
use super::wire::deserialise_call;
use crate::emitter::RuntimeUse;
use crate::ts_build::{
    as_expr, await_expr, block, call, const_, expr_stmt, ident, if_, member, method_call, return_,
    str_lit, strict_eq, switch_stmt,
};
use bynk_check::symbols::UnitTable;
use bynk_ts::{TsBindingName, TsStmt, TsSwitchCase, TsType, TsTypeMember};

/// Events dispatch (spine #936, ADR 0284). Reached only from a publishing
/// context's fan-out DO calling in over this subscriber's Service Binding
/// (`deliverEvent`) — never from external edge traffic, so it needs no
/// CORS/actor handling, unlike the HTTP routes. `None` when no service here
/// subscribes to an event.
pub(super) fn events_delivery_route(
    context: &str,
    member_mode: bool,
    table: &UnitTable,
    service_names: &[&String],
    runtime_use: &RuntimeUse,
) -> Option<TsStmt> {
    let event_services: Vec<&String> = service_names
        .iter()
        .filter(|sname| {
            table.services.get(**sname).is_some_and(|s| {
                s.handlers.iter().any(|h| {
                    matches!(
                        bynk_model::lower_handler_kind_ir(&h.kind),
                        bynk_model::IrHandlerKind::Event
                    )
                })
            })
        })
        .copied()
        .collect();
    if event_services.is_empty() {
        return None;
    }
    let mut event_body: Vec<TsStmt> = verify_gate(context, false).into_iter().collect();
    event_body.extend([
        const_(
            "servicePath",
            method_call(
                ident("path"),
                "slice",
                vec![member(str_lit("/_bynk/event/"), "length")],
            ),
        ),
        // Slice 2 (spine #936): the fan-out DO's `deliverEvent` always sends
        // `{ payload, envelope }` — the envelope is minted at emission
        // (`lower.rs`'s `__events.push`) and forwarded unconditionally over
        // this hop. Whether a *specific* subscriber's handler actually
        // wants it is decided by that subscriber's own generated wrapper
        // (`emit_event_wrapper`), not here — this route passes both through
        // uniformly regardless of which `case` it dispatches to.
        //
        // #973: until this fix, `payload`/`envelope` were cast `as unknown`
        // and forwarded unchecked — nothing on this whole path ever called a
        // `deserialise_*` function, so a malformed event silently reached the
        // subscriber's handler body. Both are now validated here, the same
        // validate-then-400 shape `/_bynk/call/` above already uses: the
        // envelope once, unconditionally (it's always on the wire regardless
        // of whether a given subscriber declared `env`), then the payload
        // per-`case`, against that subscriber's own generated codec (change A
        // in #973 — the codec didn't previously exist at all for a pure
        // subscriber, since it calls no method on the publisher).
        TsStmt::const_stmt(
            TsBindingName::ObjectPattern(vec!["payload".to_string(), "envelope".to_string()]),
            None,
            as_expr(
                // I2 (#1971): the text `__verifyCall` read and verified.
                method_call(ident("globalThis.JSON"), "parse", vec![ident("__body")]),
                TsType::Object(vec![
                    TsTypeMember::prop("payload", TsType::named("__JsonValue")),
                    TsTypeMember::prop("envelope", TsType::named("__JsonValue")),
                ]),
            ),
            None,
        ),
        const_(
            "__r_envelope",
            call(
                ident("__deserialiseEventEnvelope"),
                vec![ident("envelope"), str_lit("$.envelope")],
            ),
        ),
    ]);
    event_body.push(if_(
        strict_eq(member(ident("__r_envelope"), "tag"), str_lit("Err")),
        return_(Some(json_response(
            member(ident("__r_envelope"), "error"),
            400,
        ))),
    ));

    let mut event_cases: Vec<TsSwitchCase> = Vec::new();
    for sname in &event_services {
        let h = table.services[*sname]
            .handlers
            .iter()
            .find(|h| {
                matches!(
                    bynk_model::lower_handler_kind_ir(&h.kind),
                    bynk_model::IrHandlerKind::Event
                )
            })
            .expect("event_services filtered to services with an Event handler");
        let dser_payload = deserialise_call(
            &h.params[0].type_ref,
            &ident("payload"),
            "$.payload",
            runtime_use,
        );
        let case_body = vec![
            // #1825 review: a subscriber that throws is a fault the
            // publisher's fan-out retries; name it for the log.
            set_fault_route(format!("event {sname}")),
            const_("__r_payload", dser_payload),
            if_(
                strict_eq(member(ident("__r_payload"), "tag"), str_lit("Err")),
                return_(Some(json_response(
                    member(ident("__r_payload"), "error"),
                    400,
                ))),
            ),
            expr_stmt(await_expr(call(
                member(ident("surface"), sname.to_string()),
                vec![
                    member(ident("__r_payload"), "value"),
                    member(ident("__r_envelope"), "value"),
                ],
            ))),
            return_(Some(null_response(204))),
        ];
        event_cases.push(case_(str_lit(sname.to_string()), case_body));
    }
    event_cases.push(default_case(vec![return_(Some(miss(
        member_mode,
        text_response("Not found", 404),
    )))]));
    event_body.push(switch_stmt(ident("servicePath"), event_cases));

    let event_names: Vec<String> = event_services.iter().map(|s| s.to_string()).collect();
    Some(if_(
        own_path_guard(member_mode, "/_bynk/event/", &event_names),
        block(event_body),
    ))
}
