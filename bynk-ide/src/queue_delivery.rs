//! The editor surfaces of a queue consumer's `delivery { }` section (#3877):
//! the keys completion offers, the hover on the section keyword and on each key,
//! and the section keyword's semantic token. All lexical, like the other service
//! policies, since the section is a contextual keyword.

use bynk_syntax::lexer::{TokenKind, tokenize};
use bynk_syntax::span::Span;

use crate::completion::{innermost_open_brace, word_before_brace};

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
}
