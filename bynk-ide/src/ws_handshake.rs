//! #3900: hover on the `by` of a `from websocket` service's `on open` whose actor reads a Bearer
//! token: the handshake a client connects with.

use std::collections::HashMap;

use bynk_syntax::ast::{ActorDecl, CommonsItem, HandlerKind, SourceUnit};

/// Hover on the `by` keyword of an `on open` whose `by` clause names a `Bearer` actor (alone or in
/// a sum): the subprotocol pair a browser offers, what the response selects, and what is refused.
/// An actor sum's own description follows it. `None` when `offset` is not on such a `by`.
pub fn describe_ws_open_at(source: &str, offset: usize) -> Option<String> {
    let tokens = bynk_syntax::lexer::tokenize(source).ok()?;
    let unit = bynk_syntax::parser::parse_unit_with_recovery(&tokens, source).0?;
    let SourceUnit::Context(context) = &unit else {
        return None;
    };
    let actors: HashMap<&str, &ActorDecl> = context
        .items
        .iter()
        .filter_map(|item| match item {
            CommonsItem::Actor(a) => Some((a.name.name.as_str(), a)),
            _ => None,
        })
        .collect();
    for item in &context.items {
        let CommonsItem::Service(service) = item else {
            continue;
        };
        for handler in &service.handlers {
            let Some(by) = handler.by_clause.as_ref() else {
                continue;
            };
            let on_by = by.span.start <= offset
                && offset < by.span.start + 2
                && source.get(by.span.start..by.span.start + 2) == Some("by");
            let reads_bearer = by.actors.iter().any(|a| {
                actors
                    .get(a.name.as_str())
                    .is_some_and(|d| d.auth.as_ref().map(|i| i.name.as_str()) == Some("Bearer"))
            });
            if on_by && matches!(handler.kind, HandlerKind::Open) && reads_bearer {
                let mut out = String::from(
                    "A `Bearer` upgrade reads its token from the subprotocols the client offers:\n\n\
                     ```js\nnew WebSocket(url, [\"bynk.bearer\", token])\n```\n\n\
                     The `101` response selects `bynk.bearer`, never the token, so a browser keeps \
                     the connection and the credential is not echoed in a header. A missing or bad \
                     token is `401` (unless a `None` member of a sum admits the request); the token offered \
                     alone, `[token]`, is `400`.",
                );
                if let Some(sum) = crate::actor_sum::describe_sum_at(source, offset) {
                    out.push_str("\n\n---\n\n");
                    out.push_str(&sum);
                }
                return Some(out);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "context chat.room\n\ntype UserId = String where NonEmpty\n\n\
actor User { auth = Bearer(secret = \"AUTH\"), identity = UserId }\n\
actor Anon { auth = None }\n\n\
service gw from websocket(in: String, out: String) {\n  on open () -> Effect[()] by u: User {\n    ()\n  }\n}\n\n\
service open_gw from websocket(in: String, out: String) {\n  on open () -> Effect[()] by a: Anon {\n    ()\n  }\n}\n";

    #[test]
    fn hover_on_the_by_of_a_bearer_open_gives_the_handshake() {
        let at = SRC.find("by u").unwrap() + 1;
        let hover = describe_ws_open_at(SRC, at).expect("hover");
        assert!(hover.contains("[\"bynk.bearer\", token]"), "{hover}");
        assert!(hover.contains("`400`"), "{hover}");
    }

    #[test]
    fn an_anonymous_open_and_other_positions_are_not_described() {
        assert!(describe_ws_open_at(SRC, SRC.find("by a").unwrap() + 1).is_none());
        assert!(describe_ws_open_at(SRC, SRC.find("on open").unwrap() + 1).is_none());
    }
}
