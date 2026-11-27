//! Durable event delivery, slice 3b (#3898): each `from Events(E)` service's compiler-owned
//! event queue, as its Worker consumes it.
//!
//! One place answers "which event queues does this context consume", so the Worker's
//! `[[queues.consumers]]` (`workers_emit`), its bindings (the dead-letter producer and
//! `D1_IDEMPOTENCY`, `bindings.rs`) and its `queue` entry (`workers_entry/event_queue.rs`) agree by
//! construction. The names are the default environment's (`bynk_project::environment`); a named
//! environment's `[env.<name>]` block qualifies both queues as it qualifies any consumer's
//! (`wrangler/environment.rs`, `QualifiedQueues`).

use bynk_check::symbols::{QueueDelivery, UnitTable};
use bynk_check::worker_bindings::{BindingSource, WorkerBinding};
use bynk_project::environment::{event_dead_letter_queue_name, event_queue_name};

/// D5: the retries an event queue's consumer is configured with (Cloudflare's default). The
/// runtime's `EVENT_QUEUE_RETRIES` must agree: the entry logs the last attempt as dead-lettered.
pub(crate) const EVENT_QUEUE_RETRIES: i64 = 3;

/// One subscribing service's event queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventQueue {
    /// The subscribing service.
    pub(crate) service: String,
    /// The event it subscribes to.
    pub(crate) event: String,
    /// Its event queue, in the default environment.
    pub(crate) queue: String,
    /// Its event dead-letter queue, in the default environment.
    pub(crate) dead_letter: String,
}

/// Every `from Events` service of `context` with its queues, sorted by queue name.
pub(crate) fn event_queues(context: &str, table: &UnitTable) -> Vec<EventQueue> {
    let mut out: Vec<EventQueue> = table
        .services
        .iter()
        .filter_map(|(service, decl)| {
            let bynk_syntax::ast::ServiceProtocol::Events { event_type, .. } = &decl.protocol
            else {
                return None;
            };
            let bynk_syntax::ast::TypeRef::Named(event) = event_type else {
                return None;
            };
            Some(EventQueue {
                service: service.clone(),
                event: event.name.clone(),
                queue: event_queue_name("default", context, service)?,
                dead_letter: event_dead_letter_queue_name("default", context, service)?,
            })
        })
        .collect();
    out.sort_by(|a, b| a.queue.cmp(&b.queue));
    out
}

/// Whether `table` declares a subscribing service, and so claims each delivery's `eventId` in
/// `D1_IDEMPOTENCY` (D11).
pub(crate) fn subscribes(table: &UnitTable) -> bool {
    table
        .services
        .values()
        .any(|s| matches!(s.protocol, bynk_syntax::ast::ServiceProtocol::Events { .. }))
}

/// Each event queue's `[[queues.consumers]]` settings: its dead-letter queue and `retries`. The
/// entry delays each retry itself (`retryDelay × attempts`), so no `retry_delay` is written.
pub(crate) fn event_queue_deliveries(queues: &[EventQueue]) -> Vec<QueueDelivery> {
    queues
        .iter()
        .map(|q| QueueDelivery {
            queue: q.queue.clone(),
            retries: Some(EVENT_QUEUE_RETRIES),
            dead_letter: Some(q.dead_letter.clone()),
            ..QueueDelivery::default()
        })
        .collect()
}

/// Each event queue's `__BYNK_DLQ_<QUEUE>` producer binding, through which the entry dead-letters
/// a verified message it cannot decode (#3878's mechanism, D5).
pub(crate) fn event_dead_letter_bindings(queues: &[EventQueue]) -> Vec<WorkerBinding> {
    queues
        .iter()
        .map(|q| WorkerBinding {
            name: crate::emitter::workers_entry::dead_letter_binding(&q.queue),
            source: BindingSource::QueueDeadLetter {
                queue: q.dead_letter.clone(),
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A context with no subscriber has no event queue and needs no claim database.
    #[test]
    fn a_context_without_subscribers_has_no_event_queue() {
        let table = UnitTable::default();
        assert!(event_queues("shop.ledger", &table).is_empty());
        assert!(!subscribes(&table));
    }

    /// D5: the consumer dead-letters to its own queue after Cloudflare's three retries, and its
    /// dead-letter producer is keyed by the event queue's name.
    #[test]
    fn each_event_queue_dead_letters_to_its_own_queue() {
        let q = EventQueue {
            service: "OnPlaced".into(),
            event: "Placed".into(),
            queue: event_queue_name("default", "shop.ledger", "OnPlaced").unwrap(),
            dead_letter: event_dead_letter_queue_name("default", "shop.ledger", "OnPlaced").unwrap(),
        };
        let [d] = event_queue_deliveries(std::slice::from_ref(&q)).try_into().unwrap();
        assert_eq!(d.retries, Some(3));
        assert_eq!(d.dead_letter.as_deref(), Some(q.dead_letter.as_str()));
        assert!(q.queue.starts_with("bynk-ev-") && q.dead_letter.starts_with("bynk-dl-"));
        let [b] = event_dead_letter_bindings(&[q.clone()]).try_into().unwrap();
        assert_eq!(b.name, crate::emitter::workers_entry::dead_letter_binding(&q.queue));
        assert!(matches!(b.source, BindingSource::QueueDeadLetter { queue } if queue == q.dead_letter));
    }
}
