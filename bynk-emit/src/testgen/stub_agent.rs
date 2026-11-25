//! #4215: the test-module side of `stub <Agent>.<op>(<args>) faulted | unreachable`.
//!
//! An agent stub reuses the `__Stub_<Agent>` class a capability's stub is (so the argument
//! patterns, scoping and values lower as they do there), but it is not a `deps` entry: an agent
//! call goes through the agent's own factory, never through `deps`. The unit tier's deps factory
//! registers the classes with the runtime (`__stubAgents`), whose bundle-agent wrapper asks the
//! class before it runs an operation. A matched clause throws an error marked with the outcome it stands for,
//! which `.outcome(…)` reads back (`callAgentOutcome`), and a call no clause matches throws a
//! `bynkNoStub` error, which the wrapper takes as "run the real agent".

use super::*;
use bynk_check::test_suites::ResolvedStub;
use bynk_syntax::ast::{StubClause, StubRhs};
use bynk_ts::{TsExpr, TsObjectEntry, TsStmt};

/// `Object.assign(new Error(message), { <name>: <marker> })`: an error carrying a marker the
/// runtime reads.
fn marked_error(message: &str, name: &str, marker: TsExpr) -> TsExpr {
    call(
        member(ident("globalThis.Object"), "assign"),
        vec![
            TsExpr::New {
                callee: Box::new(ident("globalThis.Error")),
                args: vec![str_lit(message)],
            },
            TsExpr::object_entries(vec![TsObjectEntry::prop(name, marker)]),
        ],
    )
}

/// The body of a matched agent stub clause: it throws an error marked with the `CallOutcome` it
/// stands for and the agent it is for. The agent is what lets `callAgentOutcome` tell this
/// agent's own outcome from one a handler of another agent met while it ran.
pub(super) fn agent_stub_rhs(clause: &StubClause, agent: &str) -> Vec<TsStmt> {
    // Resolution admits `faulted` and `unreachable` alone; the rest are never reached.
    let tag = match clause.rhs {
        StubRhs::Unreachable(_) => "Unreachable",
        StubRhs::Faulted(_)
        | StubRhs::Returns(_)
        | StubRhs::Fails(_)
        | StubRhs::ReturnsEach(..)
        | StubRhs::Never(_)
        | StubRhs::Rejected(..) => "Faulted",
    };
    let marker = TsExpr::object_entries(vec![
        TsObjectEntry::prop("tag", str_lit(tag)),
        TsObjectEntry::prop("agent", str_lit(agent)),
    ]);
    vec![TsStmt::throw_stmt(
        marked_error("bynk: stubbed agent call", "bynkCallStub", marker),
        None,
    )]
}

/// What a stub class's method throws when no clause matches the call. A capability's stub is
/// strict about it. An agent's stands aside, and the real operation runs: the clauses stub the
/// calls they name, not the agent.
pub(super) fn no_stub_match_stmt(rp: &ResolvedStub, message: String) -> TsStmt {
    let error = if rp.agent {
        marked_error(&message, "bynkNoStub", bool_lit(true))
    } else {
        new(ident("globalThis.Error"), vec![str_lit(message)])
    };
    TsStmt::throw_stmt(error, None)
}

/// `__stubAgents({ Basket: new __Stub_Basket(__case) });`: the first statement of the unit
/// tier's deps factory, which every case and property builds its `deps` with, after the agent
/// registries are reset (a reset also clears what an earlier case registered). `None` when the
/// target stubs no agent, and for the `integration` tier's factory, which runs the real agents. A class that holds scoped clauses is told the running case, and the
/// running suite when one is scoped to a suite, as `stub_overlay` tells a `deps` entry.
pub(super) fn agent_stub_registration(
    stubs: &HashMap<String, ResolvedStub>,
    factory: &str,
) -> Option<TsStmt> {
    let mut agents: Vec<&ResolvedStub> = stubs.values().filter(|rp| rp.agent).collect();
    if factory != "__makeTestDeps" || agents.is_empty() {
        return None;
    }
    agents.sort_by(|a, b| a.cap.cmp(&b.cap));
    let entries = agents
        .into_iter()
        .map(|rp| {
            let mut args = Vec::new();
            if stub_is_scoped(rp) {
                args.push(ident("__case"));
            }
            if stub_is_suite_scoped(rp) {
                args.push(ident("__suite"));
            }
            TsObjectEntry::prop(
                rp.cap.clone(),
                TsExpr::New {
                    callee: Box::new(ident(stub_class_name(&rp.cap))),
                    args,
                },
            )
        })
        .collect();
    Some(TsStmt::expr_stmt(
        call(ident("__stubAgents"), vec![TsExpr::object_entries(entries)]),
        None,
    ))
}
