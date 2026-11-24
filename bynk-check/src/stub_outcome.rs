//! #3894: the `stub` words of a consumed service that are not a value: `rejected <Refusal>`,
//! `faulted` and `unreachable`. `returns <value>` stays the way to stub `Handled(value)`.
//!
//! A call's failure reaches a handler only as `<context>.<service>.outcome(…)`'s `CallOutcome`,
//! so the words are a consumed service's alone: a capability's `stub` is a provider, and a
//! provider faults with `fails`.

use bynk_syntax::ast::{StubClause, StubRhs};
use bynk_syntax::error::CompileError;

use crate::test_suites::ResolvedStub;

/// The cases of `bynk`'s `Refusal`: what `rejected` may name.
pub const REFUSALS: [&str; 7] = [
    "TooLarge",
    "NotAuthenticated",
    "UnsupportedWire",
    "NoSuchService",
    "NotAdmitted",
    "StaleContract",
    "Undecodable",
];

/// What is wrong with `clause`'s `rejected`, `faulted` or `unreachable`, if anything: it must
/// stub a consumed service, and `rejected` must name a `Refusal` case.
pub fn stub_outcome_word_errors(clause: &StubClause, rp: &ResolvedStub) -> Vec<CompileError> {
    let word = match &clause.rhs {
        StubRhs::Rejected(..) => "rejected",
        StubRhs::Faulted(_) => "faulted",
        StubRhs::Unreachable(_) => "unreachable",
        _ => return Vec::new(),
    };
    if rp.service_context.is_none() {
        return vec![
            CompileError::new(
                "bynk.stub.outcome_word",
                clause.rhs.span(),
                format!(
                    "`{word}` stubs the outcome of a call to a consumed service, but `{}` is not one",
                    rp.cap
                ),
            )
            .with_note("a capability's `stub` ends `returns <value>`, `fails`, or `never`"),
        ];
    }
    if let StubRhs::Rejected(refusal, _) = &clause.rhs
        && !REFUSALS.contains(&refusal.name.as_str())
    {
        return vec![
            CompileError::new(
                "bynk.stub.outcome_word",
                refusal.span,
                format!("`{}` is not a `Refusal`", refusal.name),
            )
            .with_note(format!("`rejected` names one of {}", REFUSALS.join(", "))),
        ];
    }
    Vec::new()
}
