//! #3936: hover on the `by` of a handler that names a sum of actors (`by who: User | ApiClient`):
//! the order its members are tried in, and the check each one runs.

use std::collections::HashMap;

use bynk_syntax::ast::{ActorDecl, ByClause, CommonsItem, SourceUnit};

/// Hover on the `by` keyword of a sum clause: each member in the order the request is tried
/// against, with the credential it reads and the check it runs. `None` when `offset` is not on the
/// `by` of a sum clause.
pub fn describe_sum_at(source: &str, offset: usize) -> Option<String> {
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
            let Some(by) = handler.by_clause.as_ref().filter(|b| b.is_sum()) else {
                continue;
            };
            // The clause's span starts at `by`.
            if by.span.start <= offset
                && offset < by.span.start + 2
                && source.get(by.span.start..by.span.start + 2) == Some("by")
            {
                return Some(describe(by, &actors));
            }
        }
    }
    None
}

fn describe(by: &ByClause, actors: &HashMap<&str, &ActorDecl>) -> String {
    let mut out = String::from(
        "```bynk\nby who: A | B\n```\n\nAn actor sum: the request is tried against each member in \
         this order, and the first that verifies is the actor the body matches on. A request none \
         accepts is `401`.\n\n",
    );
    for (i, a) in by.actors.iter().enumerate() {
        let check = match actors.get(a.name.as_str()) {
            Some(decl) => match decl.auth.as_ref().map(|i| i.name.as_str()) {
                Some("Bearer") => {
                    "verifies an HS256 token in `Authorization: Bearer …`".to_string()
                }
                Some("Oidc") => {
                    "verifies an OIDC token in `Authorization: Bearer …` against the issuer's JWKS"
                        .to_string()
                }
                Some("Signature") => "verifies an HMAC signature over the body".to_string(),
                Some("SignedCookie") => "reads the session cookie; on a state-changing request \
                    with one present it first refuses a cross-site request (`403`), then verifies \
                    the cookie"
                    .to_string(),
                _ => "admits any request".to_string(),
            },
            // A prelude actor: `Visitor` admits everyone.
            None => "admits any request".to_string(),
        };
        out.push_str(&format!("{}. `{}`: {check}\n", i + 1, a.name));
    }
    out.push_str(
        "\nA request carrying both a cookie and a token is the first listed member's. List the token \
         first when clients may hold both, so an explicit token wins over an ambient cookie.\n",
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "context shop.api\n\ntype UserId = String where NonEmpty\n\n\
actor User { auth = SignedCookie(secret = \"K\", ttl = 2.hours), identity = UserId }\n\
actor ApiClient { auth = Oidc(issuer = \"https://i.example\", audience = \"a\", jwks = \"https://i.example/j\"), identity = UserId }\n\n\
service api from http {\n  on GET(\"/x\") () -> Effect[HttpResult[String]] by who: User | ApiClient {\n    match who {\n      User(u) => Ok(u)\n      ApiClient(c) => Ok(c)\n    }\n  }\n}\n";

    #[test]
    fn hover_on_the_by_of_a_sum_lists_the_members_in_order() {
        let at = SRC.find("by who").unwrap() + 1;
        let hover = describe_sum_at(SRC, at).expect("hover");
        let user = hover.find("1. `User`").expect("cookie arm first");
        let client = hover.find("2. `ApiClient`").expect("token arm second");
        assert!(user < client, "{hover}");
        assert!(hover.contains("cross-site request"), "{hover}");
        assert!(hover.contains("OIDC token"), "{hover}");
    }

    #[test]
    fn hover_elsewhere_on_the_handler_is_not_the_sum() {
        let at = SRC.find("on GET").unwrap() + 1;
        assert!(describe_sum_at(SRC, at).is_none());
    }
}
