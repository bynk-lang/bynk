//! The `queue` entry's event-queue dispatch (#3898, slice 3b): a `from Events(E)` service's Worker
//! consumes its compiler-owned event queue (`emitter::event_queues`).
//!
//! Each message is handed to the runtime's `__bynkEventQueueDeliver` (`runtime/src/queue_admit.ts`,
//! `deliverEventMessage`), which verifies it as signed by the event's owner before anything is
//! decoded (D2), claims its `eventId` for the subscriber (D11), and acknowledges, retries or
//! dead-letters it (D5, D9). What the entry knows at compile time it passes in: the subscriber's
//! qualified name, the event, its owner, the dead-letter producer binding, the payload's
//! deserialiser and the compose surface's subscriber method, the one the legacy `/_bynk/event/`
//! route calls too.

use super::wire::deserialise_call;
use crate::emitter::RuntimeUse;
use crate::emitter::event_queues::{EventQueue, event_queues};
use crate::ts_build::{
    arrow, as_expr, await_expr, block, call, expr_stmt, ident, if_, member, method_call, or_expr,
    return_, str_lit, strict_eq,
};
use bynk_check::symbols::UnitTable;
use bynk_ts::{TsExpr, TsParam, TsStmt, TsType};
use std::collections::BTreeMap;

/// One subscribing service the entry consumes the event queue of.
#[derive(Debug, Clone)]
pub(super) struct EventQueueRoute {
    queue: EventQueue,
    /// The qualified subscriber, `<context>.<service>`: the D11 key's consumer.
    subscriber: String,
    /// The context declaring the event, the only sender admitted.
    owner: String,
    payload: bynk_syntax::ast::TypeRef,
}

/// The event queues `context` consumes, with the owner of each event (`owners`, by service). A
/// service whose owner is unknown, or whose `on event` handler is missing, has no route.
pub(super) fn collect_event_queue_routes(
    context: &str,
    table: &UnitTable,
    owners: &BTreeMap<String, String>,
) -> Vec<EventQueueRoute> {
    event_queues(context, table)
        .into_iter()
        .filter_map(|queue| {
            let owner = owners.get(&queue.service)?.clone();
            let handler = table.services.get(&queue.service)?.handlers.iter().find(|h| {
                matches!(
                    bynk_model::lower_handler_kind_ir(&h.kind),
                    bynk_model::IrHandlerKind::Event
                )
            })?;
            Some(EventQueueRoute {
                subscriber: format!("{context}.{}", queue.service),
                owner,
                payload: handler.params.first()?.type_ref.clone(),
                queue,
            })
        })
        .collect()
}

/// `if (batch.queue === "<queue>" || batch.queue.startsWith("<queue>-")) { for (const msg of
/// batch.messages) await __bynkEventQueueDeliver(…); return; }`, one per route. The prefix test
/// admits the queue's environment-qualified name (`<queue>-<environment>`, `env_qualify`), which is
/// also the name the publisher signed for and so the one passed on.
pub(super) fn event_queue_dispatch(routes: &[EventQueueRoute], ru: &RuntimeUse) -> Vec<TsStmt> {
    routes
        .iter()
        .map(|route| {
            let q = &route.queue.queue;
            let batch_queue = || member(ident("batch"), "queue");
            let ours = or_expr(
                strict_eq(batch_queue(), str_lit(q.clone())),
                method_call(batch_queue(), "startsWith", vec![str_lit(format!("{q}-"))]),
            );
            let spec = TsExpr::object(vec![
                ("subscriber".to_string(), str_lit(route.subscriber.clone())),
                ("event".to_string(), str_lit(route.queue.event.clone())),
                ("owner".to_string(), str_lit(route.owner.clone())),
                (
                    "deadLetter".to_string(),
                    str_lit(super::dead_letter_binding(q)),
                ),
            ]);
            let decode = arrow(
                vec![TsParam::new("p", Some(TsType::named("__JsonValue")))],
                deserialise_call(&route.payload, &ident("p"), "$.payload", ru),
            );
            let run = arrow(
                vec![
                    TsParam::new("payload", Some(TsType::named("unknown"))),
                    TsParam::new("envelope", Some(TsType::named("unknown"))),
                ],
                call(
                    member(ident("surface"), route.queue.service.clone()),
                    vec![ident("payload"), ident("envelope")],
                ),
            );
            let report = arrow(
                vec![TsParam::new("e", Some(TsType::named("unknown")))],
                call(ident("__bynkReportFault"), vec![ident("ctx"), ident("e")]),
            );
            let deliver = expr_stmt(await_expr(call(
                ident("__bynkEventQueueDeliver"),
                vec![
                    ident("msg"),
                    batch_queue(),
                    spec,
                    as_expr(ident("env"), TsType::named("unknown")),
                    report,
                    decode,
                    run,
                ],
            )));
            if_(
                ours,
                block(vec![
                    TsStmt::for_of("msg", member(ident("batch"), "messages"), block(vec![deliver]), None),
                    return_(None),
                ]),
            )
        })
        .collect()
}
