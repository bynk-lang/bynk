//! A `Producer` refinement must name a context that produces to the queue (#4441).
//!
//! A queue consumer admits Bynk contexts through a refinement of `Producer`
//! (`actor FromOrders = Producer where IsContext(shop.orders)`), and each admitted context signs what
//! it sends under its edge key (#3899). The refinement is a promise that a named context sends to
//! the queue. A context sends through a `Queue @name("…")` instance it declares, so a context that
//! has no such instance for the queue never signs a message for it: the name is dead configuration,
//! or a misspelling of the context that does, and messages from the intended producer are refused
//! only at run time.
//!
//! One report, `bynk.queue.producer_never_sends`, an error, written at the name in `IsContext(…)`:
//! a context the refinement admits that has no producer binding for the queue. A refinement whose
//! predicate names no such context but still admits no context that produces is reported once, at
//! the predicate.
//!
//! What counts as a producer of a queue: a context with a `Queue @name("…")` instance of it, and,
//! for a dead-letter queue, a producer of any queue that dead-letters into it, because a dead letter
//! keeps the signed envelope its source received (`bynk.queue.dead_letter_signing_mismatch`).
//! A name that is no context of the project is `bynk.actor.unknown_context`, reported elsewhere.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{ActorDecl, CommonsItem, ServiceDecl, ServiceProtocol, SourceUnit};
use bynk_syntax::error::CompileError;

use crate::actors::{producer_refinement, queue_admission_for};
use crate::caller_admission::mentioned_names;
use crate::project_model::ErrorSink;
use crate::queue_producers::queue_instances;
use crate::symbols::UnitTable;

/// Report each `Producer` refinement of a signed queue consumer that admits a context that does not
/// produce to the queue.
pub fn phase_producer_admission(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    errors: &mut ErrorSink,
) {
    let contexts: Vec<&str> = groups
        .keys()
        .filter(|n| kinds.get(*n) == Some(&UnitKind::Context))
        .map(String::as_str)
        .collect();
    let direct = direct_producers(&contexts, unit_tables);
    let dead_letters = dead_letter_sources(parsed, groups, kinds);
    for ctx_name in &contexts {
        let Some(table) = unit_tables.get(*ctx_name) else {
            continue;
        };
        for &i in &groups[*ctx_name] {
            let SourceUnit::Context(ctx) = parsed[i].unit() else {
                continue;
            };
            let file = parsed[i].identity_path();
            for item in &ctx.items {
                let CommonsItem::Service(service) = item else {
                    continue;
                };
                let ServiceProtocol::Queue { name: queue } = &service.protocol else {
                    continue;
                };
                let producing = producers_of(queue, &direct, &dead_letters);
                let mut seen = BTreeSet::new();
                for (actor, _) in signed_refinements(service, table) {
                    if !seen.insert(actor.clone()) {
                        continue;
                    }
                    let Some(decl) = find_actor(&actor, parsed, groups, ctx_name) else {
                        continue;
                    };
                    check_refinement(
                        decl.0,
                        queue,
                        &producing,
                        &contexts,
                        &decl
                            .1
                            .map_or_else(|| file.clone(), |f| parsed[f].identity_path()),
                        errors,
                    );
                }
            }
        }
    }
}

/// The `Producer` refinements a queue service's `on message` handlers admit, as (actor, predicate).
fn signed_refinements(
    service: &ServiceDecl,
    table: &UnitTable,
) -> Vec<(String, crate::actors::CallerPredicate)> {
    service
        .handlers
        .iter()
        .filter_map(|h| queue_admission_for(h, &service.protocol, &table.actors))
        .flat_map(|a| a.producers)
        .collect()
}

/// The actor `name` as declared in the context `ctx` and the index of the file declaring it.
fn find_actor<'a>(
    name: &str,
    parsed: &'a [ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    ctx: &str,
) -> Option<(&'a ActorDecl, Option<usize>)> {
    groups.get(ctx)?.iter().find_map(|&i| {
        let SourceUnit::Context(c) = parsed[i].unit() else {
            return None;
        };
        c.items.iter().find_map(|item| match item {
            CommonsItem::Actor(a) if a.name.name == name => Some((a, Some(i))),
            _ => None,
        })
    })
}

/// Each queue name to the contexts that declare a `Queue` instance for it.
fn direct_producers<'a>(
    contexts: &[&'a str],
    tables: &HashMap<String, UnitTable>,
) -> HashMap<String, BTreeSet<&'a str>> {
    let mut out: HashMap<String, BTreeSet<&str>> = HashMap::new();
    for ctx in contexts {
        let Some(table) = tables.get(*ctx) else {
            continue;
        };
        for queue in queue_instances(table).values() {
            out.entry((*queue).to_string()).or_default().insert(ctx);
        }
    }
    out
}

/// Each dead-letter queue to the queues whose consumers dead-letter into it.
fn dead_letter_sources(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for (ctx_name, indices) in groups {
        if kinds.get(ctx_name) != Some(&UnitKind::Context) {
            continue;
        }
        for &i in indices {
            let SourceUnit::Context(ctx) = parsed[i].unit() else {
                continue;
            };
            for item in &ctx.items {
                if let CommonsItem::Service(s) = item
                    && let ServiceProtocol::Queue { name } = &s.protocol
                    && let Some(bynk_syntax::ast::DeadLetter::Queue(dlq)) =
                        s.delivery.as_ref().and_then(|d| d.dead_letter())
                {
                    out.entry(dlq.to_string()).or_default().push(name.clone());
                }
            }
        }
    }
    out
}

/// The contexts that produce to `queue`: those with an instance of it, and the producers of every
/// queue that dead-letters into it (looked at once each, so a cycle ends).
fn producers_of<'a>(
    queue: &str,
    direct: &HashMap<String, BTreeSet<&'a str>>,
    dead_letters: &HashMap<String, Vec<String>>,
) -> BTreeSet<&'a str> {
    let mut out = BTreeSet::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut pending = vec![queue];
    while let Some(q) = pending.pop() {
        if !seen.insert(q) {
            continue;
        }
        if let Some(found) = direct.get(q) {
            out.extend(found.iter().copied());
        }
        if let Some(sources) = dead_letters.get(q) {
            pending.extend(sources.iter().map(String::as_str));
        }
    }
    out
}

fn check_refinement(
    actor: &ActorDecl,
    queue: &str,
    producing: &BTreeSet<&str>,
    contexts: &[&str],
    file: &Path,
    errors: &mut ErrorSink,
) {
    let (Some(refinement), Some(predicate)) = (
        actor.refinement.as_ref(),
        producer_refinement(actor, &HashMap::new()),
    ) else {
        return;
    };
    let mut names = Vec::new();
    mentioned_names(&refinement.predicate, &mut names);
    // A name that is no context is `bynk.actor.unknown_context`, which already says what is wrong
    // with the predicate: it is not reported a second time as admitting nothing.
    let mut reported = names.iter().any(|(n, _)| !contexts.contains(&n.as_str()));
    for (name, span) in names {
        // A name that is no context is `bynk.actor.unknown_context`; a name the predicate does not
        // admit (under a `!`) is not a context it promises to hear from.
        if !contexts.contains(&name.as_str())
            || !predicate.admits(&name)
            || producing.contains(name.as_str())
        {
            continue;
        }
        reported = true;
        errors.push_for(
            Some(file),
            never_sends(&actor.name.name, &name, queue, span, producing),
        );
    }
    if !reported
        && !contexts
            .iter()
            .any(|c| predicate.admits(c) && producing.contains(c))
    {
        errors.push_for(
            Some(file),
            CompileError::new(
                "bynk.queue.producer_never_sends",
                refinement.predicate.span,
                format!(
                    "`{}` admits no context that produces to queue `{queue}`",
                    actor.name.name
                ),
            )
            .with_note(producers_note(queue, producing)),
        );
    }
}

fn never_sends(
    actor: &str,
    context: &str,
    queue: &str,
    span: bynk_syntax::span::Span,
    producing: &BTreeSet<&str>,
) -> CompileError {
    let mut err = CompileError::new(
        "bynk.queue.producer_never_sends",
        span,
        format!(
            "`{actor}` admits `{context}`, but `{context}` has no producer for queue `{queue}`"
        ),
    )
    .with_note(producers_note(queue, producing));
    if let Some(near) = bynk_project::paths::did_you_mean(context, producing.iter().copied()) {
        err = err.with_note(format!("did you mean `{near}`?"));
    }
    err
}

/// How a context becomes a producer, and which ones are.
fn producers_note(queue: &str, producing: &BTreeSet<&str>) -> String {
    let who = if producing.is_empty() {
        "no context of this project produces to it".to_string()
    } else {
        format!(
            "the contexts that do: {}",
            producing
                .iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        "a context produces to a queue through a `Queue @name(\"{queue}\")` instance it declares, which is what signs its messages; {who}"
    )
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    const CONSUMER: &str = "context shop.fulfil\n\ntype Order = { id: String }\n\nactor FromOrders = Producer where PRED\n\nservice fulfil from queue(\"order-events\") {\n  delivery { deadLetter: None }\n  on message(order: Order) -> Effect[QueueResult] by FromOrders { Ack }\n}\n";
    const PRODUCER: &str = "context shop.orders\n\nconsumes bynk.cloudflare { Queue }\n\ncapability OrderEvents = Queue @name(\"QUEUE\")\n\ntype Order = { id: String }\n\nservice place {\n  on call(o: Order) -> Effect[()] by Caller given OrderEvents {\n    OrderEvents.sendValue(o)\n  }\n}\n";
    const QUIET: &str = "context shop.audit\n\nservice note {\n  on call(n: Int) -> Effect[Int] by Caller { Effect.pure(n) }\n}\n";

    fn categories(files: &[(&str, String)]) -> Vec<&'static str> {
        let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, s)| (*p, s.as_str())).collect();
        analyse(&borrowed).categories()
    }

    fn consumer(pred: &str) -> (&'static str, String) {
        ("shop/fulfil.bynk", CONSUMER.replace("PRED", pred))
    }

    #[test]
    fn a_producing_context_is_admitted() {
        let cats = categories(&[
            consumer("IsContext(shop.orders)"),
            (
                "shop/orders.bynk",
                PRODUCER.replace("QUEUE", "order-events"),
            ),
        ]);
        assert!(
            !cats.contains(&"bynk.queue.producer_never_sends"),
            "{cats:?}"
        );
    }

    #[test]
    fn a_context_with_no_queue_is_refused_and_so_is_one_producing_another_queue() {
        for orders in [
            QUIET.replace("shop.audit", "shop.orders"),
            PRODUCER.replace("QUEUE", "audit-events"),
        ] {
            let cats = categories(&[
                consumer("IsContext(shop.orders)"),
                ("shop/orders.bynk", orders),
            ]);
            assert!(
                cats.contains(&"bynk.queue.producer_never_sends"),
                "{cats:?}"
            );
        }
    }

    #[test]
    fn a_name_that_is_no_context_is_left_to_unknown_context() {
        let cats = categories(&[consumer("IsContext(shop.nowhere)")]);
        assert!(cats.contains(&"bynk.actor.unknown_context"), "{cats:?}");
        assert!(
            !cats.contains(&"bynk.queue.producer_never_sends"),
            "{cats:?}"
        );
    }

    #[test]
    fn only_the_name_that_does_not_produce_is_refused() {
        let cats = categories(&[
            consumer("IsContext(shop.orders) || IsContext(shop.audit)"),
            (
                "shop/orders.bynk",
                PRODUCER.replace("QUEUE", "order-events"),
            ),
            ("shop/audit.bynk", QUIET.to_string()),
        ]);
        let n = cats
            .iter()
            .filter(|c| **c == "bynk.queue.producer_never_sends")
            .count();
        assert_eq!(n, 1, "{cats:?}");
    }

    #[test]
    fn a_negated_name_is_not_asked_to_produce() {
        // Admits every context but `shop.audit`, and `shop.orders` produces.
        let cats = categories(&[
            consumer("!IsContext(shop.audit)"),
            (
                "shop/orders.bynk",
                PRODUCER.replace("QUEUE", "order-events"),
            ),
            ("shop/audit.bynk", QUIET.to_string()),
        ]);
        assert!(
            !cats.contains(&"bynk.queue.producer_never_sends"),
            "{cats:?}"
        );
    }

    #[test]
    fn a_predicate_that_admits_no_producing_context_is_refused_once() {
        // Only `shop.audit` is left once `shop.orders` is excluded, and it produces nothing.
        let cats = categories(&[
            consumer("!IsContext(shop.orders)"),
            (
                "shop/orders.bynk",
                PRODUCER.replace("QUEUE", "order-events"),
            ),
            ("shop/audit.bynk", QUIET.to_string()),
        ]);
        let n = cats
            .iter()
            .filter(|c| **c == "bynk.queue.producer_never_sends")
            .count();
        assert!(n >= 1, "{cats:?}");
    }

    #[test]
    fn a_dead_letter_consumer_names_the_producers_of_its_source() {
        let source = "context shop.fulfil\n\ntype Order = { id: String }\n\nactor FromOrders = Producer where IsContext(shop.orders)\n\nservice fulfil from queue(\"order-events\") {\n  delivery { deadLetter: \"order-dead\" }\n  on message(order: Order) -> Effect[QueueResult] by FromOrders { Ack }\n}\n\nservice review from queue(\"order-dead\") {\n  delivery { deadLetter: None }\n  on message(order: Order) -> Effect[QueueResult] by FromOrders { Ack }\n}\n";
        let cats = categories(&[
            ("shop/fulfil.bynk", source.to_string()),
            (
                "shop/orders.bynk",
                PRODUCER.replace("QUEUE", "order-events"),
            ),
        ]);
        assert!(
            !cats.contains(&"bynk.queue.producer_never_sends"),
            "{cats:?}"
        );
    }
}
