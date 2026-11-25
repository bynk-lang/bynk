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

use std::collections::{BTreeMap, HashMap};

use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{
    CommonsItem, DeliveryPolicy, ExprKind, Handler, HandlerKind, ServiceDecl, ServiceProtocol,
    SourceUnit,
};
use bynk_syntax::error::CompileError;

use crate::context_checks::ts_type_ref_display;
use crate::project_model::{ErrorSink, is_queue_name};

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

/// One consumer: its context's name, its service, and its message type.
struct Consumer<'a> {
    context: &'a str,
    handler: &'a Handler,
    file: usize,
    ty: String,
}

/// The message parameter's type of a consumer's `on message` handler, as written.
fn message_type(handler: &Handler) -> Option<String> {
    // The message is the parameter that is not the `QueueDelivery` (#3880), so
    // a handler that takes the delivery first still has its message type read.
    handler
        .params
        .iter()
        .find(|p| !bynk_syntax::queue::is_queue_delivery(&p.type_ref))
        .map(|p| ts_type_ref_display(&p.type_ref))
}

/// A dead-letter queue is an ordinary queue, so a Bynk consumer may read it
/// (`from queue("outbound-email-dlq")`): its message type must be the source
/// consumer's, since the dead letter is the same body, unchanged.
pub fn phase_dead_letter_consumers(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    errors: &mut ErrorSink,
) {
    let mut consumers: HashMap<&str, Consumer> = HashMap::new();
    let mut sources: Vec<(&str, &ServiceDecl, usize, &str)> = Vec::new();
    for (ctx_name, indices) in groups {
        if kinds.get(ctx_name) != Some(&UnitKind::Context) {
            continue;
        }
        for &i in indices {
            let SourceUnit::Context(ctx) = parsed[i].unit() else {
                continue;
            };
            for item in &ctx.items {
                let CommonsItem::Service(s) = item else {
                    continue;
                };
                let ServiceProtocol::Queue { name } = &s.protocol else {
                    continue;
                };
                let Some(handler) = s
                    .handlers
                    .iter()
                    .find(|h| matches!(h.kind, HandlerKind::Message))
                else {
                    continue;
                };
                let Some(ty) = message_type(handler) else {
                    continue;
                };
                consumers.entry(name.as_str()).or_insert(Consumer {
                    context: ctx_name,
                    handler,
                    file: i,
                    ty,
                });
                sources.push((name.as_str(), s, i, ctx_name));
            }
        }
    }
    for (queue, service, file, ctx_name) in sources {
        let Some(delivery) = &service.delivery else {
            continue;
        };
        let Some(bynk_syntax::ast::DeadLetter::Queue(dlq)) = delivery.dead_letter() else {
            continue;
        };
        let (Some(source), Some(dead)) = (consumers.get(queue), consumers.get(dlq)) else {
            continue;
        };
        if source.ty == dead.ty {
            continue;
        }
        let span = delivery
            .fields
            .iter()
            .rev()
            .find(|f| f.name.name == "deadLetter")
            .map_or(delivery.span, |f| f.value.span);
        errors.push_for(
            Some(&parsed[file].identity_path()),
            CompileError::new(
                "bynk.queue.dead_letter_type_mismatch",
                span,
                format!(
                    "the consumer of dead-letter queue `{dlq}` takes `{}`, but `{queue}` (context `{ctx_name}`) takes `{}`",
                    dead.ty, source.ty
                ),
            )
            .with_label(
                dead.handler.span,
                format!("consumed here, in context `{}`", dead.context),
            )
            .with_note(
                "a dead letter is the original message body, unchanged, so its consumer must take the same type",
            ),
        );
        let _ = dead.file;
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
}
