//! An email service's fallback (#3885): `fallback { forward: "ops@shop.example" }`.
//!
//! A handler that faults, or a message that cannot be parsed, is rejected with a permanent SMTP
//! error unless the service names a verified address to forward it to. The checks here are the
//! block's own: it is valid only on a `from email` service ([`validate_fallback`]), its one field
//! is an address ([`fallback_address`]), and the address is not one a `from email` service of
//! the project claims, or two Bynk services could forward faults to each other forever
//! ([`phase_fallback_claimed`]).

use std::collections::{BTreeMap, HashMap};

use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{
    CommonsItem, ExprKind, FallbackPolicy, ServiceDecl, ServiceProtocol, SourceUnit,
};
use bynk_syntax::error::CompileError;

use crate::email::address_problem;
use crate::project_model::ErrorSink;

/// Why a fallback's address is not one a message can be forwarded to, or `None` if it is: a bare
/// address, as `from email(…)` takes, that is not a domain's catch-all.
fn forward_problem(address: &str) -> Option<String> {
    if address.starts_with("*@") {
        return Some("a fallback is one address, not a domain's catch-all".to_string());
    }
    address_problem(address).map(|why| format!("{address:?} is not an address: {why}"))
}

/// The address a service's fallback forwards to, when it names a usable one.
pub fn fallback_address(service: &ServiceDecl) -> Option<&str> {
    let policy = service.fallback.as_ref()?;
    match &policy.field("forward")?.kind {
        ExprKind::StrLit(address) if forward_problem(address).is_none() => Some(address),
        _ => None,
    }
}

fn bad_option(span: bynk_syntax::span::Span, message: String, note: &str) -> CompileError {
    CompileError::new("bynk.email.fallback_bad_option", span, message).with_note(note.to_string())
}

/// Check one service's `fallback { }`: only on a `from email` service, with its one field
/// `forward`, a string literal holding a bare address.
pub fn validate_fallback(service: &ServiceDecl, errors: &mut Vec<CompileError>) {
    let Some(policy) = &service.fallback else {
        return;
    };
    if !matches!(service.protocol, ServiceProtocol::Email { .. }) {
        errors.push(
            CompileError::new(
                "bynk.email.fallback_not_email",
                policy.span,
                "a `fallback { }` policy is only valid on a `from email` service",
            )
            .with_note(
                "it names where mail a handler faulted on is forwarded, which only a `from email` service receives",
            ),
        );
        return;
    }
    check_fields(policy, errors);
}

fn check_fields(policy: &FallbackPolicy, errors: &mut Vec<CompileError>) {
    let mut seen: Vec<&str> = Vec::new();
    for field in &policy.fields {
        if field.name.name != "forward" {
            errors.push(bad_option(
                field.name.span,
                format!("`{}` is not a `fallback` field", field.name.name),
                "the only field is `forward`: the address mail a handler faulted on is forwarded to",
            ));
        } else if seen.contains(&"forward") {
            errors.push(bad_option(
                field.name.span,
                "`forward` is set more than once".to_string(),
                "a fallback forwards to one address",
            ));
        }
        seen.push(field.name.name.as_str());
    }
    let Some(value) = policy.field("forward") else {
        errors.push(bad_option(
            policy.span,
            "a `fallback { }` policy needs a `forward` address".to_string(),
            "write `fallback { forward: \"ops@shop.example\" }`",
        ));
        return;
    };
    match &value.kind {
        ExprKind::StrLit(address) => {
            if let Some(why) = forward_problem(address) {
                errors.push(bad_option(
                    value.span,
                    format!("`forward` is not an address a message can be forwarded to: {why}"),
                    "write a bare address, `forward: \"ops@shop.example\"`; it must be a verified destination address on the account",
                ));
            }
        }
        _ => errors.push(bad_option(
            value.span,
            "`forward` must be a string literal holding an address".to_string(),
            "write `forward: \"ops@shop.example\"`",
        )),
    }
}

/// #3885: a fallback that a `from email` service of the project claims is refused, because mail
/// forwarded there would be routed back to Bynk, and a fault in that service would forward it
/// again. A claim is an exact address or the catch-all of its domain, compared without regard
/// to case as routing does.
pub fn phase_fallback_claimed(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    errors: &mut ErrorSink,
) {
    let mut claims: Vec<String> = Vec::new();
    let mut fallbacks: Vec<(usize, &ServiceDecl)> = Vec::new();
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context) {
            continue;
        }
        for &i in indices {
            let SourceUnit::Context(ctx) = parsed[i].unit() else {
                continue;
            };
            for item in &ctx.items {
                let CommonsItem::Service(service) = item else {
                    continue;
                };
                if let ServiceProtocol::Email { address, .. } = &service.protocol {
                    claims.push(address.to_lowercase());
                    if service.fallback.is_some() {
                        fallbacks.push((i, service));
                    }
                }
            }
        }
    }
    let by_domain: HashMap<&str, ()> = claims
        .iter()
        .filter_map(|c| c.strip_prefix("*@"))
        .map(|d| (d, ()))
        .collect();
    for (file, service) in fallbacks {
        let Some(address) = fallback_address(service) else {
            continue;
        };
        let address = address.to_lowercase();
        let domain = address.rsplit('@').next().unwrap_or_default();
        if claims.contains(&address) || by_domain.contains_key(domain) {
            let at = service
                .fallback
                .as_ref()
                .and_then(|p| p.field("forward"))
                .map_or(service.name.span, |e| e.span);
            errors.push_for(
                Some(&parsed[file].identity_path()),
                CompileError::new(
                    "bynk.email.fallback_claimed",
                    at,
                    format!("a `from email` service of this project claims `{address}`, so it cannot be a fallback"),
                )
                .with_note(
                    "mail forwarded to a routed address comes straight back to Bynk, and a fault there would forward it again; name a verified destination address outside the project's routes",
                ),
            );
        }
    }
}
