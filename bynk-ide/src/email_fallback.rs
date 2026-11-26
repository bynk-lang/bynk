//! The editor surfaces of an email service's `fallback { }` section (#3885): the `forward` key
//! completion offers, the hover on the section keyword and on `forward`, and the section
//! keyword's semantic token. All lexical, like the other service policies, since the section is
//! a contextual keyword.

use bynk_syntax::lexer::{TokenKind, tokenize};
use bynk_syntax::span::Span;

use crate::completion::{innermost_open_brace, word_before_brace};

/// The one key of a `fallback { }` section, with its hover text.
pub const FORWARD_DOC: &str = "the address mail the handler faulted on, or that could not be read, is forwarded to instead of being rejected with a permanent SMTP error. A string literal holding a bare address, which must be a verified destination address on the account (`bynk deploy` warns of one that is not) and not one a `from email` service of the project claims. The forward carries `X-Bynk-Fallback: handler-fault` or `unreadable`, and nothing from the message.";

/// What the `fallback` keyword hovers as.
const SECTION_DOC: &str = "```bynk\nfallback { forward: \"<address>\" }\n```\n\nWhere mail this service could not process goes, instead of a permanent bounce: a message its handler faulted on (or returned a non-verdict for), or that could not be parsed or exceeded a parser limit. An oversized message, a refused `Reply` and a refused `Forward` are still rejected. Without a fallback, every one of them is.";

/// The cursor is at a key-*name* position inside a `fallback { … }` block, as
/// [`crate::queue_delivery::in_delivery_field_position`] is for `delivery`.
pub fn in_fallback_field_position(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_brace(text, offset) else {
        return false;
    };
    if word_before_brace(text, open) != "fallback" {
        return false;
    }
    let Some(seg) = text.get(open + 1..offset.min(text.len())) else {
        return false;
    };
    let current = seg.rsplit(['\n', ',']).next().unwrap_or("");
    !current.contains(':')
}

/// Whether the `forward` key is already written in the `fallback { … }` block around `offset`.
pub fn forward_written(text: &str, offset: usize) -> bool {
    let Some(open) = innermost_open_brace(text, offset) else {
        return false;
    };
    let close = text[open..]
        .find('}')
        .map_or(text.len(), |i| open + i)
        .max(offset.min(text.len()));
    text.get(open + 1..close).is_some_and(|block| {
        block
            .split([',', '\n'])
            .any(|part| part.trim_start().starts_with("forward:"))
    })
}

/// Whether the token at `idx` is the section keyword: `fallback {` right after the service's
/// `{` or a previous section's `}`.
fn is_section_keyword(source: &str, tokens: &[bynk_syntax::lexer::Token], idx: usize) -> bool {
    let word = |i: usize| source.get(tokens[i].span.start..tokens[i].span.end);
    tokens[idx].kind == TokenKind::Ident
        && word(idx) == Some("fallback")
        && tokens.get(idx + 1).map(|t| t.kind) == Some(TokenKind::LBrace)
        && idx > 0
        && matches!(tokens[idx - 1].kind, TokenKind::LBrace | TokenKind::RBrace)
}

/// The spans of each `fallback` section keyword, for the `keyword` semantic token.
pub fn fallback_keyword_spans(source: &str) -> Vec<Span> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    (0..tokens.len())
        .filter(|&i| is_section_keyword(source, &tokens, i))
        .map(|i| tokens[i].span)
        .collect()
}

/// Hover on the `fallback` keyword or on its `forward` key. `None` anywhere else, so an ordinary
/// name `fallback` (a parameter, a field) keeps its own hover.
pub fn describe_fallback_at(source: &str, offset: usize) -> Option<String> {
    let tokens = tokenize(source).ok()?;
    let idx = tokens
        .iter()
        .position(|t| t.span.start <= offset && offset < t.span.end)?;
    if is_section_keyword(source, &tokens, idx) {
        return Some(SECTION_DOC.to_string());
    }
    let name = source.get(tokens[idx].span.start..tokens[idx].span.end)?;
    (tokens[idx].kind == TokenKind::Ident
        && name == "forward"
        && tokens.get(idx + 1).map(|t| t.kind) == Some(TokenKind::Colon)
        && in_fallback_field_position(source, tokens[idx].span.start))
    .then(|| format!("```bynk\nfallback {{ forward: \"<address>\" }}\n```\n\n{FORWARD_DOC}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "context c\n\nservice s from email(\"a@b.example\") {\n  fallback { forward: \"ops@b.example\" }\n  on message(m: InboundEmail) -> Effect[EmailVerdict] by Sender { Accept }\n}\n";

    fn at(word: &str) -> usize {
        SRC.find(word).unwrap() + 1
    }

    #[test]
    fn the_keyword_and_its_key_hover_and_nothing_else_does() {
        assert!(
            describe_fallback_at(SRC, at("fallback"))
                .unwrap()
                .contains("permanent bounce")
        );
        assert!(
            describe_fallback_at(SRC, at("forward"))
                .unwrap()
                .contains("X-Bynk-Fallback")
        );
        assert!(describe_fallback_at(SRC, at("message")).is_none());
        // An ordinary name `fallback` keeps its own hover.
        let local = "context c\n\nfn f(fallback: Int) -> Int { fallback }\n";
        assert!(describe_fallback_at(local, local.find("fallback }").unwrap() + 1).is_none());
    }

    #[test]
    fn the_keyword_is_a_semantic_token_only_as_a_section() {
        assert_eq!(fallback_keyword_spans(SRC).len(), 1);
        let local = "context c\n\nfn f(fallback: Int) -> Int { fallback }\n";
        assert!(fallback_keyword_spans(local).is_empty());
    }

    #[test]
    fn the_key_is_offered_where_a_key_name_goes_and_until_it_is_written() {
        let open = "service s from email(\"a@b.example\") {\n  fallback { \n}";
        let at = open.find("{ \n").unwrap() + 2;
        assert!(in_fallback_field_position(open, at));
        assert!(!forward_written(open, at));
        assert!(forward_written(SRC, SRC.find("forward").unwrap()));
        assert!(!in_fallback_field_position(SRC, SRC.find("\"ops").unwrap()));
    }
}
