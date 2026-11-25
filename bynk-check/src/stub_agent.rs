//! #4215: stubbing an agent operation at `unit`: `stub <Agent>.<op>(<args>) faulted | unreachable`.
//!
//! An agent call goes through the emitted `__make<Agent>` factory and the agent's registry, never
//! through `deps`, so the clause is not a capability override: it makes every matching call to
//! that operation, whatever the key, end as `Faulted` or `Unreachable(None)` (what a faulting or a
//! busy agent gives `Agent(key).op.outcome(…)`). A plain call to it faults, as such an agent would.
//!
//! It is limited to what the test runtime can intercept: an agent the target context itself
//! declares (other contexts' agents are reached through services, which `stub` already covers),
//! at `unit` (the `integration` and `system` tiers run the real agents), and the two words (a
//! stubbed `returns <value>` would bypass the agent's state and its invariants, which a unit test
//! should reach through the real agent).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use bynk_syntax::ast::{
    AgentDecl, CapabilityDecl, CapabilityOp, ConstDecl, FixtureDecl, FnDecl, StubClause, StubRhs,
    TestTier, TypeDecl,
};
use bynk_syntax::error::CompileError;

use crate::symbols::UnitTable;
use crate::test_suites::{ResolvedStub, StubScope};
use bynk_project::ParsedFile;
use bynk_project::discovery::case_effective_tier;

/// The code every refusal of an agent stub carries.
const CODE: &str = "bynk.stub.agent_stub";

/// The agent `name` the target context declares, if it declares one.
pub(crate) fn target_agent<'a>(
    target: Option<&'a UnitTable>,
    name: &str,
) -> Option<&'a Arc<AgentDecl>> {
    target.and_then(|t| t.agents.get(name))
}

/// A stub that names an agent some other context declares (the target's own agents come first):
/// refused as out of scope, where any other unknown name is `bynk.stub.not_a_seam`.
pub(crate) fn other_context_agent_error(
    clause: &StubClause,
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
) -> Option<CompileError> {
    let name = &clause.capability.name;
    let owner = unit_tables
        .iter()
        .filter(|(unit, _)| unit.as_str() != target_name)
        .find(|(_, t)| t.agents.contains_key(name))
        .map(|(unit, _)| unit)?;
    Some(
        CompileError::new(
            CODE,
            clause.capability.span,
            format!(
                "`{name}` is an agent of `{owner}`, and a `stub` can replace only an agent of the context under test, `{target_name}`"
            ),
        )
        .with_note("another context's agent is reached through its services; `stub` those instead"),
    )
}

/// Fold a `stub <Agent>.<op>(…)` clause into the seam map. The agent is the seam and each
/// stubbed handler one operation of it, typed from the handler, so the clause is checked and
/// emitted as a capability operation is. Refused when the handler is unknown or the clause ends
/// in anything but `faulted` or `unreachable`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn resolve_agent_stub(
    agent: &AgentDecl,
    pc: StubClause,
    scope: StubScope,
    identity_path: PathBuf,
    fixtures: Vec<FixtureDecl>,
    fns: Vec<FnDecl>,
    types: Vec<TypeDecl>,
    consts: Vec<ConstDecl>,
    out: &mut HashMap<String, ResolvedStub>,
    errors: &mut Vec<CompileError>,
) {
    let seam = agent.name.name.clone();
    let handler = agent.handlers.iter().find(|h| {
        h.method_name
            .as_ref()
            .is_some_and(|n| n.name == pc.method.name)
    });
    let Some(handler) = handler else {
        errors.push(
            CompileError::new(
                "bynk.stub.unknown_op",
                pc.method.span,
                format!("`{}` is not a handler of agent `{seam}`", pc.method.name),
            )
            .with_note("a `stub` on an agent names one of its `on call` handlers"),
        );
        return;
    };
    if !matches!(pc.rhs, StubRhs::Faulted(_) | StubRhs::Unreachable(_)) {
        errors.push(
            CompileError::new(
                CODE,
                pc.rhs.span(),
                format!(
                    "`stub {seam}.{}` can end only in `faulted` or `unreachable`, not `{}`",
                    pc.method.name,
                    rhs_word(&pc.rhs)
                ),
            )
            .with_note(
                "a stubbed value would bypass the agent's state and invariants: test those through the real agent",
            ),
        );
        return;
    }
    let op = CapabilityOp {
        name: pc.method.clone(),
        type_params: Vec::new(),
        params: handler.params.clone(),
        return_type: handler.return_type.clone(),
        annotations: Vec::new(),
        documentation: None,
        span: handler.span,
        trivia: Default::default(),
    };
    let entry = out.entry(seam.clone()).or_insert_with(|| ResolvedStub {
        cap: seam.clone(),
        cap_decl: CapabilityDecl {
            name: pc.capability.clone(),
            ops: Vec::new(),
            derived: None,
            documentation: None,
            span: pc.capability.span,
            trailing_comments: Vec::new(),
            trivia: Default::default(),
        },
        service_context: None,
        agent: true,
        clauses: Vec::new(),
        clause_scopes: Vec::new(),
        clause_fixtures: Vec::new(),
        clause_fns: Vec::new(),
        clause_types: Vec::new(),
        clause_consts: Vec::new(),
        identity_path,
    });
    if !entry
        .cap_decl
        .ops
        .iter()
        .any(|o| o.name.name == op.name.name)
    {
        entry.cap_decl.ops.push(op);
    }
    entry.clauses.push(pc);
    entry.clause_scopes.push(scope);
    entry.clause_fixtures.push(fixtures);
    entry.clause_fns.push(fns);
    entry.clause_types.push(types);
    entry.clause_consts.push(consts);
}

/// The word a refused agent-stub clause ends in, as it is written.
fn rhs_word(rhs: &StubRhs) -> &'static str {
    match rhs {
        StubRhs::Returns(_) => "returns",
        StubRhs::Fails(_) => "fails",
        StubRhs::ReturnsEach(..) => "returns each",
        StubRhs::Never(_) => "never",
        StubRhs::Rejected(..) => "rejected",
        StubRhs::Faulted(_) => "faulted",
        StubRhs::Unreachable(_) => "unreachable",
    }
}

/// An agent stub runs at `unit` only: the other tiers run real agents. A clause in a suite stubs
/// every case of it, so a suite-scoped agent stub is refused when any case is `integration`, as
/// a case-scoped one is when its case is. (A `system` case is `bynk.stub.at_system`'s.)
pub(crate) fn refuse_agent_stubs_above_unit(
    target: Option<&UnitTable>,
    indices: &[usize],
    parsed: &[ParsedFile],
    errors: &mut Vec<CompileError>,
) {
    for &i in indices {
        let Some(suite) = parsed[i].test() else {
            continue;
        };
        let integration =
            |c: &bynk_syntax::ast::Case| case_effective_tier(c, suite) == TestTier::Integration;
        if suite.cases.iter().any(integration) {
            let clauses = suite.stubs.iter().map(|s| (s, "suite"));
            refuse(target, clauses, errors);
        }
        for case in suite.cases.iter().filter(|c| integration(c)) {
            refuse(target, case.stubs.iter().map(|s| (s, "case")), errors);
        }
    }
}

/// Report each of `clauses` that stubs an agent of `target`.
fn refuse<'a>(
    target: Option<&UnitTable>,
    clauses: impl Iterator<Item = (&'a StubClause, &'static str)>,
    errors: &mut Vec<CompileError>,
) {
    for (stub, scope) in clauses {
        if target_agent(target, &stub.capability.name).is_none() {
            continue;
        }
        errors.push(
            CompileError::new(
                CODE,
                stub.span,
                format!(
                    "`stub {}.{}` is in an `integration`-tier {scope}, where an agent runs for real",
                    stub.capability.name, stub.method.name
                ),
            )
            .with_note(
                "an agent operation is stubbed at `unit`, where the test runtime stands in for the agent",
            )
            .with_note(
                "move the case into a `unit` suite, or test the agent through its real handlers here",
            ),
        );
    }
}
