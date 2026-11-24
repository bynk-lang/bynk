//! #3932: the agent annotation `@busyAfter(d)`, which lowers how long a call waits behind the
//! handlers ahead of it on one instance before the call answers `AgentBusy`.
//!
//! The wait is 30 seconds by default (`AGENT_QUEUE_WAIT_MS`). An agent may *lower* it, never raise
//! it: a longer wait only holds callers longer. It is per agent class.

use bynk_syntax::ast::{AgentDecl, Annotation, ExprKind};
use bynk_syntax::error::CompileError;

/// The longest wait, and so the largest `@busyAfter`: the runtime's own limit, in milliseconds.
pub const BUSY_AFTER_MAX_MS: i64 = 30_000;
/// The shortest `@busyAfter`, in milliseconds.
pub const BUSY_AFTER_MIN_MS: i64 = 1_000;

/// The `@busyAfter` of `agent` in milliseconds, when it carries a valid one.
pub fn busy_after_ms(agent: &AgentDecl) -> Option<i64> {
    agent
        .annotations
        .iter()
        .find(|a| a.name.name == "busyAfter")
        .and_then(duration_ms)
        .filter(|ms| (BUSY_AFTER_MIN_MS..=BUSY_AFTER_MAX_MS).contains(ms))
}

/// The one positional `Duration` literal of `ann`, in milliseconds.
fn duration_ms(ann: &Annotation) -> Option<i64> {
    match ann.args.as_slice() {
        [arg] if arg.label.is_none() => match &arg.value.kind {
            ExprKind::DurationLit { millis, .. } => Some(*millis),
            _ => None,
        },
        _ => None,
    }
}

/// Validate `agent`'s annotations: `@busyAfter` is the only one, written at most once, taking one
/// `Duration` literal from 1 second to 30 seconds.
pub fn validate_agent_annotations(agent: &AgentDecl, errors: &mut Vec<CompileError>) {
    let mut seen = false;
    for ann in &agent.annotations {
        if ann.name.name != "busyAfter" {
            errors.push(
                CompileError::new(
                    "bynk.agent.bad_annotation",
                    ann.span,
                    format!("`@{}` is not an agent annotation", ann.name.name),
                )
                .with_note("the one agent annotation is `@busyAfter(<Duration>)`"),
            );
            continue;
        }
        if std::mem::replace(&mut seen, true) {
            errors.push(CompileError::new(
                "bynk.agent.bad_annotation",
                ann.span,
                "an agent carries at most one `@busyAfter` annotation",
            ));
            continue;
        }
        let Some(ms) = duration_ms(ann) else {
            errors.push(
                CompileError::new(
                    "bynk.agent.bad_annotation",
                    ann.span,
                    "`@busyAfter` takes one `Duration` literal",
                )
                .with_note("write `@busyAfter(5.seconds)`"),
            );
            continue;
        };
        if !(BUSY_AFTER_MIN_MS..=BUSY_AFTER_MAX_MS).contains(&ms) {
            errors.push(
                CompileError::new(
                    "bynk.agent.busy_after_out_of_range",
                    ann.span,
                    format!(
                        "`@busyAfter` must be from 1 second to 30 seconds, but this is {} ms",
                        ms
                    ),
                )
                .with_note(
                    "it lowers the 30-second wait a call has behind the handlers ahead of it; a longer wait only holds callers longer",
                ),
            );
        }
    }
}
