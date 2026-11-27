//! #3939 (the API keys track): editor support for an `ApiKey` actor: its config keys inside
//! `auth = ApiKey(…`, and its `mint` member inside the declaring context.

use crate::completion::{Completion, CompletionKind};

/// Inside the parentheses of `auth = ApiKey(…`, the config keys not yet written.
pub fn key_candidates(line_prefix: &str) -> Option<Vec<Completion>> {
    let open = line_prefix.rfind("ApiKey(")? + "ApiKey(".len();
    let args = &line_prefix[open..];
    if args.contains(')') {
        return None;
    }
    // The key being typed starts after the last comma; a value (after `=`) completes nothing.
    let current = args.rsplit(',').next().unwrap_or(args);
    if current.contains('=') {
        return Some(Vec::new());
    }
    let written: Vec<&str> = args
        .split(',')
        .filter_map(|part| part.split('=').next().map(str::trim))
        .collect();
    Some(
        [
            (
                "secret",
                "the environment variable holding the pepper the stored keys are hashed with (required; `bynk deploy` mints it when unset)",
            ),
            (
                "prefix",
                "the key's leading tag, 2 to 16 lower-case letters and digits; defaults to `bk`",
            ),
            (
                "header",
                "a header to read the key from; defaults to `Authorization: Bearer <key>`",
            ),
        ]
        .into_iter()
        .filter(|(key, _)| !written.contains(key))
        .map(|(key, detail)| {
            Completion::item(
                key,
                CompletionKind::Field,
                Some(format!("ApiKey key — {detail}")),
            )
        })
        .collect(),
    )
}

/// Whether an actor authenticates with API keys.
pub fn is_api_key_actor(a: &bynk_syntax::ast::ActorDecl) -> bool {
    bynk_check::api_key::is_api_key_actor(a)
}

/// The members an `ApiKey` actor has inside its context, as `(name, signature)`.
pub fn members(a: &bynk_syntax::ast::ActorDecl) -> Vec<(&'static str, String)> {
    let identity = a
        .identity
        .as_ref()
        .map_or_else(|| "identity".to_string(), crate::symbols::type_ref_str);
    vec![
        (
            "list",
            format!("list(identity: {identity}) -> Effect[List[ApiKeyInfo]]"),
        ),
        (
            "mint",
            format!(
                "mint(identity: {identity}, label: ApiKeyLabel) -> Effect[Result[ApiKeySecret, TooManyKeys]]"
            ),
        ),
        (
            "revoke",
            format!("revoke(identity: {identity}, id: ApiKeyId) -> Effect[Bool]"),
        ),
        (
            "revokeAll",
            format!("revokeAll(identity: {identity}) -> Effect[Int]"),
        ),
        (
            "sweep",
            format!("sweep(identity: {identity}) -> Effect[ApiKeySweep]"),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keys_not_yet_written_are_offered() {
        let items = key_candidates("  auth = ApiKey(secret = \"P\", ").expect("inside the parens");
        let labels: Vec<&str> = items.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["prefix", "header"]);
        assert!(key_candidates("  auth = ApiKey(secret = \"P\")").is_none());
        assert_eq!(
            key_candidates("  auth = ApiKey(secret = ").map(|v| v.len()),
            Some(0)
        );
    }
}
