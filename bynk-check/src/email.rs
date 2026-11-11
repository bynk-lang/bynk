//! Inbound email: the `from email("addr")` service protocol (#3328, slice 1).
//!
//! The checker side of the protocol's seventh member: address validation,
//! handler shape (`on message(mail: InboundEmail) -> Effect[EmailVerdict]`), the
//! one-claim-per-address rule (DECISION A), and the slice-1 refusal to emit a
//! `from email` service as a Worker (slice 2 builds the `email()` export).
//!
//! `InboundEmail`, `EmailVerdict` and the rest are first-party types exported
//! by `bynk.cloudflare`, so the handler-shape checks compare type *names*; the
//! body is then type-checked against the real declarations like any other.

use std::collections::{BTreeMap, HashMap};

use bynk_syntax::ast::{
    CommonsItem, Handler, HandlerKind, ServiceDecl, ServiceProtocol, SourceUnit, TypeRef,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::context_checks::ts_type_ref_display;
use crate::project_model::{BuildTarget, ErrorSink};
use bynk_project::{ParsedFile, UnitKind};

/// The first-party type a `from email` handler receives.
pub const INBOUND_EMAIL: &str = "InboundEmail";
/// The first-party verdict a `from email` handler returns.
pub const EMAIL_VERDICT: &str = "EmailVerdict";

/// Why an address is not a claimable recipient, or `None` if it is.
///
/// A claim is `local@domain`, or `*@domain` for the domain's catch-all. The
/// domain is dotted labels of letters, digits and `-` (not starting or ending
/// a label with `-`). A local part is non-empty and free of whitespace,
/// control, angle-bracket, quote, comma, `*` and `@` characters. Display
/// names (`Support <a@b>`) are refused: Email Routing matches the bare
/// address.
pub fn address_problem(address: &str) -> Option<&'static str> {
    let Some((local, domain)) = address.split_once('@') else {
        return Some("it has no `@`");
    };
    if local.is_empty() {
        return Some("the part before `@` is empty");
    }
    if local != "*"
        && local
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || "<>\"',@*()[]\\;:".contains(c))
    {
        return Some("the part before `@` has a character an address cannot hold");
    }
    if domain.is_empty() || !domain.contains('.') {
        return Some("the domain needs at least two dot-separated labels");
    }
    let label_ok = |l: &str| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    if !domain.split('.').all(label_ok) {
        return Some("the domain is not dotted labels of letters, digits and `-`");
    }
    None
}

/// The routing key two claims are compared by: the domain is
/// case-insensitive, and so is the local part, deliberately: RFC 5321 allows a
/// case-sensitive local part, but Email Routing matches addresses without
/// regard to case, so two claims differing only in case would be one rule.
fn claim_key(address: &str) -> String {
    address.to_ascii_lowercase()
}

/// Check every `from email` service of one context: address, handler shape and
/// the one-service-per-address rule.
pub fn check_email_services(table: &crate::symbols::UnitTable, errors: &mut Vec<CompileError>) {
    let services = &table.services;
    let mut first_claim: HashMap<String, Span> = HashMap::new();
    // Source order, so "first claimed here" is the earlier declaration.
    let mut ordered: Vec<&ServiceDecl> = services.values().collect();
    ordered.sort_by_key(|s| s.name.span);
    for service in ordered {
        let ServiceProtocol::Email { address } = &service.protocol else {
            continue;
        };
        if let Some(why) = address_problem(address) {
            errors.push(
                CompileError::new(
                    "bynk.email.invalid_address",
                    service.name.span,
                    format!("`{address}` is not an address a service can claim: {why}"),
                )
                .with_note(
                    "write a bare address, `from email(\"support@shop.example\")`, or a domain's catch-all, `from email(\"*@shop.example\")`",
                ),
            );
        } else if let Some(prev) = first_claim.get(&claim_key(address)).copied() {
            errors.push(
                CompileError::new(
                    "bynk.email.duplicate_address",
                    service.name.span,
                    format!("another `from email` service already claims `{address}`"),
                )
                .with_label(prev, "first claimed here")
                .with_note(
                    "mail is dispatched by its envelope recipient, so each address has one service; a literal address and `*@domain` may coexist, and the literal wins",
                ),
            );
        } else {
            first_claim.insert(claim_key(address), service.name.span);
        }
        let messages: Vec<&Handler> = service
            .handlers
            .iter()
            .filter(|h| h.kind == HandlerKind::Message)
            .collect();
        for extra in messages.iter().skip(1) {
            errors.push(
                CompileError::new(
                    "bynk.email.duplicate_handler",
                    extra.span,
                    format!(
                        "the `from email` service `{}` has more than one `on message` handler — it needs exactly one",
                        service.name.name
                    ),
                )
                .with_label(messages[0].span, "the service's first `on message` handler"),
            );
        }
        for handler in messages {
            validate_handler(handler, table, errors);
        }
    }
}

/// The protocol-shape checks of one context, then the email ones (kept apart
/// so the long protocol checker does not grow).
pub(crate) fn check_service_protocols_and_email(
    table: &crate::symbols::UnitTable,
    types: &HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
    errors: &mut Vec<CompileError>,
    tys: &std::sync::Arc<crate::checker::Types>,
) {
    crate::context_checks::check_service_protocols(table, types, errors, tys);
    check_email_services(table, errors);
    check_sender_only_on_email(table, errors);
}

/// `Sender` yields the envelope sender of inbound mail, so (like `Caller`,
/// which is `on call`'s) it is admissible only on a `from email` handler,
/// though its `Internal` scheme is otherwise valid on cron, queue and events.
fn check_sender_only_on_email(table: &crate::symbols::UnitTable, errors: &mut Vec<CompileError>) {
    for service in table.services.values() {
        if matches!(service.protocol, ServiceProtocol::Email { .. }) {
            continue;
        }
        let handler_by = service.handlers.iter().filter_map(|h| h.by_clause.as_ref());
        for by in handler_by.chain(service.default_by.as_ref()) {
            let names_sender = by
                .actors
                .iter()
                .any(|a| a.name == "Sender" && !table.actors.contains_key(&a.name));
            if names_sender {
                errors.push(
                    CompileError::new(
                        "bynk.actor.scheme_not_admissible",
                        by.span,
                        "the `Sender` actor is not admissible outside a `from email` service",
                    )
                    .with_note(
                        "`Sender` carries the envelope sender of inbound mail; cron takes `Scheduler`, queue takes `Producer`",
                    ),
                );
            }
        }
    }
}

/// `on message(mail: InboundEmail) -> Effect[EmailVerdict]`.
fn validate_handler(
    handler: &Handler,
    table: &crate::symbols::UnitTable,
    errors: &mut Vec<CompileError>,
) {
    // A type of the same name declared in this context is not the first-party
    // one the Worker's `email()` entry builds and reads, so it does not count.
    let first_party = |name: &str| !table.types.contains_key(name);
    match handler.params.as_slice() {
        [p] => {
            if named(&p.type_ref) != Some(INBOUND_EMAIL) || !first_party(INBOUND_EMAIL) {
                errors.push(
                    CompileError::new(
                        "bynk.email.bad_params",
                        p.type_ref.span(),
                        format!(
                            "an email `on message` handler takes the message as `InboundEmail`, but this parameter is `{}`",
                            ts_type_ref_display(&p.type_ref)
                        ),
                    )
                    .with_note(
                        "import it: `consumes bynk.cloudflare { InboundEmail, EmailVerdict }`; a type of that name declared in this context is not the first-party one",
                    ),
                );
            }
        }
        params => errors.push(
            CompileError::new(
                "bynk.email.bad_params",
                handler.span,
                format!(
                    "an email `on message` handler takes exactly one parameter (the `InboundEmail`), got {}",
                    params.len()
                ),
            )
            .with_note("write `on message(mail: InboundEmail) -> Effect[EmailVerdict]`"),
        ),
    }
    let verdict = matches!(
        &handler.return_type,
        TypeRef::Effect(inner, _)
            if named(inner) == Some(EMAIL_VERDICT) && first_party(EMAIL_VERDICT)
    );
    if !verdict {
        errors.push(
            CompileError::new(
                "bynk.email.return_not_email_verdict",
                handler.return_type.span(),
                format!(
                    "an email `on message` handler must return `Effect[EmailVerdict]`, but got `{}`",
                    ts_type_ref_display(&handler.return_type)
                ),
            )
            .with_note(
                "the verdict is the decision: `Accept`, `Reject(reason)`, `Forward(to)` or `Reply(message)`",
            ),
        );
    }
}

fn named(t: &TypeRef) -> Option<&str> {
    match t {
        TypeRef::Named(id) => Some(id.name.as_str()),
        _ => None,
    }
}

/// Slice 1 builds the surface and the test vertical; the Workers `email()`
/// export is slice 2 (ADR 0131's split, as `from websocket`'s was). Until
/// then a deployable Workers build of a `from email` service is refused
/// rather than emitted half-working.
pub fn phase_workers_email_unsupported(
    target: BuildTarget,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    errors: &mut ErrorSink,
) {
    if target != BuildTarget::Workers {
        return;
    }
    for (name, indices) in groups {
        if kinds.get(name) != Some(&UnitKind::Context) {
            continue;
        }
        for &i in indices {
            let SourceUnit::Context(ctx) = &parsed[i].unit() else {
                continue;
            };
            for item in &ctx.items {
                let CommonsItem::Service(s) = item else {
                    continue;
                };
                if matches!(s.protocol, ServiceProtocol::Email { .. }) {
                    errors.push_for(
                        Some(&parsed[i].identity_path()),
                        CompileError::new(
                            "bynk.email.workers_unsupported",
                            s.name.span,
                            "inbound email is not yet emitted for Workers (#3328 slice 2)",
                        )
                        .with_note(
                            "a `from email` service runs in a suite today (`support.message(…)`); build for `--target bundle`, or wait for the Workers mapping",
                        ),
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::address_problem;

    #[test]
    fn addresses() {
        for ok in ["support@shop.example", "*@shop.example", "a.b+c@x-y.co.uk"] {
            assert_eq!(address_problem(ok), None, "{ok}");
        }
        for bad in [
            "support",
            "@shop.example",
            "a@b",
            "a b@shop.example",
            "Support <a@shop.example>",
            "a*@shop.example",
            "a@-b.example",
            "a@shop..example",
            "a@shop.example,b@shop.example",
        ] {
            assert!(address_problem(bad).is_some(), "{bad}");
        }
    }
}
