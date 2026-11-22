//! Events in the composition (back-half paydown 1B.1c, Decisions F and I of
//! #3928): what is subscribed to what, and the failure-isolation statement
//! every delivery path shares.
//!
//! The subscriber table is one function, `bynk_check::symbols::
//! discover_event_subscribers`. This module holds the facts built on top of it
//! (each subscription's envelope decision) and the one builder of ADR 0284's
//! guarantee, so the Bundle root's in-process dispatch and the Workers fan-out
//! Durable Object cannot drift: neither spells the `try`/`catch` itself.

use std::collections::BTreeMap;

use crate::ts_build::{call, ident, member, str_lit};
use bynk_ts::{TsExpr, TsStmt};

/// The message a failed delivery logs. The structural guard of Decision I
/// (`xtask`'s `delivery_isolation_has_one_home`) counts this literal in
/// production source: it must appear once, here, so a second copy of the
/// isolation code is a CI failure.
const DELIVERY_FAILED: &str = "EventsFanout delivery failed";

/// One subscription: a service that subscribes to a publisher's event, and
/// whether a delivery to it carries the event's envelope.
///
/// Every subscription is delivered in isolation (ADR 0284): that is the rule
/// of `isolated_delivery`, not a per-subscription choice, so it is not a
/// field here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSubscription {
    pub context: String,
    pub service: String,
    /// The subscriber's `EventSubscriberShape::wants_envelope`.
    pub wants_envelope: bool,
}

/// Who subscribes to what, keyed `(publishing context, event)`. Plain facts:
/// each renderer decides how to deliver.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventPlan {
    /// Each event's subscribers, in `discover_event_subscribers`' order.
    pub subscriptions: BTreeMap<(String, String), Vec<EventSubscription>>,
}

impl EventPlan {
    /// The subscriptions to the events `publisher` declares, by event name.
    pub fn published_by<'a>(
        &'a self,
        publisher: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a [EventSubscription])> {
        self.subscriptions
            .iter()
            .filter(move |((p, _), _)| p == publisher)
            .map(|((_, event), subs)| (event.as_str(), subs.as_slice()))
    }
}

/// `try { await <delivery>; } catch (e) { globalThis.console.error("EventsFanout
/// delivery failed", { event: ev.type, service: <service>, error: String(e) }); }`.
///
/// ADR 0284: subscriber failure isolation. One subscriber's rejection is logged
/// and does not stop delivery to the remaining subscribers, nor does it
/// propagate back to the (already-committed) publishing handler. Both targets
/// build it here: the Bundle's in-process dispatch and the Workers fan-out
/// Durable Object. `delivery` is the call that delivers, spelled by the
/// renderer; `service` is the expression the log names the subscriber by.
/// Both read the event as `ev`.
pub(crate) fn isolated_delivery(delivery: TsExpr, service: TsExpr) -> TsStmt {
    TsStmt::try_catch(
        TsStmt::block(
            vec![TsStmt::expr_stmt(TsExpr::Await(Box::new(delivery)), None)],
            None,
        ),
        Some("e"),
        TsStmt::block(
            vec![TsStmt::expr_stmt(
                call(
                    member(ident("globalThis.console"), "error"),
                    vec![
                        str_lit(DELIVERY_FAILED),
                        TsExpr::object(vec![
                            ("event".to_string(), member(ident("ev"), "type")),
                            ("service".to_string(), service),
                            ("error".to_string(), call(ident("String"), vec![ident("e")])),
                        ]),
                    ],
                ),
                None,
            )],
            None,
        ),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_publishers_subscriptions_come_by_event_name() {
        let sub = |context: &str, wants_envelope| EventSubscription {
            context: context.to_string(),
            service: "S".to_string(),
            wants_envelope,
        };
        let mut plan = EventPlan::default();
        plan.subscriptions
            .insert(("b".into(), "Z".into()), vec![sub("x", false)]);
        plan.subscriptions
            .insert(("a".into(), "Y".into()), vec![sub("x", true)]);
        plan.subscriptions
            .insert(("a".into(), "X".into()), vec![sub("y", false)]);
        let names: Vec<&str> = plan.published_by("a").map(|(e, _)| e).collect();
        assert_eq!(names, ["X", "Y"]);
        assert_eq!(plan.published_by("none").count(), 0);
    }

    #[test]
    fn isolated_delivery_logs_the_failure_instead_of_propagating_it() {
        let stmt = isolated_delivery(call(ident("deliver"), vec![]), str_lit("Svc"));
        let text = bynk_ts::print_stmt(&stmt, 0);
        assert!(text.contains("await deliver()"), "{text}");
        assert!(text.contains("catch (e)"), "{text}");
        assert!(text.contains(DELIVERY_FAILED), "{text}");
        assert!(text.contains("service: \"Svc\""), "{text}");
        assert!(text.contains("error: String(e)"), "{text}");
    }
}
