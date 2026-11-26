//! #3926: completion inside `@rateLimit(Limiter, key: …)`: the `RateLimit` instances the
//! document declares, then the `key:` label, then the two keys. Lexical, like the other
//! annotation cells: it reads the text under the cursor, not an analysis.

use crate::completion::{innermost_open_paren, word_before_brace};

/// The keys `@rateLimit` counts by, each with what it is.
pub const RATE_LIMIT_KEYS: &[(&str, &str)] = &[
    (
        "clientIp",
        "the request's `CF-Connecting-IP`, asked before everything else; reliable only behind Cloudflare's edge",
    ),
    (
        "identity",
        "the verified caller (`sub`), asked after the route's actor is verified and before the body is read; a `Bearer`, `Oidc` or `SignedCookie` actor",
    ),
];

/// Where in `@rateLimit( … )` the cursor is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitArg {
    /// The first argument: the limiter's name.
    Limiter,
    /// The second argument's label, before its `:`.
    KeyLabel,
    /// The value after `key:`.
    KeyValue,
}

/// The cursor's place inside a handler's `@rateLimit( … )`, or `None` outside one.
pub fn rate_limit_arg_position(text: &str, offset: usize) -> Option<RateLimitArg> {
    let open = innermost_open_paren(text, offset)?;
    if word_before_brace(text, open) != "rateLimit" {
        return None;
    }
    let head = text[..open].trim_end();
    if !head[..head.len() - "rateLimit".len()]
        .trim_end()
        .ends_with('@')
    {
        return None;
    }
    // `offset` arrives on a char boundary, but slice defensively.
    let seg = text.get(open + 1..offset.min(text.len()))?;
    let mut args = seg.split(',');
    let current = args.next_back().unwrap_or("");
    let first = args.next().is_none();
    match current.split_once(':') {
        Some((label, _)) if label.trim() == "key" => Some(RateLimitArg::KeyValue),
        Some(_) => None,
        None if first => Some(RateLimitArg::Limiter),
        None => Some(RateLimitArg::KeyLabel),
    }
}

/// The names of the `capability X = RateLimit …` instances the document declares, in
/// source order: what `@rateLimit`'s first argument may name.
pub fn rate_limit_limiters(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let rest = line.trim_start().strip_prefix("capability ")?;
            let (name, base) = rest.split_once('=')?;
            let base = base.trim_start();
            let is_rate_limit = base
                .strip_prefix("RateLimit")
                .is_some_and(|after| after.is_empty() || after.starts_with(char::is_whitespace));
            is_rate_limit.then(|| name.trim().to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(doc: &str) -> Option<RateLimitArg> {
        rate_limit_arg_position(doc, doc.len())
    }

    #[test]
    fn the_cursor_is_placed_by_argument() {
        assert_eq!(at("  @rateLimit("), Some(RateLimitArg::Limiter));
        assert_eq!(at("  @rateLimit(Sig"), Some(RateLimitArg::Limiter));
        assert_eq!(
            at("  @rateLimit(SignupLimit, "),
            Some(RateLimitArg::KeyLabel)
        );
        assert_eq!(
            at("  @rateLimit(SignupLimit, ke"),
            Some(RateLimitArg::KeyLabel)
        );
        assert_eq!(
            at("  @rateLimit(SignupLimit, key: "),
            Some(RateLimitArg::KeyValue)
        );
        assert_eq!(
            at("  @rateLimit(SignupLimit, key: cl"),
            Some(RateLimitArg::KeyValue)
        );
    }

    #[test]
    fn other_parentheses_are_not_the_annotation() {
        assert_eq!(at("  @limit("), None);
        assert_eq!(at("  rateLimit("), None, "a call, not the annotation");
        assert_eq!(at("  @rateLimit(SignupLimit, key: clientIp)"), None);
    }

    #[test]
    fn the_declared_limiters_are_the_rate_limit_instances() {
        let doc = "capability SignupLimit = RateLimit @limit(10) @period(60.seconds)\ncapability Sessions = Kv\ncapability ReadLimit = RateLimit @limit(1) @period(10.seconds)\ncapability Other = RateLimiter\n";
        assert_eq!(rate_limit_limiters(doc), ["SignupLimit", "ReadLimit"]);
    }
}
