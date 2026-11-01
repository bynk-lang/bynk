//! #2195: the agent state model — each agent's state machine, derived from its
//! handlers (agent-state-views track, slice 1; retired-track
//! summary in `design/archive/retired-tracks.md`, §3.1, §3.3, §3.7).
//!
//! The language leaves an agent's machine implicit: a `store` `Cell` of a
//! user sum type (`enum { … }` or pipe form) *is* the machine, `match`/`is`
//! read it and `:=` moves it. This query recovers, for every such field, the
//! edges the handlers can produce — `(from-set, handler, guards, target,
//! outcome, span)` — and the set of states reachable from the initialiser.
//!
//! **A dataflow pass, not path enumeration (§3.3 step 1).** Each handler body
//! is walked over a set of abstract paths. A path carries the states it can
//! have started in (the from-set), the field's *current* value, the last
//! write, its opaque guards, and — once it reaches a tail — its outcome:
//!
//! - Before the first write, a `match <field>` arm or an `is`/`==` test
//!   narrows the from-set. After a known write the same test is decided
//!   statically (a read after a write sees the staged value — ADR 0125 D4),
//!   pruning infeasible branches. After a computed write it narrows nothing
//!   and becomes an opaque guard.
//! - Any other condition is an opaque guard, labelled with its source text.
//! - At each join, paths that agree on from-set, current value, last write
//!   and outcome merge; their guards combine.
//! - The last write on a path wins, since writes commit together at return.
//!   A domain `Err` is a value, so a write before it still makes an edge.
//! - Past [`PATH_BUDGET`] live paths a handler is truncated: its edges become
//!   one [`Target::Unknown`] edge from every state, which keeps reachability
//!   (and slice 3's overlay) sound.
//!
//! Reachability treats an [`Target::Unknown`] edge as reaching every state —
//! a sound over-approximation, so a state outside [`StateMachine::reachable`]
//! is truly unreachable from the handlers.
//!
//! **Placement (§3.7; #2220).** A pure function of an [`AgentDecl`] and the
//! type table, taking no LSP or checker-internal type. It lives in
//! `bynk-check` so the state view (`bynk-ide`, `bynk-lsp`) and a checker
//! diagnostic read one model; `bynk-ide` re-exports it as
//! `bynk_ide::agent_state`.
//!
//! Fault paths (a failing `expect` or `<-`) persist nothing and are not
//! modelled as edges: the walk follows the success path of every statement.
//!
//! Out of scope, as in the sequence diagram (ADR 0259): control flow and
//! writes nested inside an argument expression (`Ok(if c { … } else { … })`,
//! a `field.update(f)` that is not the top of its expression) or a lambda.
//! This costs guard and outcome precision, never edges: a `:=` is a
//! statement, and a `field.update(f)` only runs when it is awaited (`<-`,
//! `do`) or returned as a tail — an `Effect` buried in an argument is a value
//! that never executes.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use bynk_syntax::CompileError;
use bynk_syntax::ast::*;
use bynk_syntax::span::Span;

use crate::ast_text::{branch_outcome, pattern_summary, peel_paren};
use crate::store_write::{store_kinds, store_write};

/// The most live abstract paths one handler may carry before it is truncated
/// (#2195 DECISION C) — a backstop against a pathological handler in a live
/// view, far above anything in the corpus.
pub const PATH_BUDGET: usize = 256;

/// The state machines of one agent — one per state field.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStateModel {
    pub agent: String,
    pub machines: Vec<StateMachine>,
}

/// One state field's machine.
#[derive(Debug, Clone, PartialEq)]
pub struct StateMachine {
    pub field: String,
    pub field_span: Span,
    /// The sum type's variant constructors, in declaration order.
    pub states: Vec<String>,
    /// The initialiser's variant; `None` when it is absent or not a variant
    /// literal, in which case every state is treated as reachable.
    pub initial: Option<String>,
    pub edges: Vec<Edge>,
    /// States reachable from `initial` over `edges` (sound: an `Unknown`
    /// edge reaches every state), in declaration order.
    pub reachable: Vec<String>,
    /// #2219: the states the agent can be in *after a commit*, in declaration
    /// order. A `transition` is checked from the second commit onward (the
    /// genesis commit has no `old`), so only a move from one of these can
    /// fault. Every reachable state but the initial one is here (reaching it
    /// took a committing write); the initial state is here only when some
    /// committing path leaves the agent in it. [`Self::reachable`] still
    /// drives what the view dims.
    pub reachable_after_commit: Vec<String>,
    /// Handlers that exceeded [`PATH_BUDGET`].
    pub truncated: Vec<String>,
    /// #2197: `transition`s that mention this field but fall outside the
    /// evaluated fragment — listed, never guessed.
    pub not_evaluated: Vec<NotEvaluated>,
}

/// One way a handler can leave the field.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub handler: String,
    /// The states this path can start in, in declaration order.
    pub from: Vec<String>,
    /// Conditions on the path that don't constrain the field, as source text.
    pub guards: Vec<String>,
    pub to: Target,
    /// The value the handler yields on this path (`Ok(())`); `None` for unit.
    pub outcome: Option<String>,
    /// The write, or for an unchanged path the tail, for click-to-code.
    pub span: Span,
    /// #2197: the `transition` step invariants this edge can violate.
    pub violations: Vec<Violation>,
    /// #2197: whether this edge's handler commits — the emitter wraps a
    /// handler in the commit that runs `transition`s only when it writes some
    /// store field (`bynk_lower::body_writes_state`); a handler that doesn't
    /// can violate none.
    pub commits: bool,
}

/// #2197: an edge a `transition` would refuse at commit (`InvariantViolation`).
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    /// The `transition`'s name and declaration span.
    pub transition: String,
    pub span: Span,
    pub certainty: Certainty,
    /// The reachable start state from which it violates — one record per
    /// start, so `certainty` and `targets` are that start's own.
    pub from: Vec<String>,
    /// The violating targets from that start: the edge's target, the start
    /// itself for an unchanged path, or for a computed write the targets
    /// that violate.
    pub targets: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Certainty {
    /// Every way the edge can go violates (a known target always does).
    Known,
    /// A computed write may land on a violating target, or on one that isn't.
    Possible,
}

/// #2197: a `transition` on the field outside the evaluated fragment.
#[derive(Debug, Clone, PartialEq)]
pub struct NotEvaluated {
    pub transition: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    State(String),
    /// A computed write (`status := next(x)`, `status.update(f)`).
    Unknown,
    /// No write to the field on this path.
    Unchanged,
}

/// #2221: `bynk.transition.may_violate` — a warning for each move a handler
/// can make that a `transition` refuses at commit (track §3.2–§3.5).
///
/// Only [`Certainty::Known`] violations fire, from start states the agent can
/// be in after a commit (the overlay already filters on
/// [`StateMachine::reachable_after_commit`]); a computed write that only *may*
/// land badly, a truncated handler and a `transition` outside the evaluated
/// fragment raise nothing. The warning sits on the write (or, for an unchanged
/// path, its tail), names the path's guards, and carries the `transition` and
/// one handler that makes the start state reachable as related information.
/// The commit-time check is unchanged.
pub fn may_violate_diagnostics(
    agent: &AgentDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Vec<CompileError> {
    if agent.transitions.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for sm in agent_state_model(agent, types).machines {
        let field = &sm.field;
        for e in &sm.edges {
            if sm.truncated.contains(&e.handler) {
                continue;
            }
            for v in e
                .violations
                .iter()
                .filter(|v| v.certainty == Certainty::Known)
            {
                for start in &v.from {
                    let change = match &e.to {
                        Target::State(t) => format!("move `{field}` from `{start}` to `{t}`"),
                        Target::Unchanged => format!("leave `{field}` at `{start}`"),
                        Target::Unknown => {
                            format!("move `{field}` from `{start}` to a state it refuses")
                        }
                    };
                    let when = if e.guards.is_empty() {
                        String::new()
                    } else {
                        format!(" (when `{}`)", e.guards.join("` and `"))
                    };
                    let mut err = CompileError::new(
                        "bynk.transition.may_violate",
                        e.span,
                        format!(
                            "`{}()` can {change}{when}, which `{}` refuses at commit",
                            e.handler, v.transition
                        ),
                    )
                    .with_label(v.span, format!("`{}` is declared here", v.transition));
                    if let Some((span, label)) = witness(&sm, start) {
                        err = err.with_label(span, label);
                    }
                    out.push(err.with_note(
                        "at run time this call faults with `InvariantViolation`; the \
                         commit-time check is unchanged",
                    ));
                }
            }
        }
    }
    out
}

/// One handler path that leaves the agent in `state` after a commit — why the
/// warning considers `state` reachable. `None` without a literal initialiser,
/// where every state is assumed reachable.
fn witness(sm: &StateMachine, state: &str) -> Option<(Span, String)> {
    let field = &sm.field;
    let initial = sm.initial.as_deref()?;
    // Only a path that can start in a reachable state explains anything: an
    // edge's `from` is what its pattern allows, not what is reachable.
    let live = |e: &Edge| e.from.iter().any(|s| sm.reachable.contains(s));
    let reaches = |e: &&Edge| {
        live(e)
            && match &e.to {
                Target::State(t) => t == state,
                Target::Unchanged => {
                    state == initial && e.commits && e.from.iter().any(|s| s == state)
                }
                Target::Unknown => false,
            }
    };
    if let Some(e) = sm.edges.iter().find(reaches) {
        let label = match &e.to {
            Target::Unchanged => {
                format!(
                    "`{}()` commits and leaves `{field}` at `{state}`",
                    e.handler
                )
            }
            _ => format!("`{}()` moves `{field}` to `{state}`", e.handler),
        };
        return Some((e.span, label));
    }
    sm.edges
        .iter()
        .find(|e| e.to == Target::Unknown && e.commits && live(e))
        .map(|e| {
            (
                e.span,
                format!("`{}()` can move `{field}` to any state", e.handler),
            )
        })
}

/// Derive every state machine of `agent`. `types` is the unit's combined type
/// table (`ContextBoundaryInfo::types`); a state field whose type doesn't
/// resolve to a sum in it is skipped, not guessed (#2195 DECISION A).
pub fn agent_state_model(
    agent: &AgentDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> AgentStateModel {
    let store = store_kinds(agent);
    let machines = agent
        .store_fields
        .iter()
        .filter_map(|f| {
            let states = state_variants(f, types)?;
            Some(machine(agent, f, states, &store))
        })
        .collect();
    AgentStateModel {
        agent: agent.name.name.clone(),
        machines,
    }
}

/// The variant names of `field` when it is a `Cell` of a user sum type.
fn state_variants(
    field: &StoreField,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<Vec<String>> {
    if field.kind.head.name != "Cell" {
        return None;
    }
    let [TypeRef::Named(name)] = field.kind.args.as_slice() else {
        return None;
    };
    let TypeBody::Sum(sum) = &types.get(&name.name)?.body else {
        return None;
    };
    Some(sum.variants.iter().map(|v| v.name.name.clone()).collect())
}

fn machine(
    agent: &AgentDecl,
    field: &StoreField,
    states: Vec<String>,
    store: &HashMap<String, String>,
) -> StateMachine {
    let all: BTreeSet<usize> = (0..states.len()).collect();
    let initial = field
        .init
        .as_ref()
        .and_then(|e| variant_literal(e, &states));
    let mut edges = Vec::new();
    let mut truncated = Vec::new();
    for handler in &agent.handlers {
        let name = handler
            .method_name
            .as_ref()
            .map_or_else(|| "call".to_string(), |m| m.name.clone());
        let commits = handler_commits_in(handler, store);
        let mut w = Walker {
            field: &field.name.name,
            states: &states,
            store,
            bound: handler.params.iter().map(|p| p.name.name.clone()).collect(),
            truncated: false,
        };
        let start = Path {
            from: all.clone(),
            current: Cur::Pre,
            write: None,
            guards: BTreeSet::new(),
            outcome: None,
            outcome_span: None,
        };
        let paths = w.walk_block(&handler.body, vec![start], true);
        if w.truncated {
            truncated.push(name.clone());
            edges.push(Edge {
                handler: name,
                from: states.clone(),
                guards: Vec::new(),
                to: Target::Unknown,
                outcome: None,
                span: handler.span,
                violations: Vec::new(),
                commits,
            });
            continue;
        }
        for p in paths {
            let (to, span) = match p.write {
                Some((Write::Known(i), span)) => (Target::State(states[i].clone()), span),
                Some((Write::Unknown, span)) => (Target::Unknown, span),
                None => (Target::Unchanged, p.outcome_span.unwrap_or(handler.span)),
            };
            edges.push(Edge {
                handler: name.clone(),
                from: p.from.iter().map(|&i| states[i].clone()).collect(),
                guards: p.guards.into_iter().collect(),
                to,
                outcome: p.outcome,
                span,
                violations: Vec::new(),
                commits,
            });
        }
    }
    let reachable = reachable(&states, initial, &edges);
    let reachable_after_commit = reachable_after_commit(&states, initial, &reachable, &edges);
    let not_evaluated = overlay(
        agent,
        &field.name.name,
        &states,
        &reachable_after_commit,
        &mut edges,
    );
    StateMachine {
        field: field.name.name.clone(),
        field_span: field.name.span,
        initial: initial.map(|i| states[i].clone()),
        states,
        edges,
        reachable,
        reachable_after_commit,
        truncated,
        not_evaluated,
    }
}

/// #2197: flag each edge a `transition` would refuse at commit, and list the
/// `transition`s on `field` the fragment can't evaluate (track §3.5).
///
/// - A known target `S → T` violates when the predicate is false for
///   `old = S, new = T`.
/// - An unchanged path is `S → S`, but only in a handler that commits — the
///   emitter wraps a handler in the commit (where transitions are checked)
///   only when it writes some store field (`bynk_lower::body_writes_state`).
/// - A computed write is checked against every target: all violating is
///   [`Certainty::Known`], some is [`Certainty::Possible`].
/// - Only start states the agent can be in after a commit count (#2219): a
///   move only the genesis commit can make is never checked.
fn overlay(
    agent: &AgentDecl,
    field: &str,
    states: &[String],
    after_commit: &[String],
    edges: &mut [Edge],
) -> Vec<NotEvaluated> {
    let index = |name: &str| states.iter().position(|s| s == name);
    let mut not_evaluated = Vec::new();
    for t in &agent.transitions {
        if !mentions_field(&t.predicate, field) {
            continue;
        }
        let Some(pred) = compile(&t.predicate, field, states) else {
            not_evaluated.push(NotEvaluated {
                transition: t.name.name.clone(),
                span: t.span,
            });
            continue;
        };
        for e in edges.iter_mut() {
            // A handler that never commits runs no `transition` at all.
            if !e.commits {
                continue;
            }
            // One record per start state: certainty and targets differ by
            // start (a computed write can be Known from one, Possible from
            // another), and the view reads them per start.
            for start in e.from.iter().filter(|s| after_commit.contains(s)) {
                let Some(si) = index(start) else { continue };
                let candidates: Vec<usize> = match &e.to {
                    Target::State(t) => index(t).into_iter().collect(),
                    Target::Unchanged => vec![si],
                    Target::Unknown => (0..states.len()).collect(),
                };
                let bad: Vec<usize> = candidates
                    .iter()
                    .copied()
                    .filter(|&ti| !pred.eval(si, ti))
                    .collect();
                if bad.is_empty() {
                    continue;
                }
                e.violations.push(Violation {
                    transition: t.name.name.clone(),
                    span: t.span,
                    certainty: if bad.len() == candidates.len() {
                        Certainty::Known
                    } else {
                        Certainty::Possible
                    },
                    from: vec![start.clone()],
                    targets: bad.into_iter().map(|i| states[i].clone()).collect(),
                });
            }
        }
    }
    not_evaluated
}

/// Whether `handler` commits: whether its body writes any of `agent`'s store
/// fields anywhere, the emitter's condition for wrapping it in the commit that
/// checks `transition`s. This recomputes `bynk_lower::body_writes_state`
/// syntactically; `bynk-lower`'s tests pin the two together (#2220).
pub fn handler_commits(agent: &AgentDecl, handler: &Handler) -> bool {
    handler_commits_in(handler, &store_kinds(agent))
}

fn handler_commits_in(handler: &Handler, store: &HashMap<String, String>) -> bool {
    let mut bound: Vec<String> = handler.params.iter().map(|p| p.name.name.clone()).collect();
    block_commits(&handler.body, store, &mut bound)
}

/// Whether a block writes any store field anywhere — see [`handler_commits`].
/// `bound` is the names in scope that shadow a store field: the handler's
/// parameters, and (#2220) every `let`, match-arm and lambda binding on the
/// way down, so a local named like a field is never taken for it — the
/// emitter decides over resolved callees, which can't be fooled that way.
fn block_commits(b: &Block, store: &HashMap<String, String>, bound: &mut Vec<String>) -> bool {
    let scope = bound.len();
    let mut commits = false;
    for s in &b.statements {
        commits |= stmt_commits(s, store, bound);
    }
    commits |= expr_commits(&b.tail, store, bound);
    bound.truncate(scope);
    commits
}

fn stmt_commits(s: &Statement, store: &HashMap<String, String>, bound: &mut Vec<String>) -> bool {
    match s {
        Statement::Assign(_) => true,
        Statement::Let(l) | Statement::EffectLet(l) => {
            let c = expr_commits(&l.value, store, bound);
            bound.push(l.name.name.clone());
            c
        }
        Statement::Expect(a) => expr_commits(&a.value, store, bound),
        Statement::Send(s) => expr_commits(&s.value, store, bound),
        Statement::Do(d) => expr_commits(&d.value, store, bound),
        Statement::Advance(a) => expr_commits(&a.amount, store, bound),
        Statement::Par(p) => p
            .statements
            .iter()
            .fold(false, |c, s| c | stmt_commits(s, store, bound)),
    }
}

fn expr_commits(e: &Expr, store: &HashMap<String, String>, bound: &mut Vec<String>) -> bool {
    if store_write(store, bound, e).is_some() {
        return true;
    }
    match &e.kind {
        ExprKind::Block(b) => block_commits(b, store, bound),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            // Not short-circuited: each branch scopes its own bindings.
            let c = expr_commits(cond, store, bound);
            let t = block_commits(then_block, store, bound);
            let f = block_commits(else_block, store, bound);
            c || t || f
        }
        ExprKind::Match { discriminant, arms } => {
            let mut commits = expr_commits(discriminant, store, bound);
            for a in arms {
                let scope = bound.len();
                bound.extend(a.pattern.bound_names().into_iter().map(|n| n.name.clone()));
                if let Some(g) = &a.guard {
                    commits |= expr_commits(g, store, bound);
                }
                commits |= match &a.body {
                    MatchBody::Expr(e) => expr_commits(e, store, bound),
                    MatchBody::Block(b) => block_commits(b, store, bound),
                };
                bound.truncate(scope);
            }
            commits
        }
        ExprKind::Lambda(l) => {
            let scope = bound.len();
            bound.extend(l.params.iter().map(|p| p.name.name.clone()));
            let commits = expr_commits(&l.body, store, bound);
            bound.truncate(scope);
            commits
        }
        _ => {
            let mut commits = false;
            for c in expr_children(e) {
                commits |= expr_commits(c, store, bound);
            }
            commits
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Old,
    New,
}

/// A `transition` predicate in the evaluated fragment: `is`/`==`/`!=` tests
/// of `old.f`/`new.f` against irrefutable patterns or nullary variants,
/// composed with `!`, `&&`, `||` and `implies`.
enum Pred {
    Test(Side, BTreeSet<usize>),
    Not(Box<Pred>),
    And(Box<Pred>, Box<Pred>),
    Or(Box<Pred>, Box<Pred>),
    Implies(Box<Pred>, Box<Pred>),
}

impl Pred {
    fn eval(&self, old: usize, new: usize) -> bool {
        match self {
            Pred::Test(Side::Old, s) => s.contains(&old),
            Pred::Test(Side::New, s) => s.contains(&new),
            Pred::Not(p) => !p.eval(old, new),
            Pred::And(a, b) => a.eval(old, new) && b.eval(old, new),
            Pred::Or(a, b) => a.eval(old, new) || b.eval(old, new),
            Pred::Implies(a, b) => !a.eval(old, new) || b.eval(old, new),
        }
    }
}

/// `old.<field>` or `new.<field>`.
fn side_of(expr: &Expr, field: &str) -> Option<Side> {
    let ExprKind::FieldAccess { receiver, field: f } = &peel_paren(expr).kind else {
        return None;
    };
    let ExprKind::Ident(r) = &receiver.kind else {
        return None;
    };
    if f.name != field {
        return None;
    }
    match r.name.as_str() {
        "old" => Some(Side::Old),
        "new" => Some(Side::New),
        _ => None,
    }
}

fn mentions_field(expr: &Expr, field: &str) -> bool {
    side_of(expr, field).is_some()
        || expr_children(expr)
            .into_iter()
            .any(|c| mentions_field(c, field))
}

fn compile(expr: &Expr, field: &str, states: &[String]) -> Option<Pred> {
    let inner = peel_paren(expr);
    match &inner.kind {
        ExprKind::UnaryOp(UnaryOp::Not, e) => Some(Pred::Not(Box::new(compile(e, field, states)?))),
        ExprKind::BinOp(op @ (BinOp::And | BinOp::Or | BinOp::Implies), l, r) => {
            let (a, b) = (
                Box::new(compile(l, field, states)?),
                Box::new(compile(r, field, states)?),
            );
            Some(match op {
                BinOp::And => Pred::And(a, b),
                BinOp::Or => Pred::Or(a, b),
                _ => Pred::Implies(a, b),
            })
        }
        ExprKind::Is { value, pattern } => {
            let side = side_of(value, field)?;
            let m = pattern_sets(pattern, states);
            // A refutable pattern can't be decided on constructors alone.
            (m.may == m.must).then_some(Pred::Test(side, m.must))
        }
        ExprKind::BinOp(op @ (BinOp::Eq | BinOp::NotEq), l, r) => {
            let (side, other) = match (side_of(l, field), side_of(r, field)) {
                (Some(s), None) => (s, r),
                (None, Some(s)) => (s, l),
                _ => return None,
            };
            if has_payload(other) {
                return None;
            }
            let test = Pred::Test(side, BTreeSet::from([variant_literal(other, states)?]));
            Some(if *op == BinOp::Eq {
                test
            } else {
                Pred::Not(Box::new(test))
            })
        }
        _ => None,
    }
}

/// States reachable from `initial` (every state when there is none), with an
/// `Unknown` edge reaching every state.
fn reachable(states: &[String], initial: Option<usize>, edges: &[Edge]) -> Vec<String> {
    let index = |name: &str| states.iter().position(|s| s == name);
    let mut seen: BTreeSet<usize> = match initial {
        Some(i) => BTreeSet::from([i]),
        None => (0..states.len()).collect(),
    };
    loop {
        let before = seen.len();
        for e in edges {
            if !e
                .from
                .iter()
                .filter_map(|s| index(s))
                .any(|i| seen.contains(&i))
            {
                continue;
            }
            match &e.to {
                Target::State(s) => seen.extend(index(s)),
                Target::Unknown => seen.extend(0..states.len()),
                Target::Unchanged => {}
            }
        }
        if seen.len() == before {
            break;
        }
    }
    seen.into_iter().map(|i| states[i].clone()).collect()
}

/// #2219: the states the agent can be in after a commit (see
/// [`StateMachine::reachable_after_commit`]). Without a literal initialiser
/// every state counts, as for [`reachable`].
fn reachable_after_commit(
    states: &[String],
    initial: Option<usize>,
    reachable: &[String],
    edges: &[Edge],
) -> Vec<String> {
    let Some(init) = initial else {
        return reachable.to_vec();
    };
    let initial = &states[init];
    // Back in the initial state after a commit: a committing path from a
    // reachable start that ends there — an unchanged path from it, a move
    // into it, or a computed write (which can land anywhere).
    let returns = edges.iter().any(|e| {
        e.commits
            && e.from.iter().any(|s| {
                reachable.contains(s)
                    && match &e.to {
                        Target::State(t) => t == initial,
                        Target::Unchanged => s == initial,
                        Target::Unknown => true,
                    }
            })
    });
    reachable
        .iter()
        .filter(|s| *s != initial || returns)
        .cloned()
        .collect()
}

/// A variant literal of the state type: `Placed`, `Square(1)`,
/// `OrderStatus.Placed` or `OrderStatus.Square(1)`.
/// Whether a variant literal carries a payload (`Square(1)`), so comparing
/// against it pins the value, not just the constructor.
fn has_payload(expr: &Expr) -> bool {
    match &peel_paren(expr).kind {
        ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => !args.is_empty(),
        _ => false,
    }
}

/// What a pattern says about the field's constructor (see
/// [`Walker::pattern_states`]).
#[derive(Default)]
struct PatternStates {
    may: BTreeSet<usize>,
    must: BTreeSet<usize>,
}

fn variant_literal(expr: &Expr, states: &[String]) -> Option<usize> {
    let name = match &peel_paren(expr).kind {
        ExprKind::Ident(id) => &id.name,
        ExprKind::Call { name, .. } => &name.name,
        ExprKind::FieldAccess { field, .. } => &field.name,
        ExprKind::MethodCall { method, .. } => &method.name,
        _ => return None,
    };
    states.iter().position(|s| s == name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Cur {
    /// Not yet written on this path: the field holds the pre-state.
    Pre,
    Known(usize),
    /// Written with a computed value.
    Computed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Write {
    Known(usize),
    Unknown,
}

#[derive(Debug, Clone)]
struct Path {
    from: BTreeSet<usize>,
    current: Cur,
    write: Option<(Write, Span)>,
    guards: BTreeSet<String>,
    outcome: Option<String>,
    outcome_span: Option<Span>,
}

impl Path {
    /// Paths with the same key produce the same edge, guards aside.
    fn same_key(&self, other: &Path) -> bool {
        self.from == other.from
            && self.current == other.current
            && self.write.map(|w| w.0) == other.write.map(|w| w.0)
            && self.outcome == other.outcome
    }
}

struct Walker<'a> {
    field: &'a str,
    states: &'a [String],
    store: &'a HashMap<String, String>,
    /// Names bound in scope; a binding named like the field shadows it.
    bound: Vec<String>,
    truncated: bool,
}

impl Walker<'_> {
    fn walk_block(&mut self, block: &Block, mut paths: Vec<Path>, tail: bool) -> Vec<Path> {
        let scope = self.bound.len();
        for stmt in &block.statements {
            if self.truncated {
                return Vec::new();
            }
            paths = self.walk_stmt(stmt, paths);
        }
        let paths = self.walk_expr(&block.tail, paths, tail);
        self.bound.truncate(scope);
        paths
    }

    fn walk_stmt(&mut self, stmt: &Statement, paths: Vec<Path>) -> Vec<Path> {
        match stmt {
            Statement::Let(l) | Statement::EffectLet(l) => {
                let paths = self.walk_expr(&l.value, paths, false);
                self.bound.push(l.name.name.clone());
                paths
            }
            Statement::Do(d) => self.walk_expr(&d.value, paths, false),
            Statement::Expect(e) => self.walk_expr(&e.value, paths, false),
            Statement::Send(s) => self.walk_expr(&s.value, paths, false),
            Statement::Advance(a) => self.walk_expr(&a.amount, paths, false),
            Statement::Par(p) => p
                .statements
                .iter()
                .fold(paths, |paths, s| self.walk_stmt(s, paths)),
            Statement::Assign(a) => {
                let mut paths = self.walk_expr(&a.value, paths, false);
                // No shadowing check: `:=` only ever targets a `store Cell`
                // (`bynk.cell.invalid_target`), never a local.
                if a.target.name == self.field {
                    let write = match variant_literal(&a.value, self.states) {
                        Some(i) => Write::Known(i),
                        None => Write::Unknown,
                    };
                    for p in &mut paths {
                        Self::apply_write(p, write, a.span);
                    }
                }
                paths
            }
        }
    }

    fn apply_write(p: &mut Path, write: Write, span: Span) {
        p.current = match write {
            Write::Known(i) => Cur::Known(i),
            Write::Unknown => Cur::Computed,
        };
        p.write = Some((write, span));
    }

    fn walk_expr(&mut self, expr: &Expr, mut paths: Vec<Path>, tail: bool) -> Vec<Path> {
        if self.truncated || paths.is_empty() {
            return paths;
        }
        let inner = peel_paren(expr);
        match &inner.kind {
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                let then_paths = self.narrow(paths.clone(), cond, true);
                let else_paths = self.narrow(paths, cond, false);
                let mut out = self.walk_block(then_block, then_paths, tail);
                out.extend(self.walk_block(else_block, else_paths, tail));
                self.join(out)
            }
            ExprKind::Match { discriminant, arms } => {
                self.walk_match(discriminant, arms, paths, tail)
            }
            ExprKind::Block(b) => self.walk_block(b, paths, tail),
            _ => {
                if let Some(w) = store_write(self.store, &self.bound, inner) {
                    // `field.update(f)` on the state field: a computed write.
                    if w.field.name == self.field {
                        for p in &mut paths {
                            Self::apply_write(p, Write::Unknown, inner.span);
                        }
                    }
                    if tail {
                        // A store write yields `()`, so it carries no outcome.
                        for p in &mut paths {
                            p.outcome = None;
                            p.outcome_span = Some(inner.span);
                        }
                    }
                } else if tail {
                    let outcome = branch_outcome(inner);
                    for p in &mut paths {
                        p.outcome = outcome.clone();
                        p.outcome_span = Some(inner.span);
                    }
                }
                paths
            }
        }
    }

    fn walk_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[MatchArm],
        paths: Vec<Path>,
        tail: bool,
    ) -> Vec<Path> {
        let on_field = self.is_field(scrutinee);
        // States taken by earlier unguarded arms — an arm only sees the rest.
        let mut covered = BTreeSet::new();
        let mut out = Vec::new();
        for arm in arms {
            let mut arm_paths = if on_field {
                let m = self.pattern_states(&arm.pattern);
                let states: BTreeSet<usize> = m.may.difference(&covered).copied().collect();
                // Only a constructor the arm matches whatever its payload is
                // taken from later arms: `Square(1) =>` leaves `Square(n) =>`
                // its share.
                if arm.guard.is_none() {
                    covered.extend(m.must.iter().copied());
                }
                self.constrain(paths.clone(), &states)
            } else {
                let label = format!(
                    "{} is {}",
                    bynk_fmt::expr_to_string(scrutinee),
                    pattern_summary(&arm.pattern)
                );
                Self::guarded(paths.clone(), label)
            };
            let scope = self.bound.len();
            self.bound.extend(
                arm.pattern
                    .bound_names()
                    .into_iter()
                    .map(|n| n.name.clone()),
            );
            if let Some(guard) = &arm.guard {
                arm_paths = self.narrow(arm_paths, guard, true);
            }
            let arm_out = match &arm.body {
                MatchBody::Expr(e) => self.walk_expr(e, arm_paths, tail),
                MatchBody::Block(b) => self.walk_block(b, arm_paths, tail),
            };
            self.bound.truncate(scope);
            out.extend(arm_out);
        }
        self.join(out)
    }

    /// Keep the paths consistent with `cond` evaluating to `truth`.
    fn narrow(&mut self, paths: Vec<Path>, cond: &Expr, truth: bool) -> Vec<Path> {
        match self.facts(cond, truth) {
            Some(allowed) => self.constrain(paths, &allowed),
            None => {
                let text = bynk_fmt::expr_to_string(cond);
                let label = if truth { text } else { format!("!({text})") };
                Self::guarded(paths, label)
            }
        }
    }

    /// Constrain each path's field to `allowed`: narrow the from-set before a
    /// write, decide statically after a known write, or record a guard after a
    /// computed one.
    fn constrain(&self, paths: Vec<Path>, allowed: &BTreeSet<usize>) -> Vec<Path> {
        paths
            .into_iter()
            .filter_map(|mut p| match p.current {
                Cur::Pre => {
                    p.from = p.from.intersection(allowed).copied().collect();
                    (!p.from.is_empty()).then_some(p)
                }
                Cur::Known(i) => allowed.contains(&i).then_some(p),
                Cur::Computed => {
                    let names: Vec<&str> =
                        allowed.iter().map(|&i| self.states[i].as_str()).collect();
                    p.guards
                        .insert(format!("{} is {}", self.field, names.join(" | ")));
                    Some(p)
                }
            })
            .collect()
    }

    fn guarded(mut paths: Vec<Path>, label: String) -> Vec<Path> {
        for p in &mut paths {
            p.guards.insert(label.clone());
        }
        paths
    }

    /// What `cond == truth` says about the field, when it says anything a
    /// state set can express: `is`, `==`/`!=` against a variant, `!`, and
    /// `&&`/`||` whose sides both speak about the field.
    fn facts(&self, cond: &Expr, truth: bool) -> Option<BTreeSet<usize>> {
        let all = || (0..self.states.len()).collect::<BTreeSet<usize>>();
        let complement = |s: BTreeSet<usize>| all().difference(&s).copied().collect();
        let inner = peel_paren(cond);
        match &inner.kind {
            // True: the field may be any constructor the pattern can match.
            // False: only a constructor it matches irrefutably is excluded —
            // `shape is Square(1)` false still allows `Square(2)`.
            ExprKind::Is { value, pattern } if self.is_field(value) => {
                let m = self.pattern_states(pattern);
                Some(if truth { m.may } else { complement(m.must) })
            }
            ExprKind::BinOp(op @ (BinOp::Eq | BinOp::NotEq), l, r) => {
                let other = if self.is_field(l) {
                    r
                } else if self.is_field(r) {
                    l
                } else {
                    return None;
                };
                let i = variant_literal(other, self.states)?;
                let s = BTreeSet::from([i]);
                // Equal to a nullary variant pins the constructor both ways;
                // unequal to `Square(1)` still allows `Square(2)`.
                let pinned = !has_payload(other);
                Some(if truth == (*op == BinOp::Eq) {
                    s
                } else if pinned {
                    complement(s)
                } else {
                    all()
                })
            }
            ExprKind::UnaryOp(UnaryOp::Not, e) => self.facts(e, !truth),
            ExprKind::BinOp(op @ (BinOp::And | BinOp::Or), l, r) => {
                let a = self.facts(l, truth)?;
                let b = self.facts(r, truth)?;
                // `a && b` true, or `a || b` false, needs both sides to hold.
                Some(if (*op == BinOp::And) == truth {
                    a.intersection(&b).copied().collect()
                } else {
                    a.union(&b).copied().collect()
                })
            }
            _ => None,
        }
    }

    fn pattern_states(&self, pattern: &Pattern) -> PatternStates {
        pattern_sets(pattern, self.states)
    }

    fn is_field(&self, expr: &Expr) -> bool {
        matches!(&peel_paren(expr).kind, ExprKind::Ident(id)
            if id.name == self.field && !self.bound.contains(&id.name))
    }

    /// Merge paths that would produce the same edge, combining their guards,
    /// and enforce the budget.
    fn join(&mut self, paths: Vec<Path>) -> Vec<Path> {
        let mut out: Vec<Path> = Vec::new();
        for p in paths {
            match out.iter_mut().find(|q| q.same_key(&p)) {
                Some(q) => q.guards = combine_guards(&q.guards, &p.guards),
                None => out.push(p),
            }
        }
        if out.len() > PATH_BUDGET {
            self.truncated = true;
            return Vec::new();
        }
        out
    }
}

/// The guards of two merged paths: either side holding is enough, so an
/// unconditional side makes the merge unconditional, and two different
/// conjunctions become one disjunction label.
pub fn combine_guards(a: &BTreeSet<String>, b: &BTreeSet<String>) -> BTreeSet<String> {
    if a == b {
        return a.clone();
    }
    if a.is_empty() || b.is_empty() {
        return BTreeSet::new();
    }
    // `c` on one side and `!(c)` on the other: together, no condition.
    if let ([x], [y]) = (
        a.iter().collect::<Vec<_>>().as_slice(),
        b.iter().collect::<Vec<_>>().as_slice(),
    ) && (**y == format!("!({x})") || **x == format!("!({y})"))
    {
        return BTreeSet::new();
    }
    let side = |s: &BTreeSet<String>| {
        let parts: Vec<&str> = s.iter().map(String::as_str).collect();
        if parts.len() == 1 {
            parts[0].to_string()
        } else {
            format!("({})", parts.join(" && "))
        }
    };
    BTreeSet::from([format!("{} || {}", side(a), side(b))])
}

/// The constructors a pattern may match, and those it matches whatever
/// the payload: `Square(n)` must-matches `Square`, `Square(1)` only
/// may-match it, and a refined `_ where p` may match anything but must
/// match nothing.
fn pattern_sets(pattern: &Pattern, states: &[String]) -> PatternStates {
    let all = || (0..states.len()).collect::<BTreeSet<usize>>();
    match pattern {
        Pattern::Wildcard(_) | Pattern::Binding(_) => PatternStates {
            may: all(),
            must: all(),
        },
        Pattern::Variant {
            variant, bindings, ..
        } => {
            let may: BTreeSet<usize> = states
                .iter()
                .position(|s| *s == variant.name)
                .into_iter()
                .collect();
            let irrefutable = bindings
                .iter()
                .all(|b| matches!(b.pattern(), Pattern::Wildcard(_) | Pattern::Binding(_)));
            let must = if irrefutable {
                may.clone()
            } else {
                BTreeSet::new()
            };
            PatternStates { may, must }
        }
        Pattern::Refined { inner, .. } => PatternStates {
            may: pattern_sets(inner, states).may,
            must: BTreeSet::new(),
        },
        Pattern::Or(alts, _) => {
            let mut out = PatternStates::default();
            for a in alts {
                let m = pattern_sets(a, states);
                out.may.extend(m.may);
                out.must.extend(m.must);
            }
            out
        }
        // A record or a list never matches a state sum.
        Pattern::Literal { .. } | Pattern::Record { .. } | Pattern::List { .. } => {
            PatternStates::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a single-file context (parse only — the state model reads the
    /// AST) and derive the model of `agent`, with the type table built from
    /// the file's own `type` declarations. Sources are inline copies of the
    /// compiler's state-machine fixtures: `bynk-ide` tests stay self-contained.
    fn model(src: &str, agent: &str) -> AgentStateModel {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("tokenize");
        let (unit, errs) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, src);
        assert!(errs.is_empty(), "parse errors: {errs:?}");
        let Some(SourceUnit::Context(ctx)) = unit else {
            panic!("expected a context")
        };
        let types = ctx
            .items
            .iter()
            .filter_map(|i| match i {
                CommonsItem::Type(t) => Some((t.name.name.clone(), Arc::new(t.clone()))),
                _ => None,
            })
            .collect();
        let a = ctx
            .items
            .iter()
            .find_map(|i| match i {
                CommonsItem::Agent(a) if a.name.name == agent => Some(a),
                _ => None,
            })
            .expect("agent");
        agent_state_model(a, &types)
    }

    /// #2221: the `bynk.transition.may_violate` warnings for `agent`, as
    /// `(message, labels)`.
    fn warnings(src: &str, agent: &str) -> Vec<(String, Vec<String>)> {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("tokenize");
        let (unit, errs) = bynk_syntax::parser::parse_unit_with_recovery(&tokens, src);
        assert!(errs.is_empty(), "parse errors: {errs:?}");
        let Some(SourceUnit::Context(ctx)) = unit else {
            panic!("expected a context")
        };
        let types = ctx
            .items
            .iter()
            .filter_map(|i| match i {
                CommonsItem::Type(t) => Some((t.name.name.clone(), Arc::new(t.clone()))),
                _ => None,
            })
            .collect();
        let a = ctx
            .items
            .iter()
            .find_map(|i| match i {
                CommonsItem::Agent(a) if a.name.name == agent => Some(a),
                _ => None,
            })
            .expect("agent");
        may_violate_diagnostics(a, &types)
            .into_iter()
            .map(|e| {
                assert_eq!(e.category, "bynk.transition.may_violate");
                (e.message, e.labels.into_iter().map(|(_, l)| l).collect())
            })
            .collect()
    }

    fn only(m: &AgentStateModel) -> &StateMachine {
        assert_eq!(m.machines.len(), 1, "one state field");
        &m.machines[0]
    }

    /// `(handler, from, to, outcome)` per edge, for compact assertions.
    fn edges(sm: &StateMachine) -> Vec<(String, Vec<String>, Target, Option<String>)> {
        sm.edges
            .iter()
            .map(|e| {
                (
                    e.handler.clone(),
                    e.from.clone(),
                    e.to.clone(),
                    e.outcome.clone(),
                )
            })
            .collect()
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn to(state: &str) -> Target {
        Target::State(state.to_string())
    }

    fn out(o: &str) -> Option<String> {
        Some(o.to_string())
    }

    // 159_order_lifecycle
    const ORDER: &str = r#"context orders

type OrderStatus = enum { Pending, Placed, Cancelled }
type OrderError = enum { AlreadyPlaced, AlreadyCancelled }

agent Order {
  key id: String

  store status: Cell[OrderStatus] = Pending
  store items: Cell[Int]

  on call addItem(quantity: Int) -> Effect[Result[(), OrderError]] {
    let cur = items
    items := cur + quantity
    Ok(())
  }

  on call place() -> Effect[Result[(), OrderError]] {
    match status {
      Pending => {
        status := Placed
        Ok(())
      }
      Placed    => Err(AlreadyPlaced)
      Cancelled => Err(AlreadyCancelled)
    }
  }
}
"#;

    #[test]
    fn the_order_lifecycle_moves_pending_to_placed_and_refuses_the_rest() {
        let m = model(ORDER, "Order");
        let sm = only(&m);
        assert_eq!(sm.field, "status");
        assert_eq!(sm.states, s(&["Pending", "Placed", "Cancelled"]));
        assert_eq!(sm.initial.as_deref(), Some("Pending"));
        assert_eq!(
            edges(sm),
            [
                // `addItem` never touches `status`: unchanged from every state.
                (
                    "addItem".into(),
                    s(&["Pending", "Placed", "Cancelled"]),
                    Target::Unchanged,
                    out("Ok(())")
                ),
                ("place".into(), s(&["Pending"]), to("Placed"), out("Ok(())")),
                (
                    "place".into(),
                    s(&["Placed"]),
                    Target::Unchanged,
                    out("Err(AlreadyPlaced)")
                ),
                (
                    "place".into(),
                    s(&["Cancelled"]),
                    Target::Unchanged,
                    out("Err(AlreadyCancelled)")
                ),
            ]
        );
        // Nothing moves to `Cancelled`, so it is unreachable from `Pending`.
        assert_eq!(sm.reachable, s(&["Pending", "Placed"]));
        assert!(sm.truncated.is_empty());
        // The edge's span is the write, for click-to-code.
        let write_at = ORDER.find("status := Placed").unwrap();
        assert_eq!(sm.edges[1].span.start, write_at);
    }

    // 245_transition_step_invariant
    const PAID: &str = r#"context commerce.order

type AuthId = String where NonEmpty
type OrderStatus = enum { Pending, Placed, Paid }

agent Order {
  key id: String

  store status: Cell[OrderStatus] = Pending
  store paymentRef: Cell[Option[AuthId]]

  invariant paid_has_payment_ref:
    status == Paid implies paymentRef.isSome()

  transition paid_is_terminal:
    old.status is Paid implies new.status is Paid

  on call place() -> Effect[()] {
    status := Placed
    Effect.pure(())
  }

  on call pay(ref: AuthId) -> Effect[()] {
    status := Paid
    paymentRef := Some(ref)
    Effect.pure(())
  }
}
"#;

    #[test]
    fn an_unguarded_write_is_an_edge_from_every_state_including_paid() {
        let m = model(PAID, "Order");
        let sm = only(&m);
        let all = s(&["Pending", "Placed", "Paid"]);
        assert_eq!(
            edges(sm),
            [
                (
                    "place".into(),
                    all.clone(),
                    to("Placed"),
                    out("Effect.pure(())")
                ),
                ("pay".into(), all, to("Paid"), out("Effect.pure(())")),
            ]
        );
        assert_eq!(sm.reachable, s(&["Pending", "Placed", "Paid"]));
    }

    // 156_state_transition and 2018_history_transition_near_miss
    #[test]
    fn a_write_in_tail_or_single_statement_position_is_still_the_write() {
        let turnstile = r#"context turnstile
type Gate = enum { Locked, Unlocked }
agent Turnstile {
  key id: String
  store gate: Cell[Gate] = Locked
  on call unlock() -> Effect[Result[(), String]] {
    gate := Unlocked
    Ok(())
  }
}
"#;
        let m = model(turnstile, "Turnstile");
        assert_eq!(
            edges(only(&m)),
            [(
                "unlock".into(),
                s(&["Locked", "Unlocked"]),
                to("Unlocked"),
                out("Ok(())")
            )]
        );
        let door = r#"context demo.door
type Status = enum { Open, Closed }
agent Door {
  key id: String
  store status: Cell[Status] = Open
  transition closed_is_final: old.status is Closed implies new.status is Closed
  on call close() -> Effect[()] { status := Closed }
}
"#;
        let m = model(door, "Door");
        assert_eq!(
            edges(only(&m)),
            [("close".into(), s(&["Open", "Closed"]), to("Closed"), None)]
        );
    }

    // 155_state_sum_machine
    #[test]
    fn a_read_only_handler_is_unchanged_and_a_comparison_is_not_a_write() {
        let src = r#"context signal
type Light = enum { Red, Green }
agent Signal {
  key id: String
  store colour: Cell[Light] = Red
  on call peek() -> Effect[Result[(), String]] {
    Ok(())
  }
  on call turnGreen() -> Effect[()] {
    colour := Green
  }
  on call isGreen() -> Effect[Bool] {
    colour == Green
  }
}
"#;
        let m = model(src, "Signal");
        let sm = only(&m);
        let both = s(&["Red", "Green"]);
        assert_eq!(
            edges(sm),
            [
                (
                    "peek".into(),
                    both.clone(),
                    Target::Unchanged,
                    out("Ok(())")
                ),
                ("turnGreen".into(), both.clone(), to("Green"), None),
                (
                    "isGreen".into(),
                    both,
                    Target::Unchanged,
                    out("colour == Green")
                ),
            ]
        );
        assert_eq!(sm.reachable, s(&["Red", "Green"]));
    }

    // 1649_agent_state_round_trip: a sum with payloads, and computed writes.
    #[test]
    fn payloads_abstract_to_constructors_and_a_computed_write_is_unknown() {
        let src = r#"context demo
type Shape =
  | Circle(r: Int)
  | Square(side: Int)
agent Figure {
  key id: String
  store shape: Cell[Shape] = Square(1)
  on call set(s: Shape) -> Effect[()] { shape := s }
  on call circle(r: Int) -> Effect[()] { shape := Circle(r) }
  on call get() -> Effect[Shape] { shape }
}
"#;
        let m = model(src, "Figure");
        let sm = only(&m);
        assert_eq!(sm.states, s(&["Circle", "Square"]));
        assert_eq!(sm.initial.as_deref(), Some("Square"));
        let both = s(&["Circle", "Square"]);
        assert_eq!(
            edges(sm),
            [
                ("set".into(), both.clone(), Target::Unknown, None),
                ("circle".into(), both.clone(), to("Circle"), None),
                ("get".into(), both, Target::Unchanged, out("shape")),
            ]
        );
    }

    #[test]
    fn option_result_records_and_scalars_are_not_state_fields() {
        let src = r#"context demo
type Light = enum { Red, Green }
type Rec = { n: Int }
agent Many {
  key id: String
  store n: Cell[Option[Int]]
  store r: Cell[Result[Int, String]] = Err("unset")
  store rec: Cell[Rec] = Rec { n: 0 }
  store count: Cell[Int]
  store lights: Map[String, Light]
  store light: Cell[Light] = Red
  on call get() -> Effect[Int] { count }
}
"#;
        let m = model(src, "Many");
        assert_eq!(only(&m).field, "light");
    }

    #[test]
    fn a_field_whose_type_does_not_resolve_is_skipped() {
        // `Elsewhere` is not in this unit's type table.
        let src = r#"context demo
agent A {
  key id: String
  store s: Cell[Elsewhere]
  on call get() -> Effect[()] { () }
}
"#;
        assert!(model(src, "A").machines.is_empty());
    }

    const LIFECYCLE: &str = r#"context demo
type Status = enum { Draft, Live, Closed }
type DocError = enum { NotDraft, Invalid }
agent Doc {
  key id: String
  store status: Cell[Status] = Draft
  store attempts: Cell[Int]

  // A domain `Err` after a write still commits the write.
  on call publish(ok: Bool) -> Effect[Result[(), DocError]] {
    if status is Draft {
      status := Live
      if ok { Ok(()) } else { Err(Invalid) }
    } else {
      Err(NotDraft)
    }
  }

  // A read after a write sees the write: the second `if` is decided.
  on call reopen() -> Effect[Bool] {
    status := Draft
    if status == Draft { true } else { false }
  }

  // `update` on the state field is a computed write; a later test on the
  // field narrows nothing and becomes a guard.
  on call cycle() -> Effect[Int] {
    let _ <- status.update((s) => s)
    if status is Closed { 0 } else { 1 }
  }

  // A guard that isn't about the field is kept as text.
  on call close(force: Bool) -> Effect[()] {
    if force && attempts > 3 { status := Closed } else { () }
  }
}
"#;

    #[test]
    fn a_domain_err_after_a_write_is_still_an_edge() {
        let m = model(LIFECYCLE, "Doc");
        let publish: Vec<_> = edges(only(&m))
            .into_iter()
            .filter(|e| e.0 == "publish")
            .collect();
        assert_eq!(
            publish,
            [
                ("publish".into(), s(&["Draft"]), to("Live"), out("Ok(())")),
                (
                    "publish".into(),
                    s(&["Draft"]),
                    to("Live"),
                    out("Err(Invalid)")
                ),
                (
                    "publish".into(),
                    s(&["Live", "Closed"]),
                    Target::Unchanged,
                    out("Err(NotDraft)")
                ),
            ]
        );
        let guarded: Vec<_> = only(&m)
            .edges
            .iter()
            .filter(|e| e.handler == "publish")
            .map(|e| e.guards.clone())
            .collect();
        assert_eq!(guarded, [s(&["ok"]), s(&["!(ok)"]), vec![]]);
    }

    #[test]
    fn a_read_after_a_write_is_decided_not_narrowed() {
        let m = model(LIFECYCLE, "Doc");
        let reopen: Vec<_> = only(&m)
            .edges
            .iter()
            .filter(|e| e.handler == "reopen")
            .collect();
        // One path: the `else` is infeasible after `status := Draft`, and the
        // from-set is not narrowed by the read.
        assert_eq!(reopen.len(), 1);
        assert_eq!(reopen[0].from, s(&["Draft", "Live", "Closed"]));
        assert_eq!(reopen[0].to, to("Draft"));
        assert_eq!(reopen[0].outcome.as_deref(), Some("true"));
        assert!(reopen[0].guards.is_empty());
    }

    #[test]
    fn a_cell_update_on_the_state_field_is_unknown_and_later_tests_are_guards() {
        let m = model(LIFECYCLE, "Doc");
        let cycle: Vec<_> = only(&m)
            .edges
            .iter()
            .filter(|e| e.handler == "cycle")
            .collect();
        assert_eq!(cycle.len(), 2);
        assert!(cycle.iter().all(|e| e.to == Target::Unknown));
        assert!(
            cycle
                .iter()
                .all(|e| e.from == s(&["Draft", "Live", "Closed"]))
        );
        assert_eq!(cycle[0].guards, s(&["status is Closed"]));
        assert_eq!(cycle[1].guards, s(&["status is Draft | Live"]));
        // An `Unknown` edge reaches every state.
        assert_eq!(only(&m).reachable, s(&["Draft", "Live", "Closed"]));
    }

    #[test]
    fn an_opaque_guard_is_kept_as_source_text() {
        let m = model(LIFECYCLE, "Doc");
        let close: Vec<_> = only(&m)
            .edges
            .iter()
            .filter(|e| e.handler == "close")
            .map(|e| (e.to.clone(), e.guards.clone()))
            .collect();
        assert_eq!(
            close,
            [
                (to("Closed"), s(&["force && attempts > 3"])),
                (Target::Unchanged, s(&["!(force && attempts > 3)"])),
            ]
        );
    }

    #[test]
    fn paths_that_agree_merge_at_the_join_and_combine_guards() {
        let src = r#"context demo
type S = enum { A, B }
agent M {
  key id: String
  store s: Cell[S] = A
  on call go(x: Bool, y: Bool) -> Effect[()] {
    if x { s := B } else { if y { s := B } else { () } }
  }
}
"#;
        let m = model(src, "M");
        let sm = only(&m);
        // Both `s := B` paths merge into one edge whose guard is either side.
        assert_eq!(sm.edges.len(), 2);
        assert_eq!(sm.edges[0].to, to("B"));
        assert_eq!(sm.edges[0].guards, s(&["x || (!(x) && y)"]));
        assert_eq!(sm.edges[1].to, Target::Unchanged);
    }

    #[test]
    fn a_condition_and_its_negation_merge_to_no_guard() {
        let src = r#"context demo
type S = enum { A, B }
agent M {
  key id: String
  store s: Cell[S] = A
  on call go(x: Bool) -> Effect[()] {
    if x { s := B } else { s := B }
  }
}
"#;
        let m = model(src, "M");
        let sm = only(&m);
        // Both branches reach `B`: one edge, with no condition rather than
        // `x || !(x)`.
        assert_eq!(sm.edges.len(), 1);
        assert_eq!(sm.edges[0].to, to("B"));
        assert!(sm.edges[0].guards.is_empty(), "{:?}", sm.edges[0].guards);
    }

    #[test]
    fn a_shadowed_field_name_is_not_the_field() {
        let src = r#"context demo
type S = enum { A, B }
agent M {
  key id: String
  store s: Cell[S] = A
  on call go(s: S) -> Effect[Int] {
    match s {
      A => 1
      B => 2
    }
  }
}
"#;
        let m = model(src, "M");
        let sm = only(&m);
        // The parameter `s` is matched, not the field: both arms keep the
        // full from-set and record the match as a guard.
        assert!(sm.edges.iter().all(|e| e.from == s(&["A", "B"])));
        assert_eq!(sm.edges[0].guards, s(&["s is A"]));
    }

    #[test]
    fn a_wildcard_arm_takes_only_the_states_earlier_arms_left() {
        let src = r#"context demo
type S = enum { A, B, C }
agent M {
  key id: String
  store s: Cell[S] = A
  on call go() -> Effect[Int] {
    match s {
      A => 1
      _ => 2
    }
  }
}
"#;
        let m = model(src, "M");
        let from: Vec<_> = only(&m).edges.iter().map(|e| e.from.clone()).collect();
        assert_eq!(from, [s(&["A"]), s(&["B", "C"])]);
    }

    #[test]
    fn a_handler_past_the_budget_is_truncated_to_an_unknown_edge_from_every_state() {
        let arms: String = (0..=PATH_BUDGET)
            .map(|i| format!("      {i} => {i}\n"))
            .collect();
        let src = format!(
            "context demo\ntype S = enum {{ A, B }}\nagent M {{\n  key id: String\n  store s: Cell[S] = A\n  on call go(n: Int) -> Effect[Int] {{\n    match n {{\n{arms}      _ => 0\n    }}\n  }}\n}}\n"
        );
        let m = model(&src, "M");
        let sm = only(&m);
        assert_eq!(sm.truncated, s(&["go"]));
        assert_eq!(
            edges(sm),
            [("go".into(), s(&["A", "B"]), Target::Unknown, None)]
        );
        assert_eq!(sm.reachable, s(&["A", "B"]));
    }

    #[test]
    fn several_state_fields_give_one_machine_each() {
        let src = r#"context demo
type S = enum { A, B }
type T = enum { X, Y }
agent M {
  key id: String
  store s: Cell[S] = A
  store t: Cell[T] = X
  on call go() -> Effect[()] {
    match s {
      A => { t := Y }
      B => { () }
    }
  }
}
"#;
        let m = model(src, "M");
        assert_eq!(m.machines.len(), 2);
        // On `t`'s machine, the match on `s` is a guard.
        let t = &m.machines[1];
        assert_eq!(t.field, "t");
        assert_eq!(t.edges[0].to, to("Y"));
        assert_eq!(t.edges[0].guards, s(&["s is A"]));
    }

    const SHAPES: &str = r#"context demo
type Shape =
  | Circle(r: Int)
  | Square(side: Int)
agent Figure {
  key id: String
  store shape: Cell[Shape] = Square(1)

  // A refutable arm leaves its constructor to the later arms.
  on call grow() -> Effect[Int] {
    match shape {
      Square(1) => 0
      Square(n) => {
        shape := Circle(n)
        1
      }
      Circle(_) => 2
    }
  }

  // A refined pattern, or a payload literal, covers nothing for the arms
  // after it.
  on call guarded() -> Effect[Int] {
    match shape {
      _ where Positive => 0
      Circle(0) => 1
      Circle(_) => 2
      Square(_) => 3
    }
  }

  // `is` / `!=` against a payload: the false side keeps the constructor.
  on call probe() -> Effect[Int] {
    if shape is Square(1) { 0 } else { 1 }
  }
  on call probeNe() -> Effect[Int] {
    if shape != Square(1) { 0 } else { 1 }
  }

  // After a known write, a refutable test is not decided.
  on call reset() -> Effect[Int] {
    shape := Square(1)
    if shape is Square(2) { 0 } else { 1 }
  }
}
"#;

    fn from_of(sm: &StateMachine, handler: &str) -> Vec<(Vec<String>, Option<String>)> {
        sm.edges
            .iter()
            .filter(|e| e.handler == handler)
            .map(|e| (e.from.clone(), e.outcome.clone()))
            .collect()
    }

    #[test]
    fn a_refutable_arm_does_not_take_its_constructor_from_later_arms() {
        let m = model(SHAPES, "Figure");
        let sm = only(&m);
        let grow: Vec<_> = sm.edges.iter().filter(|e| e.handler == "grow").collect();
        assert_eq!(grow.len(), 3);
        // `Square(n)` still sees `Square`, so `Square → Circle` survives.
        assert_eq!(grow[1].from, s(&["Square"]));
        assert_eq!(grow[1].to, to("Circle"));
        assert_eq!(grow[2].from, s(&["Circle"]));
        assert_eq!(sm.reachable, s(&["Circle", "Square"]));
    }

    #[test]
    fn a_refined_wildcard_arm_leaves_every_state_to_the_later_arms() {
        let m = model(SHAPES, "Figure");
        assert_eq!(
            from_of(only(&m), "guarded"),
            [
                (s(&["Circle", "Square"]), out("0")),
                (s(&["Circle"]), out("1")),
                (s(&["Circle"]), out("2")),
                (s(&["Square"]), out("3")),
            ]
        );
    }

    #[test]
    fn the_false_side_of_a_refutable_test_keeps_the_constructor() {
        let m = model(SHAPES, "Figure");
        let both = s(&["Circle", "Square"]);
        assert_eq!(
            from_of(only(&m), "probe"),
            [(s(&["Square"]), out("0")), (both.clone(), out("1"))]
        );
        // `!= Square(1)` true allows anything; false is only `Square`.
        assert_eq!(
            from_of(only(&m), "probeNe"),
            [(both, out("0")), (s(&["Square"]), out("1"))]
        );
    }

    #[test]
    fn a_refutable_test_after_a_known_write_keeps_both_branches() {
        let m = model(SHAPES, "Figure");
        let reset: Vec<_> = from_of(only(&m), "reset")
            .into_iter()
            .map(|e| e.1)
            .collect();
        assert_eq!(reset, [out("0"), out("1")]);
    }

    // -- #2197: the `transition` overlay.

    fn violations(
        sm: &StateMachine,
        handler: &str,
    ) -> Vec<(String, Certainty, Vec<String>, Vec<String>)> {
        sm.edges
            .iter()
            .filter(|e| e.handler == handler)
            .flat_map(|e| e.violations.iter())
            .map(|v| {
                (
                    v.transition.clone(),
                    v.certainty,
                    v.from.clone(),
                    v.targets.clone(),
                )
            })
            .collect()
    }

    #[test]
    fn an_unguarded_write_from_paid_violates_paid_is_terminal() {
        let m = model(PAID, "Order");
        let sm = only(&m);
        assert_eq!(
            violations(sm, "place"),
            [(
                "paid_is_terminal".into(),
                Certainty::Known,
                s(&["Paid"]),
                s(&["Placed"])
            )]
        );
        // `pay` writes `Paid`, which the predicate allows from anywhere.
        assert!(violations(sm, "pay").is_empty());
        // The snapshot `invariant` is not a step invariant: nothing listed.
        assert!(sm.not_evaluated.is_empty());
        let v = &sm.edges[0].violations[0];
        assert_eq!(
            v.span.start,
            PAID.find("transition paid_is_terminal").unwrap()
        );
    }

    const OVERLAY: &str = r#"context demo
type S = enum { A, B, C }
agent M {
  key id: String
  store s: Cell[S] = A
  store hits: Cell[Int]

  transition must_advance: old.s is A implies new.s is B

  // Writes another field, so it commits: the refused path is A -> A.
  on call refuse() -> Effect[Bool] {
    hits := hits + 1
    match s {
      A => false
      B => true
      C => true
    }
  }

  // Writes nothing, so it never commits: transitions are not checked.
  on call peek() -> Effect[Bool] {
    match s {
      A => false
      B => true
      C => true
    }
  }

  // A computed write from A may land on B (fine) or A/C (violating).
  on call spin() -> Effect[()] {
    let _ <- s.update((x) => x)
    ()
  }
}
"#;

    #[test]
    fn an_unchanged_path_violates_only_in_a_handler_that_commits() {
        let m = model(OVERLAY, "M");
        let sm = only(&m);
        assert_eq!(
            violations(sm, "refuse"),
            [(
                "must_advance".into(),
                Certainty::Known,
                s(&["A"]),
                s(&["A"])
            )]
        );
        assert!(violations(sm, "peek").is_empty(), "peek never commits");
    }

    /// #2219: `ship()` moves `A -> C`, which `must_advance` refuses, but
    /// nothing leaves the agent in `A` after a commit, so the move can only be
    /// the genesis commit — which the runtime never checks.
    fn genesis(extra: &str) -> String {
        format!(
            r#"context demo
type S = enum {{ A, B, C }}
agent M {{
  key id: String
  store s: Cell[S] = A
  store hits: Cell[Int]

  transition must_advance: old.s is A implies new.s is B

  on call ship() -> Effect[()] {{
    if s is A {{ s := C }} else {{ () }}
  }}
{extra}
}}
"#
        )
    }

    #[test]
    fn a_move_only_the_genesis_commit_can_make_is_not_flagged() {
        let m = model(&genesis(""), "M");
        let sm = only(&m);
        assert_eq!(
            sm.reachable,
            s(&["A", "C"]),
            "A is still a state the agent is in"
        );
        assert_eq!(sm.reachable_after_commit, s(&["C"]));
        assert!(
            violations(sm, "ship").is_empty(),
            "{:?}",
            violations(sm, "ship")
        );
    }

    #[test]
    fn a_committing_unchanged_path_from_the_initial_state_brings_it_back() {
        // `touch()` commits (it writes `hits`) and leaves `s` at `A`: the
        // genesis call is skipped, but a second call from `A` is checked.
        let m = model(
            &genesis("  on call touch() -> Effect[()] {\n    hits := hits + 1\n  }"),
            "M",
        );
        let sm = only(&m);
        assert_eq!(sm.reachable_after_commit, s(&["A", "C"]));
        assert_eq!(
            violations(sm, "ship"),
            [(
                "must_advance".into(),
                Certainty::Known,
                s(&["A"]),
                s(&["C"])
            )]
        );
    }

    #[test]
    fn a_move_back_into_the_initial_state_brings_it_back() {
        let m = model(
            &genesis("  on call reset() -> Effect[()] {\n    s := A\n  }"),
            "M",
        );
        let sm = only(&m);
        assert_eq!(sm.reachable_after_commit, s(&["A", "C"]));
        assert_eq!(violations(sm, "ship").len(), 1);
    }

    #[test]
    fn a_computed_write_can_land_back_in_the_initial_state() {
        let m = model(
            &genesis(
                "  on call spin() -> Effect[()] {\n    let _ <- s.update((x) => x)\n    ()\n  }",
            ),
            "M",
        );
        assert_eq!(only(&m).reachable_after_commit, s(&["A", "B", "C"]));
    }

    #[test]
    fn a_non_committing_handler_never_brings_the_initial_state_back() {
        // `peek()` writes nothing, so it never commits: being in `A` after it
        // is still being in `A` before any commit.
        let m = model(
            &genesis(
                "  on call peek() -> Effect[Bool] {\n    match s {\n      A => false\n      B => true\n      C => true\n    }\n  }",
            ),
            "M",
        );
        let sm = only(&m);
        assert_eq!(sm.reachable_after_commit, s(&["C"]));
        assert!(violations(sm, "ship").is_empty());
    }

    #[test]
    fn without_a_literal_initialiser_every_reachable_state_counts() {
        let src = genesis("").replace("store s: Cell[S] = A", "store s: Cell[S]");
        let m = model(&src, "M");
        let sm = only(&m);
        assert_eq!(sm.reachable_after_commit, sm.reachable);
        assert_eq!(violations(sm, "ship").len(), 1);
    }

    #[test]
    fn a_computed_write_is_possible_when_some_targets_violate() {
        let m = model(OVERLAY, "M");
        assert_eq!(
            violations(only(&m), "spin"),
            [(
                "must_advance".into(),
                Certainty::Possible,
                s(&["A"]),
                s(&["A", "C"])
            )]
        );
    }

    #[test]
    fn a_computed_write_is_known_when_every_target_violates() {
        let src = r#"context demo
type S = enum { A, B }
agent M {
  key id: String
  store s: Cell[S] = A
  transition frozen: old.s is A implies !(new.s is A || new.s == B)
  on call spin() -> Effect[()] {
    let _ <- s.update((x) => x)
    ()
  }
}
"#;
        let m = model(src, "M");
        assert_eq!(
            violations(only(&m), "spin"),
            [("frozen".into(), Certainty::Known, s(&["A"]), s(&["A", "B"]))]
        );
    }

    #[test]
    fn an_edge_from_an_unreachable_state_is_not_flagged() {
        let src = r#"context demo
type S = enum { Pending, Placed, Cancelled }
agent M {
  key id: String
  store s: Cell[S] = Pending
  transition cancelled_is_final: old.s is Cancelled implies new.s is Cancelled
  on call place() -> Effect[()] { s := Placed }
}
"#;
        let m = model(src, "M");
        let sm = only(&m);
        assert!(!sm.reachable.contains(&"Cancelled".to_string()));
        assert!(violations(sm, "place").is_empty());
    }

    #[test]
    fn predicates_outside_the_fragment_are_listed_and_others_ignored() {
        // 1374's hoisting predicate, a refutable pattern, and a numeric
        // transition that never mentions the state field.
        let src = r#"context demo
type Shape = | Circle(r: Int) | Square(side: Int)
agent M {
  key id: String
  store shape: Cell[Shape] = Circle(1)
  store count: Cell[Int]
  transition hoisting: if old.shape == old.shape { true } else { false }
  transition refutable: old.shape is Circle(1) implies new.shape is Circle(1)
  transition counting: new.count >= old.count
  on call go() -> Effect[()] { shape := Square(1) }
}
"#;
        let m = model(src, "M");
        let sm = only(&m);
        let names: Vec<&str> = sm
            .not_evaluated
            .iter()
            .map(|n| n.transition.as_str())
            .collect();
        assert_eq!(names, ["hoisting", "refutable"]);
        assert!(sm.edges.iter().all(|e| e.violations.is_empty()));
    }

    #[test]
    fn a_truncated_handler_is_checked_as_a_computed_write_only_if_it_commits() {
        let arms: String = (0..=PATH_BUDGET)
            .map(|i| format!("      {i} => {i}\n"))
            .collect();
        let src = format!(
            "context demo\ntype S = enum {{ A, B }}\nagent M {{\n  key id: String\n  store s: Cell[S] = A\n  store hits: Cell[Int]\n  transition a_stays: old.s is A implies new.s is A\n  on call go(n: Int) -> Effect[Int] {{\n    hits := n\n    match n {{\n{arms}      _ => 0\n    }}\n  }}\n  on call peek(n: Int) -> Effect[Int] {{\n    match n {{\n{arms}      _ => 0\n    }}\n  }}\n}}\n"
        );
        let m = model(&src, "M");
        assert_eq!(only(&m).truncated, s(&["go", "peek"]));
        // `go` writes `hits`, so it commits: checked as a computed write.
        assert_eq!(
            violations(only(&m), "go"),
            [("a_stays".into(), Certainty::Possible, s(&["A"]), s(&["B"]))]
        );
        // `peek` writes nothing, so it never commits: no transition runs.
        assert!(violations(only(&m), "peek").is_empty());
    }

    #[test]
    fn certainty_and_targets_are_per_start_state() {
        // From C every target violates (Known); from A only A and C do
        // (Possible) — B is fine from A.
        let src = r#"context demo
type S = enum { A, B, C }
agent M {
  key id: String
  store s: Cell[S] = A
  transition t: !(old.s is C) && (old.s is A implies new.s is B)
  on call reset() -> Effect[()] { s := C }
  on call spin() -> Effect[()] {
    let _ <- s.update((x) => x)
    ()
  }
}
"#;
        let m = model(src, "M");
        assert_eq!(
            violations(only(&m), "spin"),
            [
                ("t".into(), Certainty::Possible, s(&["A"]), s(&["A", "C"])),
                ("t".into(), Certainty::Known, s(&["C"]), s(&["A", "B", "C"])),
            ]
        );
    }

    #[test]
    fn a_parameter_shadowing_a_store_field_does_not_make_a_handler_commit() {
        let src = r#"context demo
type S = enum { A, B }
agent M {
  key id: String
  store s: Cell[S] = A
  store log: Log[Int]
  transition a_moves: old.s is A implies new.s is B
  on call peek(log: List[Int]) -> Effect[Bool] {
    let _ <- log.append(1)
    true
  }
}
"#;
        let m = model(src, "M");
        // `log` is the parameter, not the store field: nothing commits, so
        // the unchanged A -> A path is not flagged.
        assert!(violations(only(&m), "peek").is_empty());
    }

    #[test]
    fn a_local_shadowing_a_store_field_does_not_make_a_handler_commit() {
        // #2220: a `let`, a match binding and a lambda parameter shadow the
        // field just as a handler parameter does (found by `bynk-lower`'s
        // agreement test against `body_writes_state`).
        let src = r#"context demo
type S = enum { A, B }
agent M {
  key id: String
  store s: Cell[S] = A
  store log: Log[Int]
  transition a_moves: old.s is A implies new.s is B
  on call byLet(xs: List[Int]) -> Effect[Bool] {
    let log = xs
    let _ <- log.append(1)
    true
  }
  on call byArm(o: Option[List[Int]]) -> Effect[()] {
    match o {
      Some(log) => log.append(1)
      None => Effect.pure(())
    }
  }
  on call byLambda(xss: List[List[Int]]) -> Effect[Bool] {
    let _ = xss.map((log) => log.append(1))
    true
  }
  on call byBlockLambda(xss: List[List[Int]]) -> Effect[Bool] {
    let _ = xss.map((x) => {
      let log = x
      log.append(1)
    })
    true
  }
}
"#;
        let m = model(src, "M");
        for h in ["byLet", "byArm", "byLambda", "byBlockLambda"] {
            assert!(violations(only(&m), h).is_empty(), "{h}");
        }
    }

    #[test]
    fn a_known_violation_warns_at_the_write_with_the_transition_and_a_witness() {
        // Fixture 245: `place()` from `Paid`, reachable through `pay()`.
        let w = warnings(PAID, "Order");
        assert_eq!(
            w,
            [(
                "`place()` can move `status` from `Paid` to `Placed`, which `paid_is_terminal` refuses at commit"
                    .to_string(),
                vec![
                    "`paid_is_terminal` is declared here".to_string(),
                    "`pay()` moves `status` to `Paid`".to_string(),
                ],
            )]
        );
    }

    #[test]
    fn a_refused_unchanged_path_in_a_committing_handler_warns() {
        let w = warnings(OVERLAY, "M");
        assert!(
            w.iter().any(|(m, l)| m
                == "`refuse()` can leave `s` at `A`, which `must_advance` refuses at commit"
                && l[1] == "`refuse()` commits and leaves `s` at `A`"),
            "{w:?}"
        );
        // `spin()`'s computed write only *may* land badly: no warning.
        assert!(!w.iter().any(|(m, _)| m.starts_with("`spin()`")), "{w:?}");
        // `peek()` never commits.
        assert!(!w.iter().any(|(m, _)| m.starts_with("`peek()`")), "{w:?}");
    }

    #[test]
    fn the_witness_is_a_path_that_can_start_in_a_reachable_state() {
        // `C` is unreachable, so `x()`'s `C -> B` can't explain why `B` is
        // reachable even though it comes first; `y()` does.
        let src = r#"context demo
type S = enum { A, B, C }
agent M {
  key id: String
  store s: Cell[S] = A
  transition b_is_final: old.s is B implies new.s is B
  on call x() -> Effect[()] {
    match s {
      C => { s := B }
      _ => { () }
    }
  }
  on call y() -> Effect[()] { s := B }
  on call undo() -> Effect[()] { s := A }
}
"#;
        let w = warnings(src, "M");
        let undo = w
            .iter()
            .find(|(m, _)| m.starts_with("`undo()`"))
            .unwrap_or_else(|| panic!("{w:?}"));
        assert_eq!(undo.1[1], "`y()` moves `s` to `B`");
    }

    #[test]
    fn a_warning_names_the_paths_guards() {
        let src = r#"context demo
type S = enum { A, B }
agent M {
  key id: String
  store s: Cell[S] = A
  transition b_is_final: old.s is B implies new.s is B
  on call finish() -> Effect[()] { s := B }
  on call undo(force: Bool) -> Effect[()] {
    if force { s := A } else { () }
  }
}
"#;
        assert_eq!(
            warnings(src, "M")[0].0,
            "`undo()` can move `s` from `B` to `A` (when `force`), which `b_is_final` refuses at commit"
        );
    }

    #[test]
    fn a_move_only_the_genesis_commit_can_make_does_not_warn() {
        assert!(warnings(&genesis(""), "M").is_empty());
    }

    #[test]
    fn a_not_evaluated_transition_does_not_warn() {
        let src = r#"context demo
type S = enum { A, B }
agent M {
  key id: String
  store s: Cell[S] = A
  store n: Cell[Int]
  transition counted: old.s is A implies new.n > old.n
  on call go() -> Effect[()] { s := B }
}
"#;
        assert!(only(&model(src, "M")).not_evaluated.len() == 1);
        assert!(warnings(src, "M").is_empty());
    }

    #[test]
    fn a_truncated_handler_does_not_warn() {
        // Past the path budget, `go()` becomes one `?` edge from every state.
        // Every target from `A` violates `never_from_a`, so the overlay marks
        // it Known, but a truncated handler's edges aren't trustworthy enough
        // to warn on.
        let arms: String = (0..=PATH_BUDGET)
            .map(|i| format!("      {i} => {i}\n"))
            .collect();
        let src = format!(
            "context demo\ntype S = enum {{ A, B }}\nagent M {{\n  key id: String\n  store s: Cell[S] = A\n  store hits: Cell[Int]\n  transition never_from_a: !(old.s is A)\n  on call go(n: Int) -> Effect[Int] {{\n    hits := hits + 1\n    match n {{\n{arms}      _ => 0\n    }}\n  }}\n}}\n"
        );
        let m = model(&src, "M");
        assert_eq!(only(&m).truncated, s(&["go"]));
        assert!(
            !violations(only(&m), "go").is_empty(),
            "the overlay still flags it"
        );
        assert!(warnings(&src, "M").is_empty());
    }

    #[test]
    fn an_agent_without_transitions_has_no_warnings() {
        let src = r#"context demo
type S = enum { A, B }
agent M {
  key id: String
  store s: Cell[S] = A
  on call go() -> Effect[()] { s := B }
}
"#;
        assert!(warnings(src, "M").is_empty());
    }
}
