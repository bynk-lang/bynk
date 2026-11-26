//! #3819: the agent annotation `@jurisdiction("eu" | "fedramp")`, which keeps every instance's
//! state in one Cloudflare jurisdiction.
//!
//! A Durable Object id minted through `namespace.jurisdiction("eu")` is a *different* id from the
//! one the unrestricted namespace mints for the same name, so the choice is made once, per agent
//! class, and a change after deploy would orphan every instance (the deploy plan treats it as
//! destructive). The values are `R2`'s and `D1`'s (`bynk_std::JURISDICTIONS`).

use bynk_syntax::ast::{AgentDecl, Annotation, ExprKind};
use bynk_syntax::error::CompileError;

/// The annotation's name.
pub const JURISDICTION: &str = "jurisdiction";

/// The `@jurisdiction` of `agent`, when it carries a valid one.
pub fn jurisdiction_of(agent: &AgentDecl) -> Option<&str> {
    agent
        .annotations
        .iter()
        .find(|a| a.name.name == JURISDICTION)
        .and_then(literal)
        .filter(|j| crate::firstparty::is_jurisdiction(j))
}

/// The one positional string literal of `ann`.
fn literal(ann: &Annotation) -> Option<&str> {
    match ann.args.as_slice() {
        [arg] if arg.label.is_none() => match &arg.value.kind {
            ExprKind::StrLit(s) => Some(s.as_str()),
            _ => None,
        },
        _ => None,
    }
}

/// Validate one `@jurisdiction` annotation: one string literal that is a jurisdiction. `seen` is
/// whether an earlier `@jurisdiction` was written on this agent.
pub fn validate(ann: &Annotation, seen: bool, errors: &mut Vec<CompileError>) {
    if seen {
        errors.push(CompileError::new(
            "bynk.agent.bad_annotation",
            ann.span,
            "an agent carries at most one `@jurisdiction` annotation",
        ));
        return;
    }
    match literal(ann) {
        None => errors.push(
            CompileError::new(
                "bynk.agent.bad_annotation",
                ann.span,
                "`@jurisdiction` takes one string literal",
            )
            .with_note("write `@jurisdiction(\"eu\")`"),
        ),
        Some(text) if !crate::firstparty::is_jurisdiction(text) => errors.push(
            CompileError::new(
                "bynk.agent.bad_annotation",
                ann.span,
                format!("`{text}` is not a jurisdiction an agent can be pinned to"),
            )
            .with_note(crate::firstparty::jurisdictions_note()),
        ),
        Some(_) => {}
    }
}
