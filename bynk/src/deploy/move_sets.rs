//! #3565 (agent-context moves, slice 3): the move sets of a run and their place in its order
//! (`design/tracks/agent-context-moves.md` §3.1, §3.2).
//!
//! A **move set** is the connected set of a run's moves: every move that shares a source or a
//! receiver script with another. It is moved as one unit, in three phases, and is placed in the
//! run's order as **two nodes**: *receive* (every phase 1 push) after what the receivers call
//! outside the set, and *commit* (every phase 2 push, then every phase 3 push) after *receive*,
//! after what the sources call outside the set, and after every Worker that calls a receiver.
//! So those callers are pushed between the two nodes, already bound to the receiver, while it
//! still forwards the moving contexts to their sources.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use super::agent_move::AgentMove;
use super::config::Resources;
use super::graph::{OrderError, deploy_order};

/// One move set: the moves that share a source or a receiver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MoveSet {
    pub(crate) moves: Vec<AgentMove>,
}

impl MoveSet {
    /// The receiving Workers, by directory, in order.
    pub(crate) fn receivers(&self) -> Vec<String> {
        sorted(self.moves.iter().map(|m| m.to_worker.clone()))
    }

    /// The source Workers, by ledger key, in order.
    #[cfg(test)]
    pub(crate) fn sources(&self) -> Vec<String> {
        sorted(self.moves.iter().map(|m| m.from_worker.clone()))
    }

    /// The source scripts, in order.
    pub(crate) fn source_scripts(&self) -> Vec<String> {
        sorted(self.moves.iter().map(|m| m.from_script.clone()))
    }

    /// The receiving scripts, in order.
    pub(crate) fn receiver_scripts(&self) -> Vec<String> {
        sorted(self.moves.iter().map(|m| m.to_script.clone()))
    }

    /// A script that is both a source and a receiver of the set (D9's `chained`), if any.
    pub(crate) fn chained(&self) -> Option<String> {
        let receivers: BTreeSet<String> = self.receiver_scripts().into_iter().collect();
        self.source_scripts()
            .into_iter()
            .find(|s| receivers.contains(s))
    }

    /// The Workers of the set that are Workers of this build: every receiver, and each source
    /// that keeps running (a dissolved source is pushed as a stub, not as a Worker of the build).
    pub(crate) fn built_workers(&self, resources: &BTreeMap<String, Resources>) -> Vec<String> {
        let mut workers = self.receivers();
        for m in &self.moves {
            if m.old_script_built
                && let Some(w) = built_worker_of(&m.from_script, &m.from_worker, resources)
            {
                workers.push(w);
            }
        }
        sorted(workers)
    }

    /// The sources that keep running, by directory.
    fn built_sources(&self, resources: &BTreeMap<String, Resources>) -> Vec<String> {
        let receivers = self.receivers();
        self.built_workers(resources)
            .into_iter()
            .filter(|w| !receivers.contains(w))
            .collect()
    }
}

/// The directory of a source that is still a Worker of the build: its ledger key, when that is
/// a Worker of the build.
fn built_worker_of(
    _script: &str,
    worker: &str,
    resources: &BTreeMap<String, Resources>,
) -> Option<String> {
    resources.contains_key(worker).then(|| worker.to_string())
}

fn sorted(items: impl IntoIterator<Item = String>) -> Vec<String> {
    let set: BTreeSet<String> = items.into_iter().collect();
    set.into_iter().collect()
}

/// The move sets of `moves`: the connected components of the moves, joined where they share a
/// source or a receiving script. In order of each set's first move.
pub(crate) fn move_sets(moves: &[AgentMove]) -> Vec<MoveSet> {
    // Union-find over the moves, by script.
    let mut parent: Vec<usize> = (0..moves.len()).collect();
    fn root(parent: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while parent[r] != r {
            r = parent[r];
        }
        parent[i] = r;
        r
    }
    let mut owner: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, m) in moves.iter().enumerate() {
        for script in [m.from_script.as_str(), m.to_script.as_str()] {
            match owner.get(script) {
                Some(&j) => {
                    let (a, b) = (root(&mut parent, i), root(&mut parent, j));
                    parent[a.max(b)] = a.min(b);
                }
                None => {
                    owner.insert(script, i);
                }
            }
        }
    }
    let mut sets: BTreeMap<usize, MoveSet> = BTreeMap::new();
    for (i, m) in moves.iter().enumerate() {
        let r = root(&mut parent, i);
        sets.entry(r)
            .or_insert_with(|| MoveSet { moves: Vec::new() })
            .moves
            .push(m.clone());
    }
    sets.into_values().collect()
}

/// One step of a run's order once its move sets are placed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// An ordinary Worker push.
    Worker(String),
    /// Set `n`'s phase 1: every receiver, forwarding the moving contexts.
    Receive(usize),
    /// Set `n`'s phase 2, then its phase 3.
    Commit(usize),
}

/// The run's order with each move set placed as its two nodes, and each set's callers (the
/// Workers outside it that call a receiver, pushed between the nodes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Placement {
    pub(crate) steps: Vec<Step>,
    pub(crate) callers: Vec<Vec<String>>,
}

/// The synthetic node names [`place`] orders with: a NUL keeps them apart from any directory.
fn receive_node(n: usize) -> String {
    ["\0receive:", &n.to_string()].concat()
}

fn commit_node(n: usize) -> String {
    ["\0commit:", &n.to_string()].concat()
}

/// Place `sets` in the run's `order` (§3.2): or, when the two nodes cannot be ordered, the
/// cycle, with each node named as the plan says it (`receive of move set 1`).
pub(crate) fn place(
    sets: &[MoveSet],
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> Result<Placement, Vec<String>> {
    let in_run: BTreeSet<&str> = order.iter().map(String::as_str).collect();
    let receiver_of: BTreeMap<String, usize> = sets
        .iter()
        .enumerate()
        .flat_map(|(n, s)| s.receivers().into_iter().map(move |w| (w, n)))
        .collect();
    let source_of: BTreeMap<String, usize> = sets
        .iter()
        .enumerate()
        .flat_map(|(n, s)| s.built_sources(resources).into_iter().map(move |w| (w, n)))
        .collect();
    let set_of = |w: &str| receiver_of.get(w).or_else(|| source_of.get(w)).copied();
    // A binding target as a node: a receiver's binding is live from its set's receive, a staying
    // source's once its set has committed.
    let node_of = |target: &str| -> String {
        if let Some(&n) = receiver_of.get(target) {
            receive_node(n)
        } else if let Some(&n) = source_of.get(target) {
            commit_node(n)
        } else {
            target.to_string()
        }
    };
    let binds = |w: &str| -> Vec<&str> {
        resources
            .get(w)
            .map(|r| r.binds_to.iter().map(String::as_str).collect())
            .unwrap_or_default()
    };
    let mut graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut callers: Vec<Vec<String>> = vec![Vec::new(); sets.len()];
    for w in order {
        if set_of(w).is_some() {
            continue;
        }
        let deps: Vec<String> = binds(w)
            .into_iter()
            .filter(|t| in_run.contains(t))
            .map(&node_of)
            .collect();
        for t in binds(w) {
            if let Some(&n) = receiver_of.get(t)
                && !callers[n].contains(w)
            {
                callers[n].push(w.clone());
            }
        }
        graph.insert(w.clone(), deps);
    }
    for (n, set) in sets.iter().enumerate() {
        let own = |t: &str| set_of(t) == Some(n);
        let outside = |workers: Vec<String>| -> Vec<String> {
            workers
                .iter()
                .flat_map(|w| binds(w))
                .filter(|t| in_run.contains(t) && !own(t))
                .map(&node_of)
                .collect()
        };
        graph.insert(receive_node(n), outside(set.receivers()));
        let mut commit = vec![receive_node(n)];
        commit.extend(outside(set.built_sources(resources)));
        commit.extend(callers[n].iter().cloned());
        graph.insert(commit_node(n), commit);
    }
    // The order to visit in: the run's, each set's two nodes at its first Worker.
    let mut visit: Vec<String> = Vec::new();
    for w in order {
        match set_of(w) {
            Some(n) => {
                for node in [receive_node(n), commit_node(n)] {
                    if !visit.contains(&node) {
                        visit.push(node);
                    }
                }
            }
            None => visit.push(w.clone()),
        }
    }
    let named = |node: &str| -> String {
        match node.strip_prefix("\0receive:") {
            Some(n) => ["receive of move set ", &plus_one(n)].concat(),
            None => match node.strip_prefix("\0commit:") {
                Some(n) => ["commit of move set ", &plus_one(n)].concat(),
                None => node.to_string(),
            },
        }
    };
    match deploy_order(&visit, &graph) {
        Ok(nodes) => Ok(Placement {
            steps: nodes
                .iter()
                .map(|node| {
                    if let Some(n) = node.strip_prefix("\0receive:") {
                        Step::Receive(n.parse().expect("a node this function named"))
                    } else if let Some(n) = node.strip_prefix("\0commit:") {
                        Step::Commit(n.parse().expect("a node this function named"))
                    } else {
                        Step::Worker(node.clone())
                    }
                })
                .collect(),
            callers,
        }),
        Err(OrderError::Cycle(path)) => Err(path.iter().map(|n| named(n)).collect()),
    }
}

fn plus_one(n: &str) -> String {
    n.parse::<usize>()
        .map_or_else(|_| n.to_string(), |n| (n + 1).to_string())
}

/// #3565: one move set as the plan's JSON lists it (`moves`): its moves, and the Workers each
/// phase pushes, by script, with the callers rebound between the two nodes.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct PlannedSet {
    pub(crate) moves: Vec<PlannedMove>,
    pub(crate) receive: Vec<String>,
    pub(crate) rebind: Vec<String>,
    pub(crate) commit: Vec<String>,
    pub(crate) go_live: Vec<String>,
}

/// One move of a [`PlannedSet`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct PlannedMove {
    pub(crate) context: String,
    pub(crate) agents: Vec<String>,
    pub(crate) from: String,
    pub(crate) to: String,
}

impl PlannedSet {
    pub(crate) fn of(set: &MoveSet, callers: &[String], script: impl Fn(&str) -> String) -> Self {
        PlannedSet {
            moves: set
                .moves
                .iter()
                .map(|m| PlannedMove {
                    context: m.context.clone(),
                    agents: m.classes.clone(),
                    from: m.from_script.clone(),
                    to: m.to_script.clone(),
                })
                .collect(),
            receive: set.receiver_scripts(),
            rebind: callers.iter().map(|w| script(w)).collect(),
            commit: set.source_scripts(),
            go_live: set.receiver_scripts(),
        }
    }

    /// The plan's text lines for this set (`n` counts from 1).
    pub(crate) fn lines(&self, n: usize) -> Vec<String> {
        let mut lines = vec![
            [
                "move set ",
                &n.to_string(),
                ": ",
                &self
                    .moves
                    .iter()
                    .map(|m| {
                        [
                            "`",
                            &m.context,
                            "` (",
                            &m.agents.join(", "),
                            ") from `",
                            &m.from,
                            "` to `",
                            &m.to,
                            "`",
                        ]
                        .concat()
                    })
                    .collect::<Vec<_>>()
                    .join("; "),
            ]
            .concat(),
        ];
        let list = |scripts: &[String]| scripts.join(", ");
        lines.push(
            [
                "  1 receive: ",
                &list(&self.receive),
                " (forwarding the moving contexts)",
            ]
            .concat(),
        );
        if !self.rebind.is_empty() {
            lines.push(["  then rebind: ", &list(&self.rebind)].concat());
        }
        lines.push(["  2 commit: ", &list(&self.commit)].concat());
        lines.push(["  3 go live: ", &list(&self.go_live)].concat());
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mv(context: &str, from: &str, to: &str, built: bool) -> AgentMove {
        AgentMove {
            context: context.into(),
            classes: vec!["K".into()],
            from_worker: from.into(),
            from_script: from.into(),
            to_worker: to.into(),
            to_script: to.into(),
            old_script_built: built,
            renamed_in_place: false,
            tombstones: Vec::new(),
        }
    }

    fn worker(binds: &[&str]) -> Resources {
        Resources {
            binds_to: binds.iter().map(|b| b.to_string()).collect(),
            ..Resources::default()
        }
    }

    /// Fan-in: two standalone Workers joining one group are one set; an unrelated move is
    /// another.
    #[test]
    fn moves_sharing_a_script_are_one_set() {
        let sets = move_sets(&[
            mv("a", "a", "core", false),
            mv("b", "b", "core", false),
            mv("c", "c", "hub", false),
        ]);
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].sources(), ["a", "b"]);
        assert_eq!(sets[0].receivers(), ["core"]);
        assert_eq!(sets[1].receivers(), ["hub"]);
    }

    #[test]
    fn a_script_that_is_both_source_and_receiver_is_chained() {
        let sets = move_sets(&[mv("a", "g1", "g2", true), mv("b", "g2", "g1", true)]);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].chained().as_deref(), Some("g1"));
    }

    /// A caller of the receiver is pushed between the set's two nodes; what the receiver calls
    /// comes before it, and the rest after.
    #[test]
    fn callers_are_rebound_between_the_two_nodes() {
        let sets = move_sets(&[mv("a", "a", "core", false)]);
        let resources = BTreeMap::from([
            ("core".to_string(), worker(&["billing"])),
            ("billing".to_string(), worker(&[])),
            ("web".to_string(), worker(&["core"])),
        ]);
        let order = ["billing", "core", "web"].map(String::from);
        let placed = place(&sets, &order, &resources).unwrap();
        assert_eq!(
            placed.steps,
            [
                Step::Worker("billing".into()),
                Step::Receive(0),
                Step::Worker("web".into()),
                Step::Commit(0),
            ]
        );
        assert_eq!(placed.callers, [vec!["web".to_string()]]);
    }

    /// A caller of the receiver that also calls a staying source cannot be placed: it must come
    /// before the commit (it calls the receiver) and after it (it calls the source as committed).
    #[test]
    fn a_set_whose_nodes_cannot_be_ordered_names_the_cycle() {
        let sets = move_sets(&[mv("a", "core", "hub", true)]);
        let resources = BTreeMap::from([
            ("core".to_string(), worker(&[])),
            ("hub".to_string(), worker(&[])),
            ("web".to_string(), worker(&["hub", "core"])),
        ]);
        let order = ["hub", "web", "core"].map(String::from);
        let cycle = place(&sets, &order, &resources).unwrap_err();
        assert!(
            cycle.contains(&"commit of move set 1".to_string()),
            "{cycle:?}"
        );
        assert!(cycle.contains(&"web".to_string()), "{cycle:?}");
    }

    #[test]
    fn a_planned_set_reads_as_its_phases() {
        let sets = move_sets(&[mv("shop.orders", "shop-orders", "core", false)]);
        let planned = PlannedSet::of(&sets[0], &["web".to_string()], |w| w.to_string());
        assert_eq!(
            planned.lines(1),
            [
                "move set 1: `shop.orders` (K) from `shop-orders` to `core`",
                "  1 receive: core (forwarding the moving contexts)",
                "  then rebind: web",
                "  2 commit: shop-orders",
                "  3 go live: core",
            ]
        );
    }
}
