//! #4035: the name of an `expect <call> violates <name>` claim, resolved against the agents the
//! suite reaches.
//!
//! A claim names an invariant or a transition of an agent, or an `@unique` constraint by its
//! field and key (`customers.email`). The runtime matches the fault's own `invariant` name (or
//! `Agent.invariant`), so the compiler's job is to refuse a name nothing can ever raise
//! (`bynk.expect.unknown_invariant`) and a bare name two agents both declare
//! (`bynk.expect.ambiguous_invariant`), and to keep the claim to the tiers where a fault is a
//! throw (`bynk.test.violates_needs_in_process`).

use bynk_syntax::ast::{AgentDecl, Block, Case, ExprKind, FaultsExpr, Ident, TestTier};
use bynk_syntax::error::CompileError;

use std::collections::HashMap;

use crate::symbols::UnitTable;

/// One refusal an agent can raise: the agent and the name the fault carries.
struct Refusal<'a> {
    agent: &'a str,
    name: String,
}

/// Every refusal `agent` can raise: its invariants, its transitions and its `@unique` fields.
fn refusals(agent: &AgentDecl) -> Vec<Refusal<'_>> {
    let mut out: Vec<Refusal<'_>> = Vec::new();
    out.extend(agent.invariants.iter().map(|i| Refusal {
        agent: &agent.name.name,
        name: i.name.name.clone(),
    }));
    out.extend(agent.transitions.iter().map(|t| Refusal {
        agent: &agent.name.name,
        name: t.name.name.clone(),
    }));
    for f in &agent.store_fields {
        for an in f.annotations.iter().filter(|an| an.name.name == "unique") {
            for arg in &an.args {
                if let ExprKind::Ident(by) = &arg.value.kind {
                    out.push(Refusal {
                        agent: &agent.name.name,
                        name: format!("{}.{}", f.name.name, by.name),
                    });
                }
            }
        }
    }
    out
}

/// The claims of `case` that name a refusal, at any depth.
fn violates_claims(block: &Block) -> Vec<&FaultsExpr> {
    fn collect<'a>(e: &'a bynk_syntax::ast::Expr, out: &mut Vec<&'a FaultsExpr>) {
        if let ExprKind::Faults(f) = &e.kind
            && f.violates.is_some()
        {
            out.push(f);
        }
        for c in bynk_syntax::ast::expr_children(e) {
            collect(c, out);
        }
    }
    let mut exprs = Vec::new();
    for s in &block.statements {
        bynk_syntax::ast::statement_exprs(s, &mut exprs);
    }
    exprs.push(&block.tail);
    let mut out = Vec::new();
    for e in exprs {
        collect(e, &mut out);
    }
    out
}

/// The agents the suite reaches: those of its target and participants, ordered by name.
fn reachable<'a>(tables: &[&'a UnitTable]) -> Vec<&'a AgentDecl> {
    let mut agents: Vec<&AgentDecl> = tables
        .iter()
        .flat_map(|t| t.agents.values().map(|a| a.as_ref()))
        .collect();
    agents.sort_by(|a, b| a.name.name.cmp(&b.name.name));
    agents.dedup_by(|a, b| a.name.name == b.name.name);
    agents
}

/// Resolve `path` against `known`: the agents that declare it, by bare name (`name`,
/// `field.index`) or qualified (`Agent.name`, `Agent.field.index`).
fn declaring_agents<'a>(path: &[Ident], known: &[Refusal<'a>]) -> Vec<&'a str> {
    let full = path
        .iter()
        .map(|i| i.name.as_str())
        .collect::<Vec<_>>()
        .join(".");
    let mut agents: Vec<&str> = known
        .iter()
        .filter(|r| r.name == full || format!("{}.{}", r.agent, r.name) == full)
        .map(|r| r.agent)
        .collect();
    agents.sort_unstable();
    agents.dedup();
    agents
}

/// Report a `violates` claim naming nothing the suite reaches, or naming two things.
pub(crate) fn check_violates_names(
    case: &Case,
    unit_tables: &HashMap<String, UnitTable>,
    roots: &[String],
    errors: &mut Vec<CompileError>,
) {
    let tables: Vec<&UnitTable> = roots.iter().filter_map(|r| unit_tables.get(r)).collect();
    let claims = violates_claims(&case.body);
    if claims.is_empty() {
        return;
    }
    let agents = reachable(&tables);
    let known: Vec<Refusal<'_>> = agents.iter().flat_map(|a| refusals(a)).collect();
    for claim in claims {
        let Some(path) = &claim.violates else {
            continue;
        };
        let (Some(first), Some(last)) = (path.first(), path.last()) else {
            continue;
        };
        let span = first.span.merge(last.span);
        let shown = path
            .iter()
            .map(|i| i.name.as_str())
            .collect::<Vec<_>>()
            .join(".");
        match declaring_agents(path, &known).as_slice() {
            [_] => {}
            [] => {
                let mut names: Vec<String> = known
                    .iter()
                    .map(|r| format!("{}.{}", r.agent, r.name))
                    .collect();
                names.dedup();
                let listing = if names.is_empty() {
                    "the agents this suite reaches declare no invariant, transition or `@unique` constraint".to_string()
                } else {
                    format!("reachable: {}", names.join(", "))
                };
                errors.push(
                    CompileError::new(
                        "bynk.expect.unknown_invariant",
                        span,
                        format!("`{shown}` is not an invariant, transition or `@unique` constraint of an agent this suite reaches"),
                    )
                    .with_note(listing),
                );
            }
            many => errors.push(
                CompileError::new(
                    "bynk.expect.ambiguous_invariant",
                    span,
                    format!(
                        "`{shown}` is declared by more than one reachable agent: {}",
                        many.join(", ")
                    ),
                )
                .with_note(format!(
                    "qualify it with its agent: `violates {}.{shown}`",
                    many[0]
                )),
            ),
        }
    }
}

/// `violates` observes a call throwing, which only an in-process call does.
pub(crate) fn check_violates_tier(case: &Case, tier: TestTier, errors: &mut Vec<CompileError>) {
    if tier != TestTier::System {
        return;
    }
    if let Some(path) = violates_claims(&case.body)
        .iter()
        .find_map(|c| c.violates.as_ref())
    {
        let span = path.first().map_or(case.name_span, |i| i.span);
        errors.push(
            CompileError::new(
                "bynk.test.violates_needs_in_process",
                span,
                format!(
                    "case `\"{}\"` claims a refusal with `violates`, but is a `system`-tier case",
                    case.name
                ),
            )
            .with_note(
                "at `system` a refused commit reaches the case as a `500` with no payload, not a throw; claim it at `unit` or `integration`",
            ),
        );
    }
}
