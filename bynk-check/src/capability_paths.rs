//! #2469: capability *paths* — the answer to "why does this need `Logger`?".
//!
//! The [requirement ledger](crate::requirements) records every capability use
//! and whether the enclosing declaration's `given` covers it. This module adds
//! the second half of the explanation: how a declaration *reaches* a use that
//! lives in another body. The only such edge in the language is an agent-handler
//! call (`Agent(key).m()`): a service or agent handler's deps are widened to
//! cover the agent handlers it calls (#527), and a provider must list them in
//! its `given` (`bynk.provider.agent_call_undeclared_capability`).
//!
//! The checker owns the answer. [`CapabilityGraph::build`] runs in the same
//! pass as that provider check, over the same `Callee::Agent` classification,
//! and records two things per file:
//!
//! - one **witness** — the shortest chain — for each `(given entry, capability)`
//!   pair, so a hover is a lookup; and
//! - the compact call/use facts the witnesses were drawn from, so the full set
//!   of chains is computed *on demand* ([`CapabilityGraph::all_paths`]) rather
//!   than stored for every pair.
//!
//! Nothing here re-reads source text: the editor surfaces ask this module, they
//! never re-derive a path.

use crate::checker::Callee;
use crate::requirements::{Requirement, RequirementSource};
use crate::symbols::UnitTable;
use bynk_syntax::ast::{Block, CapRef, ExprId, HandlerKind};
use bynk_syntax::span::{FileId, Span};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

/// The most chains [`CapabilityGraph::all_paths`] returns for one entry. A
/// densely connected project can have exponentially many simple paths; the
/// action lists the shortest `MAX_PATHS` and says it stopped.
pub const MAX_PATHS: usize = 64;

/// The most nodes one [`CapabilityGraph::all_paths`] walk expands before it
/// stops with the chains found so far.
const WALK_BUDGET: usize = 20_000;

/// One hop of a capability path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathStep {
    /// The declaration being walked calls the agent handler `agent.handler` at
    /// `site` (the method name in the call).
    Call {
        site: Span,
        agent: String,
        handler: String,
    },
    /// The chain ends: the capability is used at `site`.
    Use {
        site: Span,
        source: RequirementSource,
    },
}

impl PathStep {
    pub fn site(&self) -> Span {
        match self {
            PathStep::Call { site, .. } | PathStep::Use { site, .. } => *site,
        }
    }
}

/// A chain from a declaration to a use of a capability: zero or more
/// [`PathStep::Call`]s then exactly one [`PathStep::Use`]. A *direct* use is the
/// one-step chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityPath {
    pub steps: Vec<PathStep>,
}

impl CapabilityPath {
    /// `true` when the declaration uses the capability itself.
    pub fn is_direct(&self) -> bool {
        self.steps.len() == 1
    }
}

/// The shortest chain recorded for one `given` entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityWitness {
    /// The `given` entry the chain explains.
    pub given_entry: Span,
    pub capability: String,
    /// The declaration that carries the entry: `Cart.add`, a provider's name.
    pub declaration: String,
    pub path: CapabilityPath,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Use {
    capability: String,
    site: Span,
    source: RequirementSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    label: String,
    uses: Vec<Use>,
    /// `(call site, callee node)`, in source order.
    calls: Vec<(Span, usize, String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    given_entry: Span,
    capability: String,
    node: usize,
}

/// One file's capability call/use facts and the witnesses drawn from them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityGraph {
    /// The file this graph was recorded for. A unit's table spans all of its
    /// files, so the steps of a path can lie in a sibling file; the span's own
    /// file id says which, and this lets the editor map ids back to paths.
    pub file: FileId,
    nodes: Vec<Node>,
    entries: Vec<Entry>,
    witnesses: Vec<CapabilityWitness>,
}

/// Project-relative source path → that file's capability graph.
pub type FileCapabilityPaths = HashMap<PathBuf, CapabilityGraph>;

/// A handler-shaped body the walk treats as one node: its own `given`, body, and
/// the label it is shown under.
struct Decl<'t> {
    label: String,
    given: &'t [CapRef],
    bodies: Vec<&'t Block>,
}

fn handler_label(owner: &str, kind: &HandlerKind, method: Option<&str>) -> String {
    match (method, kind) {
        (Some(m), _) => format!("{owner}.{m}"),
        (None, HandlerKind::Call) => format!("{owner}.call"),
        (None, _) => owner.to_string(),
    }
}

fn within(inner: Span, outer: Span) -> bool {
    let same_file =
        inner.file == outer.file || inner.file == FileId::UNKNOWN || outer.file == FileId::UNKNOWN;
    same_file && outer.start <= inner.start && inner.end <= outer.end
}

/// A graph is per file, so lookups key on the byte range alone — the editor
/// holds offsets, not the checker's file ids.
fn same_range(a: Span, b: Span) -> bool {
    a.start == b.start && a.end == b.end
}

impl CapabilityGraph {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Fold `other` in. A unit-wide table is walked once per file of the unit,
    /// so a graph whose entries are all already present adds nothing.
    pub fn merge(&mut self, other: CapabilityGraph) {
        let known: HashSet<(Span, String)> = self
            .entries
            .iter()
            .map(|e| (e.given_entry, e.capability.clone()))
            .collect();
        let fresh = |given_entry: Span, capability: &str| {
            !known.contains(&(given_entry, capability.to_string()))
        };
        if !other
            .entries
            .iter()
            .any(|e| fresh(e.given_entry, &e.capability))
        {
            return;
        }
        if self.entries.is_empty() {
            self.file = other.file;
        }
        let base = self.nodes.len();
        for n in &other.nodes {
            let mut n = n.clone();
            for c in &mut n.calls {
                c.1 += base;
            }
            self.nodes.push(n);
        }
        for e in other.entries {
            if fresh(e.given_entry, &e.capability) {
                self.entries.push(Entry {
                    node: e.node + base,
                    ..e
                });
            }
        }
        for w in other.witnesses {
            if fresh(w.given_entry, &w.capability) {
                self.witnesses.push(w);
            }
        }
    }

    /// Build the graph for a unit table. `requirements` is the ledger recorded
    /// so far for the file being checked; only **covered** requirements are
    /// uses (an uncovered one is a missing `given`, not a path).
    pub fn build(
        table: &UnitTable,
        callees: &HashMap<ExprId, Callee>,
        requirements: &[Requirement],
        own: FileId,
    ) -> CapabilityGraph {
        let mut decls: Vec<Decl<'_>> = Vec::new();
        let mut agents: Vec<_> = table.agents.values().collect();
        agents.sort_by_key(|a| a.span.start);
        for agent in agents {
            for h in &agent.handlers {
                decls.push(Decl {
                    label: handler_label(
                        &agent.name.name,
                        &h.kind,
                        h.method_name.as_ref().map(|m| m.name.as_str()),
                    ),
                    given: &h.given,
                    bodies: vec![&h.body],
                });
            }
        }
        let mut services: Vec<_> = table.services.values().collect();
        services.sort_by_key(|s| s.span.start);
        for svc in services {
            for h in &svc.handlers {
                decls.push(Decl {
                    label: handler_label(
                        &svc.name.name,
                        &h.kind,
                        h.method_name.as_ref().map(|m| m.name.as_str()),
                    ),
                    given: &h.given,
                    bodies: vec![&h.body],
                });
            }
        }
        let mut providers: Vec<_> = table.providers.values().collect();
        providers.sort_by_key(|p| p.span.start);
        for p in providers {
            decls.push(Decl {
                label: p.provider_name.name.clone(),
                given: &p.given,
                bodies: p.ops.iter().map(|op| &op.body).collect(),
            });
        }

        // Agent handler `(agent, handler)` → its node, for call edges.
        let mut handler_node: HashMap<(String, String), usize> = HashMap::new();
        for (i, d) in decls.iter().enumerate() {
            if let Some((a, m)) = d.label.split_once('.')
                && table.agents.contains_key(a)
            {
                handler_node.insert((a.to_string(), m.to_string()), i);
            }
        }

        let mut nodes = Vec::with_capacity(decls.len());
        for d in &decls {
            let mut uses = Vec::new();
            let mut calls = Vec::new();
            for body in &d.bodies {
                for r in requirements.iter().filter(|r| r.covered) {
                    if within(r.site, body.span) {
                        uses.push(Use {
                            capability: r.capability.clone(),
                            site: r.site,
                            source: r.source.clone(),
                        });
                    }
                }
                for (agent, handler, site) in crate::context_checks::agent_calls_in(body, callees) {
                    if let Some(&to) = handler_node.get(&(agent.clone(), handler.clone())) {
                        calls.push((site, to, agent, handler));
                    }
                }
            }
            uses.sort_by_key(|u| (u.site.start, u.site.end));
            calls.sort_by_key(|c| (c.0.start, c.0.end));
            nodes.push(Node {
                label: d.label.clone(),
                uses,
                calls,
            });
        }

        let mut graph = CapabilityGraph {
            file: own,
            nodes,
            entries: Vec::new(),
            witnesses: Vec::new(),
        };
        let mut reported: HashSet<(usize, &str)> = HashSet::new();
        for (i, d) in decls.iter().enumerate() {
            for cap in d.given {
                // Each file's pass walks the whole unit; an entry is explained
                // by the pass of the file it is written in.
                if cap.span.file != own && cap.span.file != FileId::UNKNOWN {
                    continue;
                }
                // A second `given` of the same capability on one declaration is
                // already a diagnosed duplicate; only the first is explained.
                if !reported.insert((i, cap.key())) {
                    continue;
                }
                graph.entries.push(Entry {
                    given_entry: cap.span,
                    capability: cap.key().to_string(),
                    node: i,
                });
            }
        }
        for e in &graph.entries {
            if let Some(path) = graph.shortest(e.node, &e.capability) {
                graph.witnesses.push(CapabilityWitness {
                    given_entry: e.given_entry,
                    capability: e.capability.clone(),
                    declaration: graph.nodes[e.node].label.clone(),
                    path,
                });
            }
        }
        graph
    }

    /// The `given` entry covering byte `offset` (a cursor on either edge counts),
    /// with the capability it names.
    pub fn entry_at(&self, offset: usize) -> Option<(Span, &str)> {
        self.entries
            .iter()
            .find(|e| e.given_entry.start <= offset && offset <= e.given_entry.end)
            .map(|e| (e.given_entry, e.capability.as_str()))
    }

    /// The shortest recorded chain for the `given` entry at `given_entry`.
    pub fn witness(&self, given_entry: Span) -> Option<&CapabilityWitness> {
        self.witnesses
            .iter()
            .find(|w| same_range(w.given_entry, given_entry))
    }

    /// Every chain from the declaration carrying `given_entry` to a use of its
    /// capability, shortest first, at most [`MAX_PATHS`]. Computed on demand
    /// from the recorded call/use facts. The `bool` is `true` when the cap cut
    /// the list short.
    pub fn all_paths(&self, given_entry: Span) -> Option<(String, Vec<CapabilityPath>, bool)> {
        let e = self
            .entries
            .iter()
            .find(|e| same_range(e.given_entry, given_entry))?;
        let mut paths = self.walk(e.node, &e.capability, MAX_PATHS + 1);
        let truncated = paths.len() > MAX_PATHS;
        paths.truncate(MAX_PATHS);
        Some((e.capability.clone(), paths, truncated))
    }

    /// The nodes from which a use of `capability` is reachable (a use of its
    /// own counts). Walks that never leave this set cannot dead-end.
    fn reaches(&self, capability: &str) -> Vec<bool> {
        let mut callers: Vec<Vec<usize>> = vec![Vec::new(); self.nodes.len()];
        for (i, n) in self.nodes.iter().enumerate() {
            for c in &n.calls {
                callers[c.1].push(i);
            }
        }
        let mut reach = vec![false; self.nodes.len()];
        let mut stack: Vec<usize> = Vec::new();
        for (i, n) in self.nodes.iter().enumerate() {
            if n.uses.iter().any(|u| u.capability == capability) {
                reach[i] = true;
                stack.push(i);
            }
        }
        while let Some(i) = stack.pop() {
            for &c in &callers[i] {
                if !reach[c] {
                    reach[c] = true;
                    stack.push(c);
                }
            }
        }
        reach
    }

    /// The shortest chain from `from` to a use of `capability`: a breadth-first
    /// search over nodes with a parent pointer each, O(nodes + calls).
    fn shortest(&self, from: usize, capability: &str) -> Option<CapabilityPath> {
        // node -> (predecessor, the call step that reached it)
        let mut parent: HashMap<usize, (usize, PathStep)> = HashMap::new();
        let mut seen: HashSet<usize> = HashSet::from([from]);
        let mut queue: VecDeque<usize> = VecDeque::from([from]);
        while let Some(node) = queue.pop_front() {
            let n = &self.nodes[node];
            if let Some(u) = n.uses.iter().find(|u| u.capability == capability) {
                let mut steps = vec![PathStep::Use {
                    site: u.site,
                    source: u.source.clone(),
                }];
                let mut at = node;
                while let Some((prev, step)) = parent.get(&at) {
                    steps.push(step.clone());
                    at = *prev;
                }
                steps.reverse();
                return Some(CapabilityPath { steps });
            }
            for (site, to, agent, handler) in &n.calls {
                if seen.insert(*to) {
                    parent.insert(
                        *to,
                        (
                            node,
                            PathStep::Call {
                                site: *site,
                                agent: agent.clone(),
                                handler: handler.clone(),
                            },
                        ),
                    );
                    queue.push_back(*to);
                }
            }
        }
        None
    }

    /// Simple paths from `from` to a use of `capability`, shortest first, at
    /// most `limit`. A breadth-first walk that never revisits a node on its own
    /// path (so a call cycle terminates) and never enters a node that cannot
    /// reach a use (so an unused or sparse entry costs O(nodes + calls)). A
    /// total-work budget bounds dense graphs, where simple paths are factorial.
    fn walk(&self, from: usize, capability: &str, limit: usize) -> Vec<CapabilityPath> {
        let reach = self.reaches(capability);
        let mut out = Vec::new();
        if !reach[from] {
            return out;
        }
        let mut budget = WALK_BUDGET;
        // (node, steps so far, nodes on the path)
        let mut queue: VecDeque<(usize, Vec<PathStep>, Vec<usize>)> = VecDeque::new();
        queue.push_back((from, Vec::new(), vec![from]));
        while let Some((node, steps, on_path)) = queue.pop_front() {
            if budget == 0 {
                break;
            }
            budget -= 1;
            let n = &self.nodes[node];
            for u in n.uses.iter().filter(|u| u.capability == capability) {
                let mut s = steps.clone();
                s.push(PathStep::Use {
                    site: u.site,
                    source: u.source.clone(),
                });
                out.push(CapabilityPath { steps: s });
                if out.len() >= limit {
                    return out;
                }
            }
            for (site, to, agent, handler) in &n.calls {
                if on_path.contains(to) || !reach[*to] {
                    continue;
                }
                let mut s = steps.clone();
                s.push(PathStep::Call {
                    site: *site,
                    agent: agent.clone(),
                    handler: handler.clone(),
                });
                let mut p = on_path.clone();
                p.push(*to);
                queue.push_back((*to, s, p));
            }
        }
        out
    }
}
