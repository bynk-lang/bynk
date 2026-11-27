//! A queue consumer's `delivery { }` policy (#3877).
//!
//! The section sets the consumer's retries, retry delay, dead-letter queue,
//! batching and concurrency, which the emitter writes into
//! `[[queues.consumers]]`. This module checks one service's section: that it
//! sits on a `from queue` service, that its keys are the closed set and each is
//! written once, and that each value has its key's type and range. It also
//! warns of a consumer with no dead-letter queue, because Cloudflare deletes a
//! message that reaches the retry limit unless one is set.
//!
//! The project-wide half (a dead-letter queue's own consumer must take the same
//! message type) is [`phase_dead_letter_consumers`].

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{
    ActorDecl, CommonsItem, DeliveryPolicy, ExprKind, ServiceDecl, ServiceProtocol, SourceUnit,
};
use bynk_syntax::error::CompileError;

use crate::context_checks::ts_type_ref_display;
use crate::contract_compat::Verdict;
use crate::project_model::{ErrorSink, is_queue_name};
use crate::queue_producers::{Project, consumers_of, message_compat};

/// The keys of a `delivery { }` section, with what each takes.
const KEYS: &[(&str, &str)] = &[
    ("retries", "an `Int` from 0 to 100"),
    (
        "retryDelay",
        "a `Duration` of whole seconds, at most 24 hours",
    ),
    ("deadLetter", "a queue name, or `None`"),
    ("batchSize", "an `Int` from 1 to 100"),
    ("batchWait", "a `Duration` of whole seconds from 0 to 60"),
    ("concurrency", "an `Int` from 1 to 250"),
];

fn bad_option(span: bynk_syntax::span::Span, message: String, note: &str) -> CompileError {
    CompileError::new("bynk.queue.delivery_bad_option", span, message).with_note(note.to_string())
}

/// Check one service's `delivery { }` section, and warn of a queue consumer that
/// has no dead-letter queue.
pub fn validate_delivery(service: &ServiceDecl, errors: &mut Vec<CompileError>) {
    let ServiceProtocol::Queue { name } = &service.protocol else {
        if let Some(policy) = &service.delivery {
            errors.push(
                CompileError::new(
                    "bynk.queue.delivery_not_queue",
                    policy.span,
                    "a `delivery { }` policy is only valid on a `from queue` service",
                )
                .with_note(
                    "retries, retry delay, the dead-letter queue and batching govern a queue consumer, which only a `from queue` service is",
                ),
            );
        }
        return;
    };
    let Some(policy) = &service.delivery else {
        warn_no_dead_letter(service, errors);
        return;
    };
    check_keys(policy, errors);
    check_values(policy, name, errors);
    if policy.field("deadLetter").is_none() {
        warn_no_dead_letter(service, errors);
    }
}

fn warn_no_dead_letter(service: &ServiceDecl, errors: &mut Vec<CompileError>) {
    errors.push(
        CompileError::new(
            "bynk.queue.no_dead_letter",
            service.name.span,
            format!(
                "queue consumer `{}` has no dead-letter queue, so a message that reaches the retry limit is deleted",
                service.name.name
            ),
        )
        .with_note(
            "add `delivery { deadLetter: \"<queue>\" }` to keep them, or `deadLetter: None` to say deleting them is intended",
        ),
    );
}

fn check_keys(policy: &DeliveryPolicy, errors: &mut Vec<CompileError>) {
    let mut seen: Vec<&str> = Vec::new();
    for field in &policy.fields {
        let key = field.name.name.as_str();
        if !KEYS.iter().any(|(k, _)| *k == key) {
            errors.push(bad_option(
                field.name.span,
                format!("unknown `delivery` key `{key}`"),
                "the keys are `retries`, `retryDelay`, `deadLetter`, `batchSize`, `batchWait` and `concurrency`",
            ));
        } else if seen.contains(&key) {
            errors.push(bad_option(
                field.name.span,
                format!("`delivery` key `{key}` is written twice"),
                "each key is set once",
            ));
        } else {
            seen.push(key);
        }
    }
}

fn check_values(policy: &DeliveryPolicy, queue: &str, errors: &mut Vec<CompileError>) {
    for field in &policy.fields {
        let key = field.name.name.as_str();
        let Some((_, takes)) = KEYS.iter().find(|(k, _)| *k == key) else {
            continue;
        };
        let expr = &field.value;
        let ok = match key {
            "retries" => int_in(expr, 0, 100),
            "batchSize" => int_in(expr, 1, 100),
            "concurrency" => int_in(expr, 1, 250),
            "retryDelay" => secs_in(expr, 0, 86_400),
            "batchWait" => secs_in(expr, 0, 60),
            _ => {
                check_dead_letter(expr, queue, errors);
                continue;
            }
        };
        if !ok {
            errors.push(bad_option(
                expr.span,
                format!("`delivery` `{key}` must be {takes}"),
                "a literal, or a constant of that type, is accepted",
            ));
        }
    }
}

fn int_in(expr: &bynk_syntax::ast::Expr, lo: i64, hi: i64) -> bool {
    matches!(&expr.kind, ExprKind::IntLit { value, .. } if (lo..=hi).contains(value))
}

fn secs_in(expr: &bynk_syntax::ast::Expr, lo: i64, hi: i64) -> bool {
    matches!(&expr.kind, ExprKind::DurationLit { millis, .. }
        if millis % 1_000 == 0 && (lo..=hi).contains(&(millis / 1_000)))
}

fn check_dead_letter(expr: &bynk_syntax::ast::Expr, queue: &str, errors: &mut Vec<CompileError>) {
    match &expr.kind {
        ExprKind::None => {}
        ExprKind::StrLit(name) if !is_queue_name(name) => errors.push(
            CompileError::new(
                "bynk.queue.invalid_name",
                expr.span,
                format!("`{name}` is not a queue name Cloudflare accepts"),
            )
            .with_note(
                "a queue name is 1 to 63 lowercase letters, digits or `-`, and starts and ends with a letter or digit",
            ),
        ),
        ExprKind::StrLit(name)
            if let Some(error) = crate::queue_reserved::reserved_name_error(name, expr.span) =>
        {
            errors.push(error);
        }
        ExprKind::StrLit(name) if name == queue => errors.push(bad_option(
            expr.span,
            format!("the dead-letter queue is the consumer's own queue `{queue}`"),
            "a message that reached the retry limit would be redelivered to the consumer that failed it; name another queue",
        )),
        ExprKind::StrLit(_) => {}
        _ => errors.push(bad_option(
            expr.span,
            "`delivery` `deadLetter` must be a queue name, or `None`".to_string(),
            "a string literal, or a `String` constant, names the queue; `None` says deleting is intended",
        )),
    }
}

/// A dead-letter queue is an ordinary queue, so a Bynk consumer may read it
/// (`from queue("outbound-email-dlq")`): it must be able to decode every
/// message of the source consumer's type, since the dead letter is the same
/// body, unchanged. That is the comparison a producer's send gets (#3879,
/// ADR 0714), so an alias of the same type and a weaker refinement are
/// accepted, and a different type is refused (#4305).
pub fn phase_dead_letter_consumers(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    project: &Project<'_>,
    errors: &mut ErrorSink,
) {
    let consumers = consumers_of(parsed, groups, kinds);
    for (ctx_name, indices) in groups {
        if kinds.get(ctx_name) != Some(&UnitKind::Context) {
            continue;
        }
        for &i in indices {
            let SourceUnit::Context(ctx) = parsed[i].unit() else {
                continue;
            };
            for item in &ctx.items {
                let CommonsItem::Service(service) = item else {
                    continue;
                };
                let ServiceProtocol::Queue { name: queue } = &service.protocol else {
                    continue;
                };
                let Some(delivery) = &service.delivery else {
                    continue;
                };
                let Some(bynk_syntax::ast::DeadLetter::Queue(dlq)) = delivery.dead_letter() else {
                    continue;
                };
                let (Some(source), Some(dead)) =
                    (consumers.get(queue.as_str()), consumers.get(dlq))
                else {
                    continue;
                };
                let verdict = message_compat(
                    &source.param.type_ref,
                    &project.types_of(source.context),
                    &dead.param.type_ref,
                    &project.types_of(dead.context),
                );
                if verdict.verdict != Verdict::Breaking {
                    continue;
                }
                let span = delivery
                    .fields
                    .iter()
                    .rev()
                    .find(|f| f.name.name == "deadLetter")
                    .map_or(delivery.span, |f| f.value.span);
                let mut err = CompileError::new(
                    "bynk.queue.dead_letter_type_mismatch",
                    span,
                    format!(
                        "the consumer of dead-letter queue `{dlq}` takes `{}`, but `{queue}` (context `{ctx_name}`) takes `{}`",
                        ts_type_ref_display(&dead.param.type_ref),
                        ts_type_ref_display(&source.param.type_ref),
                    ),
                )
                .with_label(
                    dead.param.span,
                    format!("consumed here, in context `{}`", dead.context),
                );
                for f in verdict.findings.iter().filter(|f| f.breaking) {
                    err = err.with_note(format!("{}: {}", f.path, f.message));
                }
                errors.push_for(
                    Some(&parsed[i].identity_path()),
                    err.with_note(
                        "a dead letter is the original message body, unchanged, so its consumer must accept the source consumer's type",
                    ),
                );
            }
        }
    }
}

/// #3899: the senders a queue service's `on message` admits, as (actor, secret), or `None` when it
/// admits unsigned messages (`by Producer`). A `Producer` refinement has no secret of its own (its
/// senders sign under their edge keys), so it is listed by its actor's name alone.
fn signed_senders(
    service: &ServiceDecl,
    actors: &HashMap<String, ActorDecl>,
) -> Option<BTreeSet<(String, String)>> {
    service
        .handlers
        .iter()
        .find_map(|h| crate::actors::queue_admission_for(h, &service.protocol, actors))
        .map(|a| {
            a.foreign
                .into_iter()
                .map(|f| (f.actor, f.secret))
                .chain(
                    a.producers
                        .into_iter()
                        .map(|(actor, _)| (actor, String::new())),
                )
                .collect()
        })
}

/// #3899: a dead letter keeps whatever the source consumer received, so a signed queue's dead
/// letters are the signed envelope and an unsigned queue's are the bare body. The dead-letter
/// queue's consumer must therefore be signed exactly when the source is, in the same context
/// (it verifies under the queues that dead-letter into its own context's), and a signed one must
/// admit the source's senders, or every dead letter would fail to decode or verify and be dropped.
/// A dead-letter queue nobody in the project consumes is read elsewhere, and is not checked.
pub fn phase_dead_letter_signing(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    errors: &mut ErrorSink,
) {
    // Each queue's consumer: its context, its service and the senders it admits (`None`: unsigned).
    type Info<'a> = (&'a str, &'a ServiceDecl, Option<BTreeSet<(String, String)>>);
    let mut consumers: HashMap<&str, Info> = HashMap::new();
    for (ctx_name, indices) in groups {
        if kinds.get(ctx_name) != Some(&UnitKind::Context) {
            continue;
        }
        let mut actors: HashMap<String, ActorDecl> = HashMap::new();
        for &i in indices {
            if let SourceUnit::Context(ctx) = parsed[i].unit() {
                for item in &ctx.items {
                    if let CommonsItem::Actor(a) = item {
                        actors.insert(a.name.name.clone(), a.clone());
                    }
                }
            }
        }
        for &i in indices {
            let SourceUnit::Context(ctx) = parsed[i].unit() else {
                continue;
            };
            for item in &ctx.items {
                if let CommonsItem::Service(service) = item
                    && let ServiceProtocol::Queue { name } = &service.protocol
                {
                    consumers.entry(name.as_str()).or_insert((
                        ctx_name,
                        service,
                        signed_senders(service, &actors),
                    ));
                }
            }
        }
    }
    for (queue, (ctx_name, service, source)) in &consumers {
        let Some(delivery) = &service.delivery else {
            continue;
        };
        let Some(bynk_syntax::ast::DeadLetter::Queue(dlq)) = delivery.dead_letter() else {
            continue;
        };
        let Some((dead_ctx, _, dead)) = consumers.get(dlq) else {
            continue;
        };
        let problem = match (source, dead) {
            (None, None) => continue,
            (Some(_), None) => format!(
                "the consumer of dead-letter queue `{dlq}` admits unsigned messages, but `{queue}` admits only signed ones"
            ),
            (None, Some(_)) => format!(
                "the consumer of dead-letter queue `{dlq}` admits only signed messages, but `{queue}` is not signed"
            ),
            (Some(s), Some(d)) if dead_ctx != ctx_name => {
                let _ = (s, d);
                format!(
                    "the consumer of dead-letter queue `{dlq}` is in context `{dead_ctx}`, but `{queue}` is signed in context `{ctx_name}`"
                )
            }
            (Some(s), Some(d)) if !s.is_subset(d) => format!(
                "the consumer of dead-letter queue `{dlq}` does not admit every sender `{queue}` does"
            ),
            (Some(_), Some(_)) => continue,
        };
        let span = delivery
            .fields
            .iter()
            .rev()
            .find(|f| f.name.name == "deadLetter")
            .map_or(delivery.span, |f| f.value.span);
        let file = groups
            .get(*ctx_name)
            .into_iter()
            .flatten()
            .find(|&&i| match parsed[i].unit() {
                SourceUnit::Context(c) => c
                    .items
                    .iter()
                    .any(|it| matches!(it, CommonsItem::Service(sv) if std::ptr::eq(sv, *service))),
                _ => false,
            })
            .copied();
        let err = CompileError::new("bynk.queue.dead_letter_signing_mismatch", span, problem)
            .with_note(
                "a dead letter keeps what the source consumer received, the signed envelope for a signed queue, so its consumer must verify the same senders in the same context (or neither queue is signed)",
            );
        errors.push_for(file.map(|i| parsed[i].identity_path()).as_deref(), err);
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    /// A one-service context holding `body` after a queue header, with a handler.
    fn consumer(queue: &str, body: &str) -> String {
        format!(
            "context t.q\n\nservice w from queue(\"{queue}\") {{\n{body}\n  on message(m: String) -> Effect[QueueResult] by Producer {{ Ack }}\n}}\n"
        )
    }

    fn categories(source: &str) -> Vec<&'static str> {
        analyse(&[("t/q.bynk", source)]).categories()
    }

    #[test]
    fn a_complete_policy_with_a_dead_letter_queue_is_clean() {
        let a = analyse(&[(
            "t/q.bynk",
            &consumer(
                "jobs",
                "  delivery { retries: 5, retryDelay: 30.seconds, deadLetter: \"jobs-dlq\", batchSize: 25, batchWait: 10.seconds, concurrency: 4 }",
            ),
        )]);
        a.assert_clean();
    }

    #[test]
    fn a_consumer_with_no_dead_letter_queue_warns_and_none_silences_it() {
        assert_eq!(
            categories(&consumer("jobs", "")),
            ["bynk.queue.no_dead_letter"]
        );
        assert_eq!(
            categories(&consumer("jobs", "  delivery { retries: 1 }")),
            ["bynk.queue.no_dead_letter"]
        );
        assert!(categories(&consumer("jobs", "  delivery { deadLetter: None }")).is_empty());
    }

    #[test]
    fn each_out_of_range_value_is_refused() {
        for bad in [
            "retries: 101",
            "retries: \"3\"",
            "retryDelay: 25.hours",
            "retryDelay: 1500.milliseconds",
            "retryDelay: 5",
            "batchSize: 0",
            "batchSize: 101",
            "batchWait: 61.seconds",
            "concurrency: 0",
            "concurrency: 251",
            "deadLetter: 3",
        ] {
            let rest = if bad.starts_with("deadLetter") {
                ""
            } else {
                ", deadLetter: None"
            };
            let source = consumer("jobs", &format!("  delivery {{ {bad}{rest} }}"));
            assert_eq!(
                categories(&source),
                ["bynk.queue.delivery_bad_option"],
                "{bad}"
            );
        }
    }

    #[test]
    fn an_unknown_or_repeated_key_is_refused() {
        for body in [
            "  delivery { retry: 1, deadLetter: None }",
            "  delivery { retries: 1, retries: 2, deadLetter: None }",
        ] {
            assert_eq!(
                categories(&consumer("jobs", body)),
                ["bynk.queue.delivery_bad_option"],
                "{body}"
            );
        }
    }

    #[test]
    fn a_dead_letter_queue_must_be_another_valid_queue() {
        assert_eq!(
            categories(&consumer("jobs", "  delivery { deadLetter: \"jobs\" }")),
            ["bynk.queue.delivery_bad_option"]
        );
        assert_eq!(
            categories(&consumer("jobs", "  delivery { deadLetter: \"Jobs_DLQ\" }")),
            ["bynk.queue.invalid_name"]
        );
    }

    #[test]
    fn a_constant_stands_for_a_literal_of_its_type() {
        let source = "context t.q\n\nconst Backoff: Duration = 30.seconds\nconst Limit: Int = 7\nconst Dlq: String = \"jobs-dlq\"\n\nservice w from queue(\"jobs\") {\n  delivery { retries: Limit, retryDelay: Backoff, deadLetter: Dlq }\n  on message(m: String) -> Effect[QueueResult] by Producer { Ack }\n}\n";
        analyse(&[("t/q.bynk", source)]).assert_clean();
        // A constant of the wrong type is left as written, and refused.
        let wrong = source.replace("const Limit: Int = 7", "const Limit: String = \"7\"");
        assert_eq!(categories(&wrong), ["bynk.queue.delivery_bad_option"]);
    }

    #[test]
    fn the_section_is_only_valid_on_a_queue_service() {
        let source = "context t.q\n\nservice tick from cron {\n  delivery { retries: 1 }\n  on schedule(\"0 * * * *\") () -> Effect[Result[(), String]] { Ok(()) }\n}\n";
        assert_eq!(categories(source), ["bynk.queue.delivery_not_queue"]);
    }

    #[test]
    fn a_dead_letter_queues_consumer_must_take_the_same_message() {
        let source = |dlq_ty: &str| {
            format!(
                "context t.q\n\nservice w from queue(\"jobs\") {{\n  delivery {{ deadLetter: \"jobs-dlq\" }}\n  on message(m: String) -> Effect[QueueResult] by Producer {{ Ack }}\n}}\n\nservice review from queue(\"jobs-dlq\") {{\n  delivery {{ deadLetter: None }}\n  on message(m: {dlq_ty}) -> Effect[QueueResult] by Producer {{ Ack }}\n}}\n"
            )
        };
        analyse(&[("t/q.bynk", &source("String"))]).assert_clean();
        assert_eq!(
            categories(&source("Int")),
            ["bynk.queue.dead_letter_type_mismatch"]
        );
    }

    /// #4305: the two consumers' types are each read in their own context, so a record with the
    /// same fields under the same name in two contexts is the same message, and different fields
    /// are not.
    #[test]
    fn dead_letter_consumers_in_two_contexts_are_compared_in_their_own_namespaces() {
        let source = "context t.a\n\ntype Job = { id: String }\n\nservice w from queue(\"jobs\") {\n  delivery { deadLetter: \"jobs-dlq\" }\n  on message(m: Job) -> Effect[QueueResult] by Producer { Ack }\n}\n";
        let review = |fields: &str| {
            format!(
                "context t.b\n\ntype Job = {{ {fields} }}\n\nservice review from queue(\"jobs-dlq\") {{\n  delivery {{ deadLetter: None }}\n  on message(m: Job) -> Effect[QueueResult] by Producer {{ Ack }}\n}}\n"
            )
        };
        analyse(&[("t/a.bynk", source), ("t/b.bynk", &review("id: String"))]).assert_clean();
        assert_eq!(
            analyse(&[("t/a.bynk", source), ("t/b.bynk", &review("name: String"))]).categories(),
            ["bynk.queue.dead_letter_type_mismatch"]
        );
    }

    /// #4305: the dead letter is the source's body, so a dead-letter consumer of a weaker
    /// refinement accepts it and one of a stronger refinement does not.
    #[test]
    fn a_dead_letter_consumer_may_take_a_weaker_refinement_but_not_a_stronger() {
        let pair = |source_ty: &str, dlq_ty: &str| {
            format!(
                "context t.q\n\ntype Small = Int where InRange(1, 10)\ntype Wide = Int where InRange(1, 100)\n\nservice w from queue(\"jobs\") {{\n  delivery {{ deadLetter: \"jobs-dlq\" }}\n  on message(m: {source_ty}) -> Effect[QueueResult] by Producer {{ Ack }}\n}}\n\nservice review from queue(\"jobs-dlq\") {{\n  delivery {{ deadLetter: None }}\n  on message(m: {dlq_ty}) -> Effect[QueueResult] by Producer {{ Ack }}\n}}\n"
            )
        };
        analyse(&[("t/q.bynk", &pair("Small", "Wide"))]).assert_clean();
        assert_eq!(
            categories(&pair("Wide", "Small")),
            ["bynk.queue.dead_letter_type_mismatch"]
        );
    }

    #[test]
    fn a_second_section_is_a_parse_error() {
        let a = analyse(&[(
            "t/q.bynk",
            &consumer(
                "jobs",
                "  delivery { retries: 1 }\n  delivery { deadLetter: None }",
            ),
        )]);
        a.assert_reports("bynk.parse.duplicate_delivery");
    }

    /// #3899: a signed queue and its dead-letter queue, with the dead-letter consumer's `by`.
    fn signed_pair(dead_by: &str) -> String {
        format!(
            "context t.q\n\nactor Legacy {{ auth = Foreign(secret = \"LEGACY_QUEUE_KEY\") }}\nactor Other {{ auth = Foreign(secret = \"OTHER_QUEUE_KEY\") }}\n\n\
             service w from queue(\"jobs\") {{\n  delivery {{ deadLetter: \"jobs-dead\" }}\n  on message(m: String) -> Effect[QueueResult] by Legacy {{ Ack }}\n}}\n\n\
             service d from queue(\"jobs-dead\") {{\n  delivery {{ deadLetter: None }}\n  on message(m: String) -> Effect[QueueResult] by {dead_by} {{ Ack }}\n}}\n"
        )
    }

    #[test]
    fn a_signed_queues_dead_letter_consumer_must_admit_its_senders() {
        // Signed by the same actor: clean (the one warning is the dead-letter queue's own missing one).
        assert!(
            !categories(&signed_pair("Legacy"))
                .contains(&"bynk.queue.dead_letter_signing_mismatch")
        );
        // Unsigned, or signed by another actor: every dead letter would be dropped.
        for by in ["Producer", "Other"] {
            assert!(
                categories(&signed_pair(by)).contains(&"bynk.queue.dead_letter_signing_mismatch"),
                "{by}"
            );
        }
    }

    #[test]
    fn an_unsigned_queues_signed_dead_letter_consumer_is_refused() {
        let source = "context t.q\n\nactor Legacy { auth = Foreign(secret = \"LEGACY_QUEUE_KEY\") }\n\nservice w from queue(\"jobs\") {\n  delivery { deadLetter: \"jobs-dead\" }\n  on message(m: String) -> Effect[QueueResult] by Producer { Ack }\n}\n\nservice d from queue(\"jobs-dead\") {\n  delivery { deadLetter: None }\n  on message(m: String) -> Effect[QueueResult] by Legacy { Ack }\n}\n";
        assert!(categories(source).contains(&"bynk.queue.dead_letter_signing_mismatch"));
    }
}
