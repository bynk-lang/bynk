//! The editor surfaces of a queue consumer's `delivery { }` section (#3877) and
//! its delivery metadata (#3880):
//! the keys completion offers, the hover on the section keyword and on each key,
//! and the section keyword's semantic token. All lexical, like the other service
//! policies, since the section is a contextual keyword.

use std::collections::HashMap;
use std::path::PathBuf;

use bynk_syntax::ast::{CommonsItem, HandlerKind, ServiceProtocol, SourceUnit};
use bynk_syntax::lexer::{TokenKind, tokenize};
use bynk_syntax::span::Span;

use crate::completion::{
    for_each_unit, innermost_open_brace, innermost_open_paren, word_before_brace,
};
use crate::symbols::type_ref_str;

/// The keys of a `delivery { }` section, in the order the book gives them: the
/// name, its hover text (meaning, range and Cloudflare's default) and the value
/// completion inserts.
pub const DELIVERY_FIELDS: &[(&str, &str)] = &[
    (
        "retries",
        "how many times a failed message is redelivered before it is dead-lettered or deleted: an `Int` from 0 to 100. Cloudflare's default is 3 (`max_retries`).",
    ),
    (
        "retryDelay",
        "how long to wait before a retried message is redelivered: a `Duration` of whole seconds, at most 24 hours. Cloudflare's default is no delay (`retry_delay`).",
    ),
    (
        "deadLetter",
        "the queue a message is written to when it reaches the retry limit, or `None` to say it may be deleted. Without one Cloudflare deletes it (`dead_letter_queue`). The queue is created if it is missing, and a Bynk consumer of it takes the same message type.",
    ),
    (
        "batchSize",
        "the most messages delivered to the consumer at once: an `Int` from 1 to 100. Bynk's default is 10 (`max_batch_size`).",
    ),
    (
        "batchWait",
        "the longest to wait for a batch to fill: a `Duration` of whole seconds from 0 to 60. Cloudflare's default is 5 seconds (`max_batch_timeout`).",
    ),
    (
        "concurrency",
        "the most consumer invocations running at once: an `Int` from 1 to 250. Unset, Cloudflare scales to the maximum, which it recommends (`max_concurrency`).",
    ),
];

/// What the `delivery` keyword hovers as.
const SECTION_DOC: &str = "```bynk\ndelivery { retries, retryDelay, deadLetter, batchSize, batchWait, concurrency }\n```\n\nThe consumer's delivery policy, written into its `[[queues.consumers]]` in `wrangler.toml`. Every key is optional and an omitted key keeps Cloudflare's default. Without a `deadLetter` (or `deadLetter: None`) a message that reaches the retry limit is deleted.";

/// The cursor is at a key-*name* position inside a `delivery { … }` block, as
/// [`crate::completion::in_limits_field_position`] is for `limits`.
pub fn in_delivery_field_position(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_brace(text, offset) else {
        return false;
    };
    if word_before_brace(text, open) != "delivery" {
        return false;
    }
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return false;
    };
    let current = seg.rsplit(['\n', ',']).next().unwrap_or("");
    !current.contains(':')
}

/// The keys the `delivery { … }` block around `offset` has not written yet.
pub fn unwritten_keys(text: &str, offset: usize) -> Vec<(&'static str, &'static str)> {
    let Some(open) = innermost_open_brace(text, offset) else {
        return DELIVERY_FIELDS.to_vec();
    };
    let close = text[open..]
        .find('}')
        .map_or(text.len(), |i| open + i)
        .max(offset.min(text.len()));
    let block = text.get(open + 1..close).unwrap_or("");
    DELIVERY_FIELDS
        .iter()
        .copied()
        .filter(|(name, _)| {
            !block
                .split([',', '\n'])
                .any(|part| part.trim_start().starts_with(&format!("{name}:")))
        })
        .collect()
}

/// Whether the token at `idx` is the section keyword: `delivery {` right after
/// the service's `{` or a previous section's `}`.
fn is_section_keyword(source: &str, tokens: &[bynk_syntax::lexer::Token], idx: usize) -> bool {
    let word = |i: usize| source.get(tokens[i].span.start..tokens[i].span.end);
    tokens[idx].kind == TokenKind::Ident
        && word(idx) == Some("delivery")
        && tokens.get(idx + 1).map(|t| t.kind) == Some(TokenKind::LBrace)
        && idx > 0
        && matches!(tokens[idx - 1].kind, TokenKind::LBrace | TokenKind::RBrace)
}

/// The spans of each `delivery` section keyword, for the `keyword` semantic token.
pub fn delivery_keyword_spans(source: &str) -> Vec<Span> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    (0..tokens.len())
        .filter(|&i| is_section_keyword(source, &tokens, i))
        .map(|i| tokens[i].span)
        .collect()
}

/// Hover on the `delivery` keyword or on one of its keys. `None` anywhere else,
/// so an ordinary name `delivery` (a parameter, a field) keeps its own hover.
pub fn describe_delivery_at(source: &str, offset: usize) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let idx = tokens
        .iter()
        .position(|t| t.span.start <= offset && offset < t.span.end)?;
    if is_section_keyword(source, &tokens, idx) {
        return Some(SECTION_DOC.to_string());
    }
    let name = source.get(tokens[idx].span.start..tokens[idx].span.end)?;
    if tokens[idx].kind != TokenKind::Ident
        || tokens.get(idx + 1).map(|t| t.kind) != Some(TokenKind::Colon)
        || !in_delivery_field_position(source, tokens[idx].span.start)
    {
        return None;
    }
    let (key, doc) = DELIVERY_FIELDS.iter().find(|(k, _)| *k == name)?;
    Some(format!("```bynk\ndelivery {{ {key}: … }}\n```\n\n{doc}"))
}

/// What `RetryAfter` hovers as (#3881).
const RETRY_AFTER_DOC: &str = "```bynk\nRetryAfter(reason: String, delay: Duration) -> QueueResult\n```\n\nRetries the message after `delay`, which overrides the consumer's `retryDelay` for this message. Cloudflare retries after a whole number of seconds, 1 second to 24 hours: a delay written as a literal outside that range is `bynk.queue.retry_delay_out_of_range`, and a computed one is rounded up and clamped, with the clamp logged. `Retry(reason)` retries after the consumer's `retryDelay`, or at once when it has none. With `attempts` from a `QueueDelivery`, a handler can back off: `RetryAfter(\"unavailable\", 30.seconds * delivery.attempts)`.";

/// Hover on a `RetryAfter` verdict. `None` anywhere else, so a user's own name
/// `RetryAfter` (a declared variant, a binding) keeps its own hover, which
/// resolves first.
pub fn describe_retry_after_at(source: &str, offset: usize) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let idx = tokens
        .iter()
        .position(|t| t.span.start <= offset && offset < t.span.end)?;
    let word = source.get(tokens[idx].span.start..tokens[idx].span.end)?;
    (tokens[idx].kind == TokenKind::Ident && word == "RetryAfter")
        .then(|| RETRY_AFTER_DOC.to_string())
}

/// The signature label of a queue service's test address, `svc.message(…)`: the
/// message, and, when the handler declares a `QueueDelivery`, the optional
/// delivery that follows it. `None` when `service` is not a queue consumer.
pub fn message_address_label(
    service: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    let mut label = None;
    for_each_unit(doc_text, files, |unit| {
        let items = match unit {
            SourceUnit::Context(c) => &c.items,
            SourceUnit::Commons(c) => &c.items,
            _ => return,
        };
        for item in items {
            let CommonsItem::Service(s) = item else {
                continue;
            };
            if s.name.name != service || !matches!(s.protocol, ServiceProtocol::Queue { .. }) {
                continue;
            }
            let Some(h) = s
                .handlers
                .iter()
                .find(|h| matches!(h.kind, HandlerKind::Message))
            else {
                continue;
            };
            let message = h
                .params
                .iter()
                .find(|p| !bynk_syntax::queue::is_queue_delivery(&p.type_ref));
            let has_delivery = h
                .params
                .iter()
                .any(|p| bynk_syntax::queue::is_queue_delivery(&p.type_ref));
            if let Some(m) = message {
                let mut text = format!(
                    "{service}.message({}: {}",
                    m.name.name,
                    type_ref_str(&m.type_ref)
                );
                if has_delivery {
                    text.push_str(", delivery: QueueDelivery?");
                }
                text.push(')');
                label = Some(text);
            }
        }
    });
    label
}

/// #3879: the context that consumes `queue` and the message type it takes, as
/// written, over the current buffer and the project's files. `None` when no
/// context in the project consumes it.
pub fn consumer_of(
    queue: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<(String, String)> {
    let mut found = None;
    for_each_unit(doc_text, files, |unit| {
        let SourceUnit::Context(c) = unit else {
            return;
        };
        for item in &c.items {
            let CommonsItem::Service(s) = item else {
                continue;
            };
            let ServiceProtocol::Queue { name } = &s.protocol else {
                continue;
            };
            if name != queue || found.is_some() {
                continue;
            }
            let message = s
                .handlers
                .iter()
                .filter(|h| matches!(h.kind, HandlerKind::Message))
                .find_map(|h| {
                    h.params
                        .iter()
                        .find(|p| !bynk_syntax::queue::is_queue_delivery(&p.type_ref))
                });
            if let Some(m) = message {
                found = Some((c.name.joined(), type_ref_str(&m.type_ref)));
            }
        }
    });
    found
}

/// #3879: the line a `Queue` instance's hover gains: who consumes the queue and
/// what the consumer takes, which a `sendValue*` is checked against, or that
/// nothing in this project consumes it (so nothing is checked).
pub fn consumer_note(
    queue: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> String {
    match consumer_of(queue, doc_text, files) {
        Some((context, ty)) => format!(
            "\nConsumed by `{context}`, which takes `{ty}`: `sendValue`, `sendValueWith` and `sendValueBatch` are checked against it.\n"
        ),
        None => "\nNo consumer in this project, so a send is not checked against a message type.\n"
            .to_string(),
    }
}

/// #3879: `content`, a hover for a derived `Queue` instance, with its consumer
/// note added; any other hover is returned as it is.
pub fn with_consumer_note(
    content: String,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> String {
    const MARK: &str = "sending to queue `";
    if !content.starts_with("```bynk\ncapability ") {
        return content;
    }
    let Some(rest) = content.find(MARK).map(|i| &content[i + MARK.len()..]) else {
        return content;
    };
    let Some(queue) = rest.split('`').next() else {
        return content;
    };
    let note = consumer_note(queue, doc_text, files);
    format!("{content}{note}")
}

/// #3879: the signature label of `Instance.sendValue*(…)` on a `Queue` instance
/// whose queue the project consumes, with the message type the consumer expects
/// in place of the open `T`. `None` for any other receiver or operation.
pub fn send_value_label(
    instance: &str,
    op: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    if !["sendValue", "sendValueWith", "sendValueBatch"].contains(&op) {
        return None;
    }
    // A capability name is scoped to its context, so the instance is looked up in the
    // document's own unit first; only when it is declared elsewhere does the project's
    // other units answer.
    let from_unit = |unit: &SourceUnit| -> Option<String> {
        let SourceUnit::Context(c) = unit else {
            return None;
        };
        c.items.iter().find_map(|item| {
            let CommonsItem::Capability(cap) = item else {
                return None;
            };
            let d = cap.derived.as_ref()?;
            let ann = d.annotations.iter().find(|a| a.name.name == "name")?;
            match &ann.args.first()?.value.kind {
                bynk_syntax::ast::ExprKind::StrLit(q)
                    if cap.name.name == instance && d.base.name == "Queue" =>
                {
                    Some(q.clone())
                }
                _ => None,
            }
        })
    };
    let own = tokenize(doc_text)
        .ok()
        .and_then(|tokens| bynk_syntax::parser::parse_unit_with_recovery(&tokens, doc_text).0)
        .and_then(|unit| from_unit(&unit));
    let mut queue = own;
    if queue.is_none() {
        for_each_unit(doc_text, files, |unit| {
            if queue.is_none() {
                queue = from_unit(unit);
            }
        });
    }
    let (_, ty) = consumer_of(&queue?, doc_text, files)?;
    Some(match op {
        "sendValue" => format!("{instance}.sendValue(message: {ty}) -> Effect[()]"),
        "sendValueWith" => {
            format!("{instance}.sendValueWith(message: {ty}, options: QueueSend) -> Effect[()]")
        }
        _ => format!("{instance}.sendValueBatch(messages: List[{ty}]) -> Effect[()]"),
    })
}

/// The cursor is at a parameter-*name* position in the parameter list of an
/// `on message( … )` handler header: the innermost open paren follows `on message`,
/// and the current parameter has no `:` yet.
pub fn in_message_param_name_position(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_paren(text, offset) else {
        return false;
    };
    let head = text[..open].trim_end();
    if word_before_brace(head, head.len()) != "message" {
        return false;
    }
    let before = head[..head.len() - "message".len()].trim_end();
    if !before.ends_with("on") {
        return false;
    }
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return false;
    };
    !seg.rsplit(['\n', ',']).next().unwrap_or("").contains(':')
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "service outbox from queue(\"q\") {\n  delivery {\n    retries: 5,\n    deadLetter: None,\n  }\n  on message(m: String) -> Effect[QueueResult] { Ack }\n}\n";

    #[test]
    fn the_section_keyword_and_each_key_hover() {
        let at = |needle: &str| SRC.find(needle).unwrap() + 1;
        assert!(
            describe_delivery_at(SRC, at("delivery"))
                .unwrap()
                .contains("delivery policy")
        );
        let retries = describe_delivery_at(SRC, at("retries")).unwrap();
        assert!(retries.contains("0 to 100") && retries.contains("default is 3"));
        assert!(
            describe_delivery_at(SRC, at("deadLetter"))
                .unwrap()
                .contains("dead_letter_queue")
        );
        // A name that is not a key of the section has no hover here.
        assert_eq!(describe_delivery_at(SRC, at("message")), None);
    }

    #[test]
    fn completion_offers_the_keys_not_yet_written() {
        let text = "service s from queue(\"q\") {\n  delivery {\n    retries: 5,\n    ";
        assert!(in_delivery_field_position(text, text.len()));
        let names: Vec<&str> = unwritten_keys(text, text.len())
            .iter()
            .map(|(n, _)| *n)
            .collect();
        assert!(!names.contains(&"retries"));
        assert!(names.contains(&"deadLetter") && names.len() == 5);
        // After a `:` the cursor is at a value, not a name.
        let value = "service s from queue(\"q\") {\n  delivery {\n    retries: ";
        assert!(!in_delivery_field_position(value, value.len()));
    }

    #[test]
    fn the_section_keyword_is_tokenised_and_a_plain_name_is_not() {
        assert_eq!(delivery_keyword_spans(SRC).len(), 1);
        let plain = "fn f(delivery: Int) -> Int { delivery }\n";
        assert!(delivery_keyword_spans(plain).is_empty());
    }

    const META_SRC: &str = "context c\n\nconsumes bynk.cloudflare { QueueDelivery }\n\ntype Job = { id: String }\n\nservice w from queue(\"q\") {\n  on message(job: Job, delivery: QueueDelivery) -> Effect[QueueResult] { Ack }\n}\n\nservice p from queue(\"p\") {\n  on message(job: Job) -> Effect[QueueResult] { Ack }\n}\n";

    #[test]
    fn the_message_address_shows_the_optional_delivery_only_when_the_handler_takes_it() {
        assert_eq!(
            message_address_label("w", META_SRC, None).as_deref(),
            Some("w.message(job: Job, delivery: QueueDelivery?)")
        );
        assert_eq!(
            message_address_label("p", META_SRC, None).as_deref(),
            Some("p.message(job: Job)")
        );
        assert_eq!(message_address_label("nope", META_SRC, None), None);
    }

    #[test]
    fn a_message_handlers_parameter_name_position_is_found() {
        let t = "service w from queue(\"q\") {\n  on message(";
        assert!(in_message_param_name_position(t, t.len()));
        let t2 = "service w from queue(\"q\") {\n  on message(job: Job, ";
        assert!(in_message_param_name_position(t2, t2.len()));
        let t3 = "service w from queue(\"q\") {\n  on message(job: ";
        assert!(!in_message_param_name_position(t3, t3.len()));
    }

    const PROJECT: &str = "context shop.orders\n\nconsumes bynk.cloudflare { Queue }\n\ncapability OrderEvents = Queue @name(\"order-events\")\ncapability Audit = Queue @name(\"audit\")\n\nservice fulfil from queue(\"order-events\") {\n  on message(order: Order) -> Effect[QueueResult] { Ack }\n}\n";

    #[test]
    fn a_queue_instance_hover_names_its_consumer_or_says_there_is_none() {
        let hover = |queue: &str| {
            with_consumer_note(
                format!(
                    "```bynk\ncapability X = Queue\n```\n\nEmits `[[queues.producers]]` sending to queue `{queue}` (x)."
                ),
                PROJECT,
                None,
            )
        };
        assert!(hover("order-events").contains("Consumed by `shop.orders`, which takes `Order`"));
        assert!(hover("audit").contains("No consumer in this project"));
        assert_eq!(
            with_consumer_note("type A = Int".to_string(), PROJECT, None),
            "type A = Int"
        );
    }

    #[test]
    fn send_value_signature_shows_the_consumers_message_type() {
        assert_eq!(
            send_value_label("OrderEvents", "sendValue", PROJECT, None).as_deref(),
            Some("OrderEvents.sendValue(message: Order) -> Effect[()]")
        );
        assert_eq!(
            send_value_label("OrderEvents", "sendValueBatch", PROJECT, None).as_deref(),
            Some("OrderEvents.sendValueBatch(messages: List[Order]) -> Effect[()]")
        );
        assert_eq!(send_value_label("Audit", "sendValue", PROJECT, None), None);
        assert_eq!(send_value_label("OrderEvents", "send", PROJECT, None), None);
    }

    #[test]
    fn retry_after_hovers_with_its_payload_and_range() {
        let src = "on message(m: String) -> Effect[QueueResult] { RetryAfter(\"x\", 30.seconds) }";
        let at = src.find("RetryAfter").unwrap() + 2;
        let hover = describe_retry_after_at(src, at).unwrap();
        assert!(hover.contains("RetryAfter(reason: String, delay: Duration)"));
        assert!(hover.contains("1 second to 24 hours"));
        assert_eq!(
            describe_retry_after_at(src, src.find("Effect").unwrap() + 1),
            None
        );
    }

    #[test]
    fn send_value_signature_prefers_the_documents_own_instance() {
        let other = "context shop.billing\n\nconsumes bynk.cloudflare { Queue }\n\ncapability OrderEvents = Queue @name(\"billing-events\")\n\nservice pay from queue(\"billing-events\") {\n  on message(invoice: Invoice) -> Effect[QueueResult] { Ack }\n}\n";
        let dir = std::env::temp_dir().join("bynk-sigdoc-4281");
        let mut files = HashMap::new();
        files.insert(dir.join("billing.bynk"), other.to_string());
        // The document's own `OrderEvents` names `order-events`, whatever another context's names.
        assert_eq!(
            send_value_label("OrderEvents", "sendValue", PROJECT, Some(&files)).as_deref(),
            Some("OrderEvents.sendValue(message: Order) -> Effect[()]")
        );
    }
}
