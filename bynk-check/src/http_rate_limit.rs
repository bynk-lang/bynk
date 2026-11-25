//! #3926: `@rateLimit(Limiter, key: …)` on an `on http` route: the router asks a
//! rate limiter before it reads the request body, so a flood of large bodies
//! costs a counter and a `429`, not a full read and decode.
//!
//! The annotation names a `RateLimit` instance the route's context declares
//! (`capability SignupLimit = RateLimit @limit(10) @period(60.seconds)`) and
//! the key it counts by. This module is the checker half: the shape of the
//! annotation and what it names. The router's step is `bynk-emit`'s.
//!
//! This increment counts by `clientIp`, the request's `CF-Connecting-IP`, checked
//! ahead of every other router step so it also protects the authentication
//! step. `identity` (the verified actor, checked after authentication and before
//! the body) is part of the annotation's design and is refused until the router
//! verifies the actor ahead of the body read.

use bynk_syntax::ast::{Annotation, ExprKind, Handler, HandlerKind};
use bynk_syntax::error::CompileError;

use crate::symbols::{DerivedInstance, UnitTable};

/// What the router needs of a valid `@rateLimit`: the limiter's binding and
/// window, and the key it counts by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitRoute {
    /// The limiter's name as the context declares it (`SignupLimit`).
    pub limiter: String,
    /// The limiter's Worker binding (`RATELIMIT_SIGNUP_LIMIT`).
    pub binding: String,
    /// The limiter's window in seconds: the `Retry-After` of a `429`.
    pub period_secs: i64,
    pub key: RateLimitKey,
}

/// What a limit counts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitKey {
    /// The request's `CF-Connecting-IP`, checked before the actor.
    ClientIp,
}

/// The route's valid `@rateLimit`, if it carries one. The checker has reported
/// anything invalid, so a route that does not read here is left unlimited.
pub fn route_rate_limit(handler: &Handler, table: &UnitTable) -> Option<RateLimitRoute> {
    let ann = handler
        .annotations
        .iter()
        .find(|a| a.name.name == "rateLimit")?;
    let (limiter, key) = parse_args(ann).ok()?;
    let instance = limiter_instance(table, &limiter)?;
    Some(RateLimitRoute {
        binding: instance.binding.clone(),
        period_secs: instance.number("period").unwrap_or(60),
        limiter,
        key: match key.as_str() {
            "clientIp" => RateLimitKey::ClientIp,
            _ => return None,
        },
    })
}

/// The `RateLimit` instance `name` the context declares.
fn limiter_instance<'a>(table: &'a UnitTable, name: &str) -> Option<&'a DerivedInstance> {
    table.derived.get(name).filter(|d| d.is_ratelimit())
}

/// The annotation's two arguments: the limiter's name and the key word. `Err`
/// carries the diagnostic's text.
fn parse_args(ann: &Annotation) -> Result<(String, String), &'static str> {
    const SHAPE: &str =
        "`@rateLimit` takes a limiter and a key: `@rateLimit(SignupLimit, key: clientIp)`";
    let [first, second] = ann.args.as_slice() else {
        return Err(SHAPE);
    };
    let limiter = match (&first.label, &first.value.kind) {
        (None, ExprKind::Ident(id)) => id.name.clone(),
        _ => return Err(SHAPE),
    };
    let key = match (&second.label, &second.value.kind) {
        (Some(label), ExprKind::Ident(id)) if label.name == "key" => id.name.clone(),
        _ => return Err(SHAPE),
    };
    Ok((limiter, key))
}

/// Validate every `@rateLimit` of the context's services and agents (an agent's handler is
/// never `on http`, so one there is `bynk.http.rate_limit_not_http`, not ignored).
pub(crate) fn validate_rate_limits(table: &UnitTable, errors: &mut Vec<CompileError>) {
    let service_handlers = table.services.values().flat_map(|s| s.handlers.iter());
    let agent_handlers = table.agents.values().flat_map(|a| a.handlers.iter());
    for handler in service_handlers.chain(agent_handlers) {
        validate_handler(handler, table, errors);
    }
}

fn validate_handler(handler: &Handler, table: &UnitTable, errors: &mut Vec<CompileError>) {
    let mut found = handler
        .annotations
        .iter()
        .filter(|a| a.name.name == "rateLimit");
    let Some(ann) = found.next() else {
        return;
    };
    for extra in found {
        errors.push(CompileError::new(
            "bynk.http.rate_limit_duplicate",
            extra.span,
            "a route carries at most one `@rateLimit`",
        ).with_note("a shared limiter counts across every route that names it, by key; name a second limiter on another route instead"));
    }
    if !matches!(handler.kind, HandlerKind::Http { .. }) {
        errors.push(
            CompileError::new(
                "bynk.http.rate_limit_not_http",
                ann.span,
                "`@rateLimit` is only valid on an `on http` route",
            )
            .with_note("it is the router that asks the limiter, before the request body is read"),
        );
        return;
    }
    let (limiter, key) = match parse_args(ann) {
        Ok(args) => args,
        Err(shape) => {
            errors.push(CompileError::new(
                "bynk.http.rate_limit_bad_args",
                ann.span,
                shape,
            ));
            return;
        }
    };
    if limiter_instance(table, &limiter).is_none() {
        let declared: Vec<&str> = {
            let mut v: Vec<&str> = table
                .derived
                .iter()
                .filter(|(_, d)| d.is_ratelimit())
                .map(|(n, _)| n.as_str())
                .collect();
            v.sort_unstable();
            v
        };
        let mut err = CompileError::new(
            "bynk.http.rate_limit_unknown_limiter",
            ann.args[0].value.span,
            format!("`{limiter}` is not a `RateLimit` capability this context declares"),
        );
        err = if declared.is_empty() {
            err.with_note("declare one in the context: `capability SignupLimit = RateLimit @limit(10) @period(60.seconds)`")
        } else {
            err.with_note(format!("the context declares: {}", declared.join(", ")))
        };
        errors.push(err);
    }
    match key.as_str() {
        "clientIp" => {}
        "identity" => errors.push(
            CompileError::new(
                "bynk.http.rate_limit_key_unavailable",
                ann.args[1].value.span,
                "`key: identity` is not available yet",
            )
            .with_note(
                "`identity` counts by the verified caller and needs the router to authenticate before it reads the body, which it does not yet; use `key: clientIp`, or call the limiter's `allow(key)` in the handler for a limit keyed on the caller",
            ),
        ),
        other => errors.push(
            CompileError::new(
                "bynk.http.rate_limit_bad_args",
                ann.args[1].value.span,
                format!("`{other}` is not a `@rateLimit` key"),
            )
            .with_note("the keys are `clientIp` and, once the router authenticates before the body, `identity`"),
        ),
    }
}
