//! #3939 (the API keys track, `design/tracks/api-keys.md`): the `ApiKey` actor scheme.
//!
//! An `ApiKey` actor's keys are opaque bearer secrets the service mints, stores only as peppered
//! HMACs and verifies at the seam (D1–D3). This module holds the scheme's compile-time facts: the
//! seam data the emitter lowers, the names of its two generated agents, and the declaration and
//! admission rules. The agents themselves are generated as Bynk source by
//! `bynk_project::api_key_agents`, as a `sessions = true` cookie's agent is (ADR 0998 B).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bynk_syntax::CompileError;
use bynk_syntax::ast::{ActorDecl, ByClause, Handler, Ident, ServiceProtocol, TypeDecl, TypeRef};
use bynk_syntax::span::Span;

use crate::actors::Scheme;
use crate::symbols::UnitTable;

/// The prefix a key carries when the actor names none.
pub const DEFAULT_PREFIX: &str = "bk";

/// The keys an `ApiKey` actor's config admits.
const KEYS: &[&str] = &["secret", "previous", "prefix", "header", "ttl"];

/// The keys this slice reads. `previous` and `ttl` arrive with expiry and rotation (track §14,
/// slice 3), and until then are refused rather than read and ignored.
const KEYS_NOW: &[&str] = &["secret", "prefix", "header"];

/// The marker the generated agents' documentation begins with, so the checker can tell them from
/// an author's declaration of the same (reserved) name.
pub const GENERATED_MARKER: &str = "Generated for the `ApiKey` actor";

/// The data the emitter needs to lower an `ApiKey` seam or member: the `by` binder, the actor's
/// name, the pepper's environment variable, the key prefix, the header the key is read from (`None`
/// for `Authorization: Bearer`) and the identity type the store answers. The checker guarantees the
/// secret is present and the identity is a string-constructible, context-owned type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeySeam {
    /// The identity binder, or `None` for `by <Actor>` (verify, capture nothing).
    pub binder: Option<String>,
    /// The declaring actor's name.
    pub actor: String,
    /// The environment variable holding the pepper.
    pub secret: String,
    /// The key prefix, `[a-z][a-z0-9]{1,15}`.
    pub prefix: String,
    /// The header the raw key is read from, or `None` for `Authorization: Bearer <key>`.
    pub header: Option<String>,
    /// The identity type the key store answers.
    pub identity_type: String,
}

impl ApiKeySeam {
    /// The header the seam consumes, lower-cased: the named one, else `authorization`.
    pub fn consumed_header(&self) -> String {
        self.header
            .as_deref()
            .unwrap_or("authorization")
            .to_ascii_lowercase()
    }
}

/// The name of the agent holding one identity's keys, keyed by the random handle (D1 B).
pub fn keys_agent_name(actor: &str) -> String {
    format!("{actor}Keys")
}

/// The name of the agent mapping an identity to its handle, used only by the members (D1 B).
pub fn key_owner_agent_name(actor: &str) -> String {
    format!("{actor}KeyOwner")
}

/// Whether `actor` declares the `ApiKey` scheme.
pub fn is_api_key_actor(actor: &ActorDecl) -> bool {
    actor
        .auth
        .as_ref()
        .and_then(|a| Scheme::from_name(a.name.as_str()))
        == Some(Scheme::ApiKey)
}

/// The seam of one `ApiKey` actor declaration.
pub fn api_key_seam_of(actor: &ActorDecl, binder: Option<String>) -> Option<ApiKeySeam> {
    if !is_api_key_actor(actor) {
        return None;
    }
    let secret = actor.scheme_arg("secret")?.value.as_str()?.to_string();
    let TypeRef::Named(id) = actor.identity.as_ref()? else {
        return None;
    };
    Some(ApiKeySeam {
        binder,
        actor: actor.name.name.clone(),
        secret,
        prefix: actor
            .scheme_arg("prefix")
            .and_then(|a| a.value.as_str())
            .unwrap_or(DEFAULT_PREFIX)
            .to_string(),
        header: actor
            .scheme_arg("header")
            .and_then(|a| a.value.as_str())
            .map(str::to_string),
        identity_type: id.name.clone(),
    })
}

/// A handler's `ApiKey` seam, when its `by` clause names a single local `ApiKey` actor (an actor
/// sum with an `ApiKey` member is slice 4's).
pub fn api_key_seam_for(
    handler: &Handler,
    actors: &HashMap<String, ActorDecl>,
) -> Option<ApiKeySeam> {
    let by = handler.by_clause.as_ref()?;
    if by.is_sum() {
        return None;
    }
    let actor = actors.get(&by.primary().name)?;
    api_key_seam_of(actor, by.binder.as_ref().map(|b| b.name.clone()))
}

/// Whether a `by` clause names a single local `ApiKey` actor.
pub fn by_clause_is_api_key(by: &ByClause, actors: &HashMap<String, ActorDecl>) -> bool {
    !by.is_sum() && actors.get(&by.primary().name).is_some_and(is_api_key_actor)
}

/// Whether `prefix` is a legal key prefix: `[a-z][a-z0-9]{1,15}`.
pub fn is_legal_prefix(prefix: &str) -> bool {
    let b = prefix.as_bytes();
    (2..=16).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
}

/// Whether `name` is an RFC 9110 field name (a `token`).
fn is_header_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

/// Headers an `ApiKey` actor cannot read its key from: `authorization` is the default form (omit
/// `header`), and the others carry something the platform or the browser owns.
const REFUSED_HEADERS: &[&str] = &[
    "authorization",
    "cookie",
    "host",
    "content-length",
    "content-type",
];

/// #3939: validate one `ApiKey` actor declaration (track §4): the closed key set, a `secret`, a
/// legal and distinct `prefix`, a legal `header`, secrets distinct from every other actor's, and a
/// string-constructible identity. `Some(Scheme::ApiKey)` in `check_actor_contracts` calls this.
pub(crate) fn check_api_key_actor(
    actor: &ActorDecl,
    auth: &Ident,
    table: &UnitTable,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    check_keys(actor, errors);
    check_secret(actor, auth, table, errors);
    check_prefix(actor, table, errors);
    check_header(actor, errors);
    check_reserved_names(actor, table, errors);
    match &actor.identity {
        None => errors.push(identity_error(auth.span, "must declare")),
        Some(id) if !crate::context_checks::is_string_constructible(id, types) => {
            errors.push(identity_error(id.span(), "must be"))
        }
        Some(_) => {}
    }
}

fn bad_arg(span: Span, key: &str, want: &str) -> CompileError {
    CompileError::new(
        "bynk.actor.api_key_bad_arg",
        span,
        format!("an `ApiKey` actor's `{key}` must be {want}"),
    )
    .with_note(
        "write `auth = ApiKey(secret = \"<ENV_NAME>\")`; the optional keys are `prefix` (default `bk`) and `header` (default `Authorization: Bearer`)",
    )
}

fn identity_error(span: Span, verb: &str) -> CompileError {
    CompileError::new(
        "bynk.actor.api_key_identity_not_string_constructible",
        span,
        format!("an `ApiKey` actor {verb} a string-constructible `identity`"),
    )
    .with_note(
        "the key store answers the identity it was minted for as a string, which the seam mints through the identity type's `of`: declare `identity = T` where `T` is a refined or opaque `String`",
    )
}

/// The closed key set; a repeated key is refused, since the first would win quietly.
fn check_keys(actor: &ActorDecl, errors: &mut Vec<CompileError>) {
    let mut seen: HashSet<&str> = HashSet::new();
    for arg in &actor.auth_config {
        let key = arg.key.name.as_str();
        if !seen.insert(key) {
            errors.push(
                CompileError::new(
                    "bynk.actor.api_key_bad_arg",
                    arg.key.span,
                    format!("the `ApiKey` config key `{key}` is given more than once"),
                )
                .with_note("only the first value would be read: keep one"),
            );
        }
        if !KEYS.contains(&key) {
            errors.push(
                CompileError::new(
                    "bynk.actor.api_key_unknown_key",
                    arg.key.span,
                    format!("unknown `ApiKey` config key `{key}`"),
                )
                .with_note("the keys are `secret`, `prefix` and `header`"),
            );
        } else if !KEYS_NOW.contains(&key) {
            errors.push(
                CompileError::new(
                    "bynk.actor.api_key_unknown_key",
                    arg.key.span,
                    format!("the `ApiKey` config key `{key}` is not supported yet"),
                )
                .with_note(
                    "key expiry (`ttl`) and pepper rotation (`previous`) arrive in a later release; until then a key does not expire, and the pepper is rotated only by revoking every key",
                ),
            );
        }
    }
}

/// The pepper: named, a string, and not another actor's secret or previous key.
fn check_secret(
    actor: &ActorDecl,
    auth: &Ident,
    table: &UnitTable,
    errors: &mut Vec<CompileError>,
) {
    let Some(arg) = actor.scheme_arg("secret") else {
        errors.push(
            CompileError::new(
                "bynk.actor.api_key_missing_secret",
                auth.span,
                "an `ApiKey` actor must name its pepper",
            )
            .with_note(
                "write `auth = ApiKey(secret = \"<ENV_NAME>\")`: the environment variable holding the pepper the stored keys are hashed with (`bynk deploy` mints it when unset)",
            ),
        );
        return;
    };
    let Some(secret) = arg.value.as_str() else {
        errors.push(bad_arg(
            arg.span,
            "secret",
            "an env binding name, as a string",
        ));
        return;
    };
    let mut others: Vec<&ActorDecl> = table
        .actors
        .values()
        .filter(|o| o.name.name != actor.name.name)
        .collect();
    others.sort_by(|a, b| a.name.name.cmp(&b.name.name));
    for other in others {
        let shared = ["secret", "previous"]
            .iter()
            .any(|k| other.scheme_arg(k).and_then(|a| a.value.as_str()) == Some(secret));
        if shared {
            errors.push(
                CompileError::new(
                    "bynk.actor.api_key_secret_shared",
                    arg.span,
                    format!(
                        "the pepper `{secret}` of `ApiKey` actor `{}` is also used by actor `{}`",
                        actor.name.name, other.name.name
                    ),
                )
                .with_note(
                    "a pepper hashes one actor's keys only: a variable shared with another scheme would let that scheme's credentials be checked against it; give each actor its own variable",
                ),
            );
        }
    }
}

/// The prefix: legal, and distinct from every other `ApiKey` actor's in the context.
fn check_prefix(actor: &ActorDecl, table: &UnitTable, errors: &mut Vec<CompileError>) {
    let (prefix, span) = match actor.scheme_arg("prefix") {
        Some(arg) => match arg.value.as_str() {
            Some(p) if is_legal_prefix(p) => (p.to_string(), arg.span),
            _ => {
                errors.push(bad_arg(
                    arg.span,
                    "prefix",
                    "a string of 2 to 16 lower-case letters and digits, starting with a letter",
                ));
                return;
            }
        },
        None => (DEFAULT_PREFIX.to_string(), actor.name.span),
    };
    // Report each pair once, at the actor that sorts later.
    for other in table.actors.values() {
        if other.name.name >= actor.name.name {
            continue;
        }
        let Some(theirs) = api_key_seam_of(other, None) else {
            continue;
        };
        if theirs.prefix == prefix {
            errors.push(
                CompileError::new(
                    "bynk.actor.api_key_prefix_shared",
                    span,
                    format!(
                        "`ApiKey` actors `{}` and `{}` share the prefix `{prefix}`",
                        other.name.name, actor.name.name
                    ),
                )
                .with_note(
                    "the prefix tells a leaked key's owner and, in an actor sum, which actor a key is for: give each `ApiKey` actor its own `prefix`",
                ),
            );
        }
    }
}

/// The header: a legal field name that is not one the key cannot be read from.
fn check_header(actor: &ActorDecl, errors: &mut Vec<CompileError>) {
    let Some(arg) = actor.scheme_arg("header") else {
        return;
    };
    match arg.value.as_str() {
        Some(h) if is_header_token(h) && !REFUSED_HEADERS.contains(&h.to_ascii_lowercase().as_str()) => {}
        Some(h) if h.eq_ignore_ascii_case("authorization") => errors.push(
            bad_arg(arg.span, "header", "a header other than `Authorization`")
                .with_note("omit `header` to read the key from `Authorization: Bearer <key>`"),
        ),
        _ => errors.push(bad_arg(
            arg.span,
            "header",
            "an HTTP header name other than `Authorization`, `Cookie`, `Host`, `Content-Length` and `Content-Type`",
        )),
    }
}

/// #3939: an `ApiKey` actor on a handler: admitted on HTTP alone, never as an actor-sum member yet
/// (slice 4), and never on a WebSocket upgrade, which has no liveness check (track §3, ADR 1061
/// A). `None` when `scheme` is not `ApiKey` or the handler admits it.
pub(crate) fn by_member_refusal(
    protocol: &ServiceProtocol,
    by: &ByClause,
    scheme: Scheme,
    member: Span,
) -> Option<CompileError> {
    if scheme != Scheme::ApiKey {
        return None;
    }
    let (message, note) = match protocol {
        ServiceProtocol::WebSocket { .. } => (
            "an `ApiKey` actor is not admissible on a WebSocket `on open`",
            "a WebSocket upgrade does not ask the key store, so a revoked key would keep its socket; authenticate the upgrade with a `Bearer`, `Oidc` or `SignedCookie` actor",
        ),
        ServiceProtocol::Http if by.is_sum() => (
            "an `ApiKey` actor is not admissible as a member of an actor sum yet",
            "guard the route with the `ApiKey` actor alone (`by c: Client`); `ApiKey` members of an actor sum arrive in a later release",
        ),
        _ => return None,
    };
    Some(CompileError::new("bynk.actor.scheme_not_admissible", member, message).with_note(note))
}

/// #3939 (D1 D): the generated agents' names are reserved in a context that declares an `ApiKey`
/// actor. An author's item that takes one is `bynk.actor.api_key_agent_name_reserved`; the
/// generator leaves its own agent out in that case, so an item with the name that is not the
/// generated agent is the author's.
fn check_reserved_names(actor: &ActorDecl, table: &UnitTable, errors: &mut Vec<CompileError>) {
    let owner = &actor.name.name;
    for reserved in [keys_agent_name(owner), key_owner_agent_name(owner)] {
        let taken: Option<Span> = if let Some(agent) = table.agents.get(&reserved) {
            (!is_generated_agent(agent)).then_some(agent.name.span)
        } else {
            table
                .types
                .get(&reserved)
                .map(|t| t.name.span)
                .or_else(|| table.fns.get(&reserved).map(|f| f.name.ident().span))
                .or_else(|| table.services.get(&reserved).map(|s| s.name.span))
                .or_else(|| table.actors.get(&reserved).map(|a| a.name.span))
                .or_else(|| table.capabilities.get(&reserved).map(|c| c.name.span))
        };
        let Some(span) = taken else { continue };
        errors.push(
            CompileError::new(
                "bynk.actor.api_key_agent_name_reserved",
                span,
                format!(
                    "`{reserved}` is the name of an agent the compiler generates for the `ApiKey` actor `{owner}`"
                ),
            )
            .with_note(format!(
                "an `ApiKey` actor's keys live in generated agents named `{}` and `{}`: rename this declaration",
                keys_agent_name(owner),
                key_owner_agent_name(owner)
            )),
        );
    }
}

/// Whether `agent` is one the compiler generated for an `ApiKey` actor.
pub fn is_generated_agent(agent: &bynk_syntax::ast::AgentDecl) -> bool {
    agent
        .documentation
        .as_ref()
        .is_some_and(|d| d.as_str().trim_start().starts_with(GENERATED_MARKER))
}

/// #3939 (D3 F, track §12): `bynk.actor.api_key_unlimited`, a warning on an HTTP route an `ApiKey`
/// actor guards that applies no rate limit before authentication. A well-formed fabricated key
/// is cheap to make (its checksum is public) and each one reaches the key store, so the defence,
/// `@rateLimit(Limiter, key: clientIp)`, must not be opt-in. `key: identity` does not count: a
/// fabricated key never verifies, so it never reaches that limiter.
pub(crate) fn unlimited_warning(handler: &Handler, table: &UnitTable) -> Option<CompileError> {
    let seam = api_key_seam_for(handler, &table.actors)?;
    let by = handler.by_clause.as_ref()?;
    let limited = crate::http_rate_limit::route_rate_limit(handler, table)
        .is_some_and(|r| r.key == crate::http_rate_limit::RateLimitKey::ClientIp);
    if limited {
        return None;
    }
    Some(
        CompileError::new(
            "bynk.actor.api_key_unlimited",
            by.span,
            format!(
                "this route is guarded by the `ApiKey` actor `{}` but applies no rate limit before authentication",
                seam.actor
            ),
        )
        .with_note(
            "anyone can make a well-formed key (its checksum is public), and each one the seam parses costs a key-store request; limit by client address before the key is checked: declare `capability PerIp = RateLimit @limit(100) @period(60.seconds)` and write `@rateLimit(PerIp, key: clientIp)` on the route. A service rate-limited at its edge can leave this warning",
        ),
    )
}

/// #3939 (D2 D): whether the named type is, or holds at any depth, an `ApiKeyValue`.
pub fn carries_api_key_value(name: &str, types: &HashMap<String, Arc<TypeDecl>>) -> bool {
    fn walk(
        name: &str,
        types: &HashMap<String, Arc<TypeDecl>>,
        seen: &mut HashSet<String>,
    ) -> bool {
        let Some(decl) = types.get(name) else {
            return false;
        };
        if crate::context_checks::secret_kind(decl)
            == Some(crate::context_checks::SecretKind::ApiKeyValue)
        {
            return true;
        }
        if !seen.insert(name.to_string()) {
            return false;
        }
        let fields: Vec<&TypeRef> = match &decl.body {
            bynk_syntax::ast::TypeBody::Record(r) => r.fields.iter().map(|f| &f.type_ref).collect(),
            bynk_syntax::ast::TypeBody::Sum(s) => s
                .variants
                .iter()
                .flat_map(|v| v.payload.iter().map(|f| &f.type_ref))
                .collect(),
            _ => Vec::new(),
        };
        fields.into_iter().any(|f| {
            bynk_syntax::ast::type_ref_heads(f)
                .iter()
                .any(|h| walk(h, types, seen))
        })
    }
    walk(name, types, &mut HashSet::new())
}

/// #3939 (D2 D): an HTTP handler's result. `Effect[HttpResult[ApiKeySecret]]` is the one place a
/// minted key leaves the service: an `ApiKeySecret` exists only as `mint` answered it (no Bynk
/// code can build an `ApiKeyValue`), and its codec shows the key that once. Every other result
/// takes the ordinary boundary rule.
pub(crate) fn reject_http_result(
    r: &TypeRef,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    let mut inner = r;
    if let TypeRef::Effect(t, _) = inner {
        inner = t;
    }
    if let TypeRef::HttpResult(t, _, _) = inner
        && let TypeRef::Named(id) = &**t
        && types.get(&id.name).is_some_and(|d| {
            crate::context_checks::is_firstparty_decl(d)
                && matches!(d.body, bynk_syntax::ast::TypeBody::Record(_))
                && d.name.name == "ApiKeySecret"
        })
    {
        return;
    }
    crate::context_checks::reject_fn_types(r, "a service handler signature", types, errors);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_rule_matches_the_track() {
        for ok in ["bk", "acme", "a1", "abcdefghijklmnop"] {
            assert!(is_legal_prefix(ok), "{ok}");
        }
        for bad in ["", "a", "1a", "Acme", "ac_me", "abcdefghijklmnopq", "ac-me"] {
            assert!(!is_legal_prefix(bad), "{bad}");
        }
    }

    #[test]
    fn agent_names_follow_the_actor() {
        assert_eq!(keys_agent_name("Client"), "ClientKeys");
        assert_eq!(key_owner_agent_name("Client"), "ClientKeyOwner");
    }
}
